mod common;

use rusqlite::{Connection, params};
use teamarness_package_host::{PackageErrorCode, PackageHost};

#[allow(clippy::too_many_arguments)]
fn insert_lease(
    private: &std::path::Path,
    pid: u32,
    process_group: i32,
    start_time: &str,
    executable: &std::path::Path,
    digest: &str,
    token: &str,
    workspace: &str,
) {
    let database = private.join("packages/package-host.sqlite3");
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    let connection = Connection::open(database).unwrap();
    connection
        .execute_batch(include_str!("../migrations/0001_package_host.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO process_leases(package_identity, workspace_id, process_generation, pid, process_group, start_time, canonical_executable, executable_digest, launch_token) VALUES (?1, ?2, 7, ?3, ?4, ?5, ?6, ?7, ?8)",
            params!["dev.teamarness.orphan", workspace, pid, process_group, start_time, executable.to_string_lossy(), digest, token],
        )
        .unwrap();
}

#[tokio::test]
async fn incomplete_or_reused_pid_evidence_is_never_killed_and_blocks_relaunch() {
    let fixture = common::Fixture::node(&[("orphan", "dev.teamarness.orphan")]).await;
    let workspace = fixture.core.list_workspaces().unwrap()[0].id().to_string();
    let executable = std::env::current_exe().unwrap();
    insert_lease(
        &fixture._private,
        std::process::id(),
        i32::try_from(std::process::id()).unwrap(),
        "definitely-not-this-process-start-time",
        &executable,
        &common::digest(&std::fs::read(&executable).unwrap()),
        "missing-token",
        &workspace,
    );
    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let snapshot = host.snapshot().await;
    assert!(
        snapshot.instances.is_empty(),
        "ambiguous lease blocked a second process"
    );
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|error| error.code == PackageErrorCode::OrphanUnresolved)
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn exact_lease_evidence_terminates_the_process_group_and_removes_the_lease() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let fixture = common::Fixture::node(&[]).await;
    let executable = std::fs::canonicalize("/usr/bin/yes").unwrap();
    let token = format!("orphan-{}", std::process::id());
    let argument = format!("--teamarness-launch-token={token}");
    let mut child = Command::new(&executable)
        .arg(&argument)
        .arg("--teamarness-protocol=1")
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut system = System::new_all();
    let deadline = Instant::now() + Duration::from_secs(5);
    let start_time = loop {
        system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
        if let Some(process) = system.process(Pid::from_u32(pid)) {
            break process.start_time().to_string();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    };
    insert_lease(
        &fixture._private,
        pid,
        i32::try_from(pid).unwrap(),
        &start_time,
        &executable,
        &common::digest(&std::fs::read(&executable).unwrap()),
        &token,
        "workspace1",
    );

    let host = PackageHost::open(fixture.config.clone(), fixture.core.clone())
        .await
        .unwrap();
    let diagnostics = host.snapshot().await.diagnostics;
    if diagnostics
        .iter()
        .any(|error| error.code == PackageErrorCode::OrphanUnresolved)
    {
        let _ = child.kill();
        let _ = child.wait();
        panic!("exact process evidence was treated as ambiguous");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success());
    let connection =
        Connection::open(fixture._private.join("packages/package-host.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM process_leases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
