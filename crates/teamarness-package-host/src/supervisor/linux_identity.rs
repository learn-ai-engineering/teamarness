//! Linux lease evidence uses boot identity and kernel start ticks, not wall time.

use super::{LeaseEvidence, ProcessLease, fs, sha256};
use std::path::PathBuf;

fn process_stat(pid: u32) -> std::io::Result<(String, i32)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // comm may contain spaces and parentheses. Fields after its final ')' start
    // at field 3 (state); pgrp is field 5 and starttime is field 22.
    let (_, fields) = stat
        .rsplit_once(')')
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    let group = fields
        .get(2)
        .and_then(|field| field.parse().ok())
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let ticks = fields
        .get(19)
        .filter(|field| field.parse::<u64>().is_ok())
        .ok_or(std::io::ErrorKind::InvalidData)?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok((format!("{}:{ticks}", boot.trim()), group))
}

pub(super) fn start_time(pid: u32) -> Option<String> {
    process_stat(pid).ok().map(|(start, _)| start)
}

pub(super) fn inspect(lease: &ProcessLease) -> LeaseEvidence {
    let proc = PathBuf::from(format!("/proc/{}", lease.pid));
    let (start, group) = match process_stat(lease.pid) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Missing /proc or a missing boot/stat source is not proof of
            // death. Only the kernel's no-such-process result clears a lease.
            let Ok(pid) = i32::try_from(lease.pid) else {
                return LeaseEvidence::Ambiguous;
            };
            if pid <= 0 {
                return LeaseEvidence::Ambiguous;
            }
            return match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
                Err(nix::errno::Errno::ESRCH) => LeaseEvidence::Gone,
                _ => LeaseEvidence::Ambiguous,
            };
        }
        Err(_) => return LeaseEvidence::Ambiguous,
    };
    let executable = fs::read_link(proc.join("exe")).ok();
    let expected = fs::canonicalize(&lease.canonical_executable).ok();
    let token = format!("--teamarness-launch-token={}", lease.launch_token);
    let token_matches = fs::read(proc.join("cmdline")).ok().is_some_and(|bytes| {
        bytes
            .split(|byte| *byte == 0)
            .any(|arg| arg == token.as_bytes())
    });
    let digest_matches = fs::read(proc.join("exe"))
        .ok()
        .is_some_and(|bytes| sha256(&bytes) == lease.executable_digest);
    // Recheck the start after all evidence reads to reject a PID reused while
    // inspecting. Only a dedicated group leader can authorize group cleanup.
    if start == lease.start_time
        && i32::try_from(lease.pid).ok() == Some(group)
        && group == lease.process_group
        && executable.is_some()
        && executable == expected
        && digest_matches
        && token_matches
        && process_stat(lease.pid).ok() == Some((start, group))
    {
        LeaseEvidence::Exact
    } else {
        LeaseEvidence::Ambiguous
    }
}
