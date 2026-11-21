#![cfg(target_os = "linux")]

mod common;

use rusqlite::{Connection, params};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use teamarness_package_host::{PackageErrorCode, PackageHost};

struct IsolatedProcess(Child);
impl Drop for IsolatedProcess {
    fn drop(&mut self) {
        let group = nix::unistd::Pid::from_raw(i32::try_from(self.0.id()).unwrap());
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        let _ = self.0.wait();
    }
}
fn spawn(token: &str) -> IsolatedProcess {
    IsolatedProcess(
        Command::new("/bin/sh")
            .args(["-c", "while :; do sleep 1; done", token])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
fn start_time(pid: u32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let ticks = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    format!("{}:{ticks}", boot.trim())
}
fn lease(private: &Path, pid: u32, mismatch: &str, workspace: &str) {
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    let digest = common::digest(&std::fs::read(&executable).unwrap());
    let database = private.join("packages/package-host.sqlite3");
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    let connection = Connection::open(database).unwrap();
    connection
        .execute_batch(include_str!("../migrations/0001_package_host.sql"))
        .unwrap();
    connection.execute("INSERT INTO process_leases(package_identity,workspace_id,process_generation,pid,process_group,start_time,canonical_executable,executable_digest,launch_token) VALUES (?1,?2,7,?3,?4,?5,?6,?7,?8)",
        params!["dev.teamarness.orphan", workspace, pid,
            if mismatch == "group" { 1 } else { i32::try_from(pid).unwrap() },
            if mismatch == "start" { "other-boot:0".into() } else { start_time(pid) },
            if mismatch == "executable" { "/unrelated".into() } else { executable.to_string_lossy().into_owned() },
            if mismatch == "digest" { "0".repeat(64) } else { digest },
            if mismatch == "token" { "wrong-token" } else { "linux-orphan" }]).unwrap();
}

#[tokio::test]
async fn exact_linux_survivor_is_reclaimed_and_lease_cleared() {
    let fixture = common::Fixture::node(&[]).await;
    let mut process = spawn("--teamarness-launch-token=linux-orphan");
    let mut unrelated = spawn("--teamarness-launch-token=unrelated");
    lease(&fixture._private, process.0.id(), "", "workspace1");
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    assert!(
        !host
            .snapshot()
            .await
            .diagnostics
            .iter()
            .any(|error| error.code == PackageErrorCode::OrphanUnresolved)
    );
    assert!(process.0.wait().unwrap().code().is_none());
    assert!(unrelated.0.try_wait().unwrap().is_none());
    let count: i64 = Connection::open(fixture._private.join("packages/package-host.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM process_leases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn mismatched_linux_evidence_never_kills_or_unblocks_the_instance() {
    for mismatch in ["token", "executable", "start", "group", "digest"] {
        let fixture = common::Fixture::node(&[("orphan", "dev.teamarness.orphan")]).await;
        let mut process = spawn("--teamarness-launch-token=linux-orphan");
        let workspace = fixture.core.list_workspaces().unwrap()[0].id().to_string();
        lease(&fixture._private, process.0.id(), mismatch, &workspace);
        let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
            .await
            .unwrap();
        let snapshot = host.snapshot().await;
        assert!(snapshot.instances.is_empty(), "mismatch: {mismatch}");
        assert!(
            snapshot
                .diagnostics
                .iter()
                .any(|error| error.code == PackageErrorCode::OrphanUnresolved),
            "mismatch: {mismatch}"
        );
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "mismatch: {mismatch}"
        );
    }
}
