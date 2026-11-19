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

pub const HELP: &str = "teamarness-headless

Usage:
  teamarness-headless --help | --version
  teamarness-headless serve --data-dir <private-dir> [--application-root <resource-root>]
  teamarness-headless --data-dir <private-dir> status
  teamarness-headless --data-dir <private-dir> identity show
  teamarness-headless --data-dir <private-dir> identity set-name --name <name>
  teamarness-headless --data-dir <private-dir> workspace list
  teamarness-headless --data-dir <private-dir> workspace join --person-name <name> --invitation-file <file|->
  teamarness-headless --data-dir <private-dir> workspace members --workspace <id>
  teamarness-headless --data-dir <private-dir> workspace rename-self --workspace <id> --name <name>
  teamarness-headless --data-dir <private-dir> agent list --workspace <id>
  teamarness-headless --data-dir <private-dir> agent add --workspace <id> --name <name>
  teamarness-headless --data-dir <private-dir> agent rename --workspace <id> --agent <id> --name <name>
  teamarness-headless --data-dir <private-dir> agent remove --workspace <id> --agent <id>
  teamarness-headless --data-dir <private-dir> packages list
  teamarness-headless --data-dir <private-dir> packages inspect --directory <absolute-path>
  teamarness-headless --data-dir <private-dir> packages install --directory <absolute-path> --package <expected-identity> --input <file|->
  teamarness-headless --data-dir <private-dir> packages reload|remove --package <identity>
  teamarness-headless --data-dir <private-dir> packages restart|pause|resume --workspace <id> --package <identity>

  teamarness-headless --data-dir <private-dir> package operations --workspace <id> --package <identity>
  teamarness-headless --data-dir <private-dir> package schema --workspace <id> --package <identity> --operation <name>
  teamarness-headless --data-dir <private-dir> package request --workspace <id> --package <identity> --operation <name> --contract-digest <digest> --input <file|->

