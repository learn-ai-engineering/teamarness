#![cfg(target_os = "macos")]

use std::{
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid as NixPid, getpgid},
};
use sysinfo::{Pid, ProcessesToUpdate, System};

#[test]
fn observes_exact_child_identity_then_terminates_and_reaps_its_process_group() {
    let launch_token = format!("--teamarness-launch-token={}", std::process::id());
    let executable = std::fs::canonicalize("/usr/bin/yes").expect("canonical test executable");
    let mut child = Command::new(&executable)
        .arg(&launch_token)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn isolated test child");

    let raw_pid = child.id();
    let sysinfo_pid = Pid::from_u32(raw_pid);
    let nix_pid = NixPid::from_raw(i32::try_from(raw_pid).expect("PID fits i32"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut system = System::new_all();
    let evidence = loop {
        system.refresh_processes(ProcessesToUpdate::Some(&[sysinfo_pid]), true);
        if let Some(process) = system.process(sysinfo_pid)
            && process.exe().is_some()
            && !process.cmd().is_empty()
        {
            break (
                process.pid().as_u32(),
                process.start_time(),
                process.exe().map(std::path::Path::to_path_buf),
                process.cmd().to_vec(),
                getpgid(Some(nix_pid)),
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("sysinfo did not expose complete process evidence within five seconds");
        }
        thread::sleep(Duration::from_millis(20));
    };

    let termination = killpg(nix_pid, Signal::SIGTERM);
    let status = child.wait();

    assert_eq!(evidence.0, raw_pid, "observed PID");
    assert!(evidence.1 > 0, "observed process start time");
    assert_eq!(
        evidence.2.as_deref(),
        Some(executable.as_path()),
        "observed canonical executable"
    );
    assert!(
        evidence
            .3
            .iter()
            .any(|argument| argument == std::ffi::OsStr::new(&launch_token)),
        "observed launch token in process arguments"
    );
    assert_eq!(
        evidence.4.expect("process group lookup"),
        nix_pid,
        "observed dedicated process group"
    );
    termination.expect("terminate exact process group");
    let status = status.expect("reap exact child");
    assert_eq!(status.signal(), Some(Signal::SIGTERM as i32));
}
