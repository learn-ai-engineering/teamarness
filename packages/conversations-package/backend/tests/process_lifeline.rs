//! Process-level lifeline tests for the real Conversations Backend executable.
//!
//! The Package Host normally reaps the process group, but a host crash, a
//! `SIGKILL`ed desktop, or a dev-server restart leaves the Backend with only its
//! stdin lifeline. These tests drive the binary directly over framed stdio and
//! assert it exits promptly once the host goes away or asks it to shut down.

#![cfg(unix)]

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use teamarness_package_protocol::{Frame, FrameKind, decode_frame, encode_frame};

const EXIT_DEADLINE: Duration = Duration::from_secs(5);
const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const RECORD_GRANT: &str = "cmVjb3JkLWdyYW50";

// Every test here spawns a Conversations Backend as a child of this test
// process. Serialize them so one test's child is never mistaken for another's.
static CHILD_PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Backend {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Frame>,
    _data: tempfile::TempDir,
}

impl Backend {
    fn launch() -> Self {
        let data = tempfile::tempdir().expect("data directory");
        let mut child = Command::new(env!("CARGO_BIN_EXE_teamarness-conversations-package"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn Conversations Backend");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let (sender, frames) = mpsc::channel();
        thread::spawn(move || read_frames(stdout, &sender));
        let mut backend = Self {
            child,
            stdin,
            frames,
            _data: data,
        };
        let path = backend._data.path().to_string_lossy().into_owned();
        backend.send(json!({
            "kind": "initialize",
            "nonce": "bm9uY2U",
            "package": {
                "identity": "dev.teamarness.conversations",
                "version": "0.1.0",
                "manifestDigest": DIGEST,
                "backendDigest": DIGEST,
            },
            "uiContractDigest": DIGEST,
            "processGeneration": "1",
            "workspace": {"id": "workspace", "name": "Workspace"},
            "installation": {"identity": "installation", "displayName": "Installation"},
            "dataDirectory": {"path": path, "continuityId": "Y29udGludWl0eQ"},
            "protocol": {"major": 1, "minor": 0},
            "capabilities": [{
                "identity": "teamarness.synchronization.immutable-record-log",
                "grant": RECORD_GRANT,
                "major": 1,
                "minor": 0,
            }, {
                "identity": "teamarness.principals",
                "grant": "bmFtZS1ncmFudA",
                "major": 1,
                "minor": 0,
            }],
            "limits": {},
        }));
        backend.expect(
            |frame| frame.kind() == FrameKind::Initialized,
            "initialized",
        );
        backend
    }

    /// Activates the Package and waits until its synchronization session has opened
    /// the long-lived immutable-record-log capability stream, as it does under the
    /// real host.
    fn activate_with_open_capability_stream(&mut self) {
        self.send(json!({
            "kind": "request",
            "id": "h:lifecycle-activate-1",
            "operation": "lifecycle.activate",
            "deadlineUnixMs": "99999999999999",
            "payload": {},
        }));
        let mut activated = false;
        let mut stream_opened = false;
        let deadline = Instant::now() + EXIT_DEADLINE;
        while !(activated && stream_opened) {
            let frame = self.next_frame(deadline).expect("activation frames");
            match frame.kind() {
                FrameKind::Response => activated = true,
                FrameKind::Error => panic!("activation failed: {frame:?}"),
                FrameKind::StreamOpen
                    if frame.field("topic").and_then(Value::as_str)
                        == Some("capability.stream") =>
                {
                    stream_opened = true;
                }
                _ => {}
            }
        }
    }

    fn open_changes_stream(&mut self) {
        self.send(json!({
            "kind": "stream_open",
            "id": "h:changes-1",
            "topic": "conversations.changes",
            "initialCredit": 32,
            "payload": {},
        }));
        self.expect(
            |frame| {
                frame.kind() == FrameKind::Event
                    && frame.field("streamId").and_then(Value::as_str) == Some("h:changes-1")
            },
            "initial conversations.changes event",
        );
    }

    fn send(&mut self, value: Value) {
        let frame = Frame::try_from(value).expect("valid frame");
        let bytes = encode_frame(&frame).expect("encoded frame");
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(&bytes).expect("write frame");
        stdin.flush().expect("flush frame");
    }

    fn next_frame(&self, deadline: Instant) -> Option<Frame> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        self.frames.recv_timeout(remaining).ok()
    }

    fn expect(&self, predicate: impl Fn(&Frame) -> bool, what: &str) {
        let deadline = Instant::now() + EXIT_DEADLINE;
        loop {
            let frame = self
                .next_frame(deadline)
                .unwrap_or_else(|| panic!("timed out waiting for {what}"));
            if predicate(&frame) {
                return;
            }
        }
    }

    /// Simulates host death: the parent's end of the stdin pipe closes.
    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    fn exits_within(&mut self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        loop {
            if self.child.try_wait().expect("try_wait").is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_frames(mut stdout: ChildStdout, sender: &mpsc::Sender<Frame>) {
    loop {
        let mut prefix = [0_u8; 4];
        if stdout.read_exact(&mut prefix).is_err() {
            return;
        }
        let length = u32::from_be_bytes(prefix) as usize;
        let mut bytes = prefix.to_vec();
        bytes.resize(4 + length, 0);
        if stdout.read_exact(&mut bytes[4..]).is_err() {
            return;
        }
        let Ok(frame) = decode_frame(&bytes) else {
            return;
        };
        if sender.send(frame).is_err() {
            return;
        }
    }
}

#[test]
fn backend_exits_when_host_dies_before_activation() {
    let _serial = CHILD_PROCESS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut backend = Backend::launch();
    backend.close_stdin();
    assert!(
        backend.exits_within(EXIT_DEADLINE),
        "Conversations Backend outlived its host before activation"
    );
}

#[test]
fn backend_exits_when_host_dies_with_active_synchronization_session() {
    let _serial = CHILD_PROCESS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut backend = Backend::launch();
    backend.activate_with_open_capability_stream();
    backend.open_changes_stream();
    backend.close_stdin();
    assert!(
        backend.exits_within(EXIT_DEADLINE),
        "Conversations Backend was orphaned after host death with an active session"
    );
}

#[test]
fn backend_exits_after_shutdown_frame_with_active_synchronization_session() {
    let _serial = CHILD_PROCESS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut backend = Backend::launch();
    backend.activate_with_open_capability_stream();
    backend.open_changes_stream();
    backend.send(json!({"kind": "shutdown", "reason": "application_shutdown"}));
    backend.expect(
        |frame| frame.kind() == FrameKind::ShutdownAck,
        "shutdown_ack",
    );
    // The host SIGTERMs the process group right after the acknowledgement. If the
    // host dies in that window, closing stdin must be enough for the Backend to exit.
    backend.close_stdin();
    assert!(
        backend.exits_within(EXIT_DEADLINE),
        "Conversations Backend acknowledged shutdown but did not exit"
    );
}