Install input: {\"acceptedRisks\":[\"filesystem\",\"network\",\"process\"]}.
Input files must be owned regular non-symlink files with no group/other bits.
Results are JSON. Upstream errors include details on stdout, never routine stderr.
Package request failures after validation are conservatively outcome-uncertain and nonretryable.
Embedded Host/Package errors retain their original details and retryability; they do not prove no effect.
Known input/digest/busy refusals do not dispatch. Inspect Package state; never replay automatically.
Clients never initialize storage or retry mutations.
A lost mutation reply means outcome unknown. Inspect state before taking further action.
";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Serve {
        data_directory: PathBuf,
        application_root: Option<PathBuf>,
    },
    Operator {
        data_directory: PathBuf,
        command: OperatorCommand,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum OperatorCommand {
    Ready(Operation),
    Join {
        person_name: String,
        invitation_file: PathBuf,
    },
    Request {
        workspace: String,
        package: teamarness_package_host::PackageIdentity,
        operation: String,
        contract_digest: String,
        input: PathBuf,
    },
    Install {
        directory: PathBuf,
        expected_identity: teamarness_package_host::PackageIdentity,
        input: PathBuf,
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
    let mut options = std::collections::BTreeMap::new();
    let mut words = Vec::new();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        if argument.as_encoded_bytes().starts_with(b"--") {
            let name = argument.into_string().map_err(|_| invalid())?;
            let value = args.next().ok_or_else(invalid)?;
            if value.is_empty()
                || value.as_encoded_bytes().starts_with(b"--")
                || options.insert(name, value).is_some()
            {
                return Err(invalid());
            }
        } else {
            words.push(argument.into_string().map_err(|_| invalid())?);
        }
    }
    let data_directory = PathBuf::from(options.remove("--data-dir").ok_or_else(invalid)?);
    let mut value = |name: &str| options.remove(name).ok_or_else(invalid);
    let command = match words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["serve"] => Command::Serve {
            data_directory,
            application_root: options.remove("--application-root").map(PathBuf::from),
        },
        words => {
            let command = match words {
                ["status"] => OperatorCommand::Ready(Operation::Status {}),
                ["identity", "show"] => OperatorCommand::Ready(Operation::IdentityShow {}),
                ["identity", "set-name"] => OperatorCommand::Ready(Operation::IdentitySetName {
                    name: text(value("--name")?)?,
                }),
                ["workspace", "list"] => OperatorCommand::Ready(Operation::WorkspaceList {}),
                ["workspace", "join"] => OperatorCommand::Join {
                    person_name: text(value("--person-name")?)?,
                    invitation_file: value("--invitation-file")?.into(),
                },
                ["workspace", "members"] => OperatorCommand::Ready(Operation::WorkspaceMembers {
                    workspace: workspace(value("--workspace")?)?,
                }),
                ["workspace", "rename-self"] => {
                    OperatorCommand::Ready(Operation::WorkspaceRenameSelf {
                        workspace: workspace(value("--workspace")?)?,
                        name: text(value("--name")?)?,
                    })
                }
                ["agent", "list"] => OperatorCommand::Ready(Operation::AgentList {
                    workspace: workspace(value("--workspace")?)?,
                }),
                ["agent", "add"] => OperatorCommand::Ready(Operation::AgentAdd {
                    workspace: workspace(value("--workspace")?)?,
                    name: text(value("--name")?)?,
                }),
                ["agent", "rename"] => OperatorCommand::Ready(Operation::AgentRename {
                    workspace: workspace(value("--workspace")?)?,
                    agent: agent(value("--agent")?)?,
                    name: text(value("--name")?)?,
                }),
                ["agent", "remove"] => OperatorCommand::Ready(Operation::AgentRemove {
                    workspace: workspace(value("--workspace")?)?,
                    agent: agent(value("--agent")?)?,
                }),
                ["package", "operations"] => OperatorCommand::Ready(Operation::PackageOperations {
                    workspace: workspace(value("--workspace")?)?,
                    package: text(value("--package")?)?.parse().map_err(|_| invalid())?,
                }),
                ["package", "schema"] => OperatorCommand::Ready(Operation::PackageSchema {
                    workspace: workspace(value("--workspace")?)?,
                    package: text(value("--package")?)?.parse().map_err(|_| invalid())?,
                    operation: text(value("--operation")?)?,
                }),
                ["package", "request"] => OperatorCommand::Request {
                    workspace: workspace(value("--workspace")?)?,
                    package: text(value("--package")?)?.parse().map_err(|_| invalid())?,
                    operation: text(value("--operation")?)?,
                    contract_digest: text(value("--contract-digest")?)?,
                    input: value("--input")?.into(),
                },
                ["packages", "list"] => OperatorCommand::Ready(Operation::PackagesList {}),
                ["packages", "inspect"] => OperatorCommand::Ready(Operation::PackagesInspect {
                    directory: value("--directory")?.into(),
                }),
                ["packages", "install"] => OperatorCommand::Install {
                    directory: value("--directory")?.into(),
                    expected_identity: text(value("--package")?)?.parse().map_err(|_| invalid())?,
                    input: value("--input")?.into(),
                },
                ["packages", "reload"] => OperatorCommand::Ready(Operation::PackagesReload {
                    package: text(value("--package")?)?.parse().map_err(|_| invalid())?,
                }),
                ["packages", "remove"] => OperatorCommand::Ready(Operation::PackagesRemove {
                    package: text(value("--package")?)?.parse().map_err(|_| invalid())?,
                }),
                ["packages", verb @ ("restart" | "pause" | "resume")] => {
                    let workspace = workspace(value("--workspace")?)?;
                    let package = text(value("--package")?)?.parse().map_err(|_| invalid())?;
                    OperatorCommand::Ready(match *verb {
                        "restart" => Operation::PackagesRestart { workspace, package },
                        "pause" => Operation::PackagesPause { workspace, package },
                        _ => Operation::PackagesResume { workspace, package },
                    })
                }
                _ => return Err(invalid()),
            };
            Command::Operator {
                data_directory,
                command,
            }
        }
    };
    if !options.is_empty() {
        return Err(invalid());
    }
    Ok(command)
}

fn invalid() -> ControlError {
    ControlError::new(ErrorCode::InvalidInput)
}
fn text(value: OsString) -> Result<String, ControlError> {
    value.into_string().map_err(|_| invalid())
}
fn workspace(value: OsString) -> Result<String, ControlError> {
    let value = text(value)?;
    crate::administration::parse_workspace(&value)?;
    Ok(value)
}
fn agent(value: OsString) -> Result<String, ControlError> {
    let value = text(value)?;
    value
        .parse::<teamarness_core::PrincipalId>()
        .map_err(|_| invalid())?;
    Ok(value)
}

