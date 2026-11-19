//! Strict offline parsing and the executable's actual foreground/client composition.
use std::ffi::OsString;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;

use teamarness_core::{CoreError, CoreRuntime, CoreRuntimeConfig};
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::Instant;

use crate::application::{Application, ApplicationConfig, fatal_shutdown};
use crate::control::{self, Operation, PROTOCOL_MAJOR, Request, SHUTDOWN_TIMEOUT};
use crate::{ControlError, ErrorCode};

pub const HELP: &str = "teamarness-headless\n\nUsage:\n  teamarness-headless --help\n  teamarness-headless --version\n  teamarness-headless serve --data-dir <private-dir> [--application-root <resource-root>]\n  teamarness-headless --data-dir <private-dir> status\n\nResults are JSON. Clients never initialize storage.\n";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Serve {
        data_directory: PathBuf,
        application_root: Option<PathBuf>,
    },
    Status {
        data_directory: PathBuf,
    },
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, ControlError> {
    let args: Vec<_> = arguments.into_iter().collect();
    if args == [OsString::from("--help")] || args == [OsString::from("-h")] {
        return Ok(Command::Help);
    }
    if args == [OsString::from("--version")] || args == [OsString::from("-V")] {
        return Ok(Command::Version);
    }
    let mut data_directory = None;
    let mut application_root = None;
    let mut command = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--data-dir") if data_directory.is_none() => {
                data_directory = Some(path_argument(args.next())?);
            }
            Some("--application-root") if application_root.is_none() => {
                application_root = Some(path_argument(args.next())?);
            }
            Some("serve" | "status") if command.is_none() => command = Some(argument),
            _ => return Err(ControlError::new(ErrorCode::InvalidInput)),
        }
    }
    let data_directory =
        data_directory.ok_or_else(|| ControlError::new(ErrorCode::InvalidInput))?;
    match command.as_deref().and_then(|value| value.to_str()) {
        Some("serve") => Ok(Command::Serve {
            data_directory,
            application_root,
        }),
        Some("status") if application_root.is_none() => Ok(Command::Status { data_directory }),
        _ => Err(ControlError::new(ErrorCode::InvalidInput)),
    }
}

fn path_argument(value: Option<OsString>) -> Result<PathBuf, ControlError> {
    match value {
        Some(value) if !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--") => {
            Ok(value.into())
        }
        _ => Err(ControlError::new(ErrorCode::InvalidInput)),
    }
}

pub async fn run(arguments: impl IntoIterator<Item = OsString>) -> i32 {
    // production-addressing: production entry only; tests inject Core's existing loopback opener.
    run_with(arguments, CoreRuntime::open).await
}

/// Runs the same CLI, signal handling, composition and dispatch with an embedding-supplied Core opener.
pub async fn run_with<O, F>(arguments: impl IntoIterator<Item = OsString>, opener: O) -> i32
where
    O: FnOnce(CoreRuntimeConfig) -> F,
    F: Future<Output = Result<CoreRuntime, CoreError>>,
{
    let command = match parse(arguments) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}\n{HELP}");
            return 2;
        }
    };
    match command {
        Command::Help => output(HELP.as_bytes()),
        Command::Version => {
            output(concat!("teamarness-headless ", env!("CARGO_PKG_VERSION"), "\n").as_bytes())
        }
        Command::Serve {
            data_directory,
            application_root,
        } => {
            let root = match application_root {
                Some(root) => root.canonicalize(),
                None => std::env::current_exe()
                    .and_then(|path| path.canonicalize())
                    .and_then(|path| {
                        path.parent()
                            .map(PathBuf::from)
                            .ok_or_else(|| std::io::Error::other("executable has no parent"))
                    }),
            };
            let root = match root {
                Ok(root) if root.is_dir() => root,
                _ => {
                    eprintln!("invalid_application_root");
                    return 2;
                }
            };
            if crate::socket::path(&data_directory).is_err() {
                eprintln!("invalid_socket_path");
                return 2;
            }
            let stop = match terminal_signal() {
                Ok(stop) => stop,
                Err(error) => {
                    eprintln!("signal_setup: {error}");
                    return 1;
                }
            };
            // Record the first terminal signal immediately, including during Core/Host startup.
            let signal_task = tokio::spawn(stop);
            let (sender, receiver) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let _ = sender.send(signal_task.await);
            });
            let mut receiver = receiver;
            let opening = Application::open_with(
                ApplicationConfig {
                    data_directory,
                    application_root: root,
                },
                opener,
            );
            tokio::pin!(opening);
            let mut stopped_at = None;
            let opened = tokio::select! {
                result = &mut opening => result,
                signal = &mut receiver => {
                    let at = signal.ok().and_then(Result::ok).unwrap_or_else(Instant::now);
                    stopped_at = Some(at);
                    match tokio::time::timeout_at(at + SHUTDOWN_TIMEOUT, &mut opening).await {
                        Ok(result) => result,
                        Err(_) => fatal_shutdown("startup"),
                    }
                }
            };
            let application = match opened {
                Ok(application) => application,
                Err(error) => {
                    eprintln!("{error}");
                    return 1;
                }
            };
            if let Some(at) = stopped_at {
                application.shutdown_by(at + SHUTDOWN_TIMEOUT).await;
                return 0;
            }
            match application
                .run_until(async move {
                    receiver
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .unwrap_or_else(Instant::now)
                })
                .await
            {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            }
        }
        Command::Status { data_directory } => {
            let stop = match terminal_signal() {
                Ok(stop) => stop,
                Err(error) => {
                    eprintln!("signal_setup: {error}");
                    return 1;
                }
            };
            let result = tokio::select! {
                result = control::request(&data_directory, Request { major: PROTOCOL_MAJOR, id: 1, operation: Operation::Status {} }) => result,
                _ = stop => { eprintln!("client_interrupted"); return 130; }
            };
            match result.and_then(|response| response.result) {
                Ok(status) => match serde_json::to_vec(&status) {
                    Ok(mut bytes) => {
                        bytes.push(b'\n');
                        output(&bytes)
                    }
                    Err(error) => {
                        eprintln!("output_encoding: {error}");
                        1
                    }
                },
                Err(error) => {
                    eprintln!("{error}");
                    if matches!(
                        error.code,
                        ErrorCode::InvalidInput
                            | ErrorCode::UnsupportedVersion
                            | ErrorCode::FrameTooLarge
                    ) {
                        2
                    } else {
                        1
                    }
                }
            }
        }
    }
}

fn output(bytes: &[u8]) -> i32 {
    match std::io::stdout().lock().write_all(bytes) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("output_failed: {error}");
            1
        }
    }
}

fn terminal_signal() -> std::io::Result<impl Future<Output = Instant> + Send + 'static> {
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} };
        Instant::now()
    })
}
