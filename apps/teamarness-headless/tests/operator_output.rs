//! Real operator subprocesses against a private protocol peer, without opening Core.
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Operator(Child);
impl Drop for Operator {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

fn peer() -> (tempfile::TempDir, Operator, UnixStream, Value) {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
        .arg("--data-dir")
        .arg(directory.path())
        .args(["identity", "set-name", "--name", "proof"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let operator = Operator(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "operator did not connect");
                // poll-interval: observe subprocess connection, not a mutation retry.
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept: {error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut length = [0; 4];
    stream.read_exact(&mut length).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut bytes).unwrap();
    let request: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(request["operation"]["kind"], "identity_set_name");
    (directory, operator, stream, request)
}

fn reply(stream: &mut UnixStream, request: &Value, error: bool) -> Value {
    let value = if error {
        json!({"code":"host_failure","retryable":false,"outcomeUncertain":true,
            "details":{"kind":"core","family":"synthetic","code":"x".repeat(1024 * 1024)}})
    } else {
        json!({"proof":"x".repeat(1024 * 1024)})
    };
    let result = if error {
        json!({"Err":value})
    } else {
        json!({"Ok":value})
    };
    let bytes = serde_json::to_vec(&json!({"major":1,"id":request["id"],"result":result})).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
    value
}

fn wait(operator: &mut Operator) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = operator.0.try_wait().unwrap() {
            return status.code().expect("normal exit, not signal death");
        }
        assert!(
            Instant::now() < deadline,
            "operator remained blocked after signal with reader open"
        );
        // poll-interval: bound process exit observation while the pipe reader stays open.
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn interrupted(signal: &str, terminal_error: Option<bool>) {
    let (_directory, mut operator, mut stream, request) = peer();
    let mut held_reader = None;
    if let Some(error) = terminal_error {
        reply(&mut stream, &request, error);
        let mut reader = operator.0.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut byte = [0];
            reader.read_exact(&mut byte).unwrap();
            send.send(reader).unwrap();
        });
        held_reader = Some(
            receive
                .recv_timeout(Duration::from_secs(5))
                .expect("terminal output began"),
        );
    }
    assert!(
        Command::new("/bin/kill")
            .args([signal, &operator.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(wait(&mut operator), 130);
    // Keep the non-draining reader alive until after real process exit.
    drop(held_reader);
    let mut stderr = String::new();
    operator
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("client_interrupted outcomeUncertain=true"),
        "{stderr}"
    );
}

// Verifies: TEAM.SYS.PKG-R09.
#[test]
fn sigint_interrupts_success_output_with_open_non_draining_pipe() {
    interrupted("-INT", Some(false));
}
#[test]
fn sigterm_interrupts_success_output_with_open_non_draining_pipe() {
    interrupted("-TERM", Some(false));
}
#[test]
fn sigint_interrupts_error_details_with_open_non_draining_pipe() {
    interrupted("-INT", Some(true));
}
#[test]
fn sigterm_interrupts_error_details_with_open_non_draining_pipe() {
    interrupted("-TERM", Some(true));
}
#[test]
fn signals_before_response_exit_130_without_replay() {
    for signal in ["-INT", "-TERM"] {
        interrupted(signal, None);
    }
}
#[test]
fn complete_output_and_broken_pipe_preserve_terminal_exit_semantics() {
    for error in [false, true] {
        for broken in [false, true] {
            let (_directory, mut operator, mut stream, request) = peer();
            if broken {
                drop(operator.0.stdout.take());
            }
            let value = reply(&mut stream, &request, error);
            let bytes = if !broken {
                let mut reader = operator.0.stdout.take().unwrap();
                let (send, receive) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let mut bytes = Vec::new();
                    reader.read_to_end(&mut bytes).unwrap();
                    let _ = send.send(bytes);
                });
                receive
                    .recv_timeout(Duration::from_secs(5))
                    .expect("complete terminal output")
            } else {
                Vec::new()
            };
            assert_eq!(wait(&mut operator), i32::from(error || broken));
            if !broken {
                assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), value);
            }
            let mut stderr = String::new();
            operator
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            if broken {
                assert!(stderr.contains("output_failed"), "{stderr}");
                assert!(
                    stderr.contains("outcome_unknown outcomeUncertain=true"),
                    "{stderr}"
                );
            }
        }
    }
}