impl OperatorCommand {
    fn resolve(self) -> Result<Operation, ControlError> {
        match self {
            Self::Ready(operation) => Ok(operation),
            Self::Join {
                person_name,
                invitation_file,
            } => {
                let bytes = crate::input::read_private(
                    &invitation_file,
                    crate::input::MAX_INVITATION_BYTES,
                )?;
                let invitation = std::str::from_utf8(&bytes).map_err(|_| invalid())?.trim();
                invitation
                    .parse::<teamarness_core::WorkspaceInvitation>()
                    .map_err(|_| invalid())?;
                Ok(Operation::WorkspaceJoin {
                    invitation: invitation.to_owned(),
                    person_name,
                })
            }
            Self::Request {
                workspace,
                package,
                operation,
                contract_digest,
                input,
            } => {
                let payload = crate::input::json(&crate::input::read_private(
                    &input,
                    crate::input::MAX_INPUT_BYTES,
                )?)?;
                Ok(Operation::PackageRequest {
                    workspace,
                    package,
                    operation,
                    contract_digest,
                    payload,
                })
            }
            Self::Install {
                directory,
                expected_identity,
                input,
            } => {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Consent {
                    accepted_risks: Vec<String>,
                }
                let consent: Consent = crate::input::json(&crate::input::read_private(
                    &input,
                    crate::input::MAX_INPUT_BYTES,
                )?)?;
                Ok(Operation::PackagesInstall {
                    directory,
                    expected_identity,
                    accepted_risks: consent.accepted_risks,
                })
            }
        }
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
        Command::Operator {
            data_directory,
            command,
        } => {
            let operation = match tokio::task::spawn_blocking(move || command.resolve()).await {
                Ok(Ok(operation)) => operation,
                Ok(Err(error)) => {
                    eprintln!("{error}");
                    return 2;
                }
                Err(_) => {
                    eprintln!("input_failed");
                    return 2;
                }
            };
            let mutation = operation.is_mutation();
            let stop = match terminal_signal() {
                Ok(stop) => stop,
                Err(error) => {
                    eprintln!("signal_setup: {error}");
                    return 1;
                }
            };
            let observe_and_output = async {
                let result = control::request(
                    &data_directory,
                    Request {
                        major: PROTOCOL_MAJOR,
                        id: 1,
                        operation,
                    },
                )
                .await;
                let (send, receive) = tokio::sync::oneshot::channel();
                // A pipe can block forever. A dedicated thread is deliberately not joined: unlike
                // spawn_blocking it cannot hold Tokio runtime shutdown before main's process exit.
                if std::thread::Builder::new()
                    .name("operator-output".into())
                    .spawn(move || {
                        let _ = send.send(operator_output(
                            result.and_then(|response| response.result),
                            mutation,
                        ));
                    })
                    .is_err()
                {
                    eprintln!("output_thread_failed outcomeUncertain={mutation}");
                    return 1;
                }
                receive.await.unwrap_or(1)
            };
            tokio::select! {
                code = observe_and_output => code,
                _ = stop => { eprintln!("client_interrupted outcomeUncertain={mutation}"); 130 }
            }
        }
    }
}

fn operator_output(result: Result<serde_json::Value, ControlError>, mutation: bool) -> i32 {
    match result {
        Ok(value) => output_json(&value, mutation),
        Err(error) => {
            let mut diagnostic = error.clone();
            diagnostic.details = None;
            eprintln!("{diagnostic}");
            if error.details.is_some() && output_json(&error, mutation) != 0 {
                return 1;
            }
            if error.outcome_uncertain {
                eprintln!("outcome_unknown: inspect state; do not replay automatically");
            }
            if error.outcome_uncertain {
                1
            } else if matches!(
                error.code,
                ErrorCode::InvalidInput | ErrorCode::UnsupportedVersion | ErrorCode::FrameTooLarge
            ) {
                2
            } else {
                1
            }
        }
    }
}

fn output_json(value: &impl serde::Serialize, mutation: bool) -> i32 {
    let result = match serde_json::to_vec(value) {
        Ok(mut bytes) => {
            bytes.push(b'\n');
            output(&bytes)
        }
        Err(_) => {
            eprintln!("output_encoding_failed");
            1
        }
    };
    if result != 0 && mutation {
        eprintln!("outcome_unknown outcomeUncertain=true");
    }
    result
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
