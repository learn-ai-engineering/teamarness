//! The Package Host reaps the Backend process group it spawned.
//!
//! This counts this test process's own children, so it is the only test in its
//! binary: a sibling test's Backend would be indistinguishable from a leaked
//! one. It drives a real Core Runtime and Package Host, which is why it lives
//! here rather than in the Conversations crate (ARC-10).

#![cfg(unix)]

#[allow(dead_code)]
mod support;

use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
use teamarness_package_host::{PackageIdentity, PackageInstanceKey, WorkspaceId};

const EXIT_DEADLINE: Duration = Duration::from_secs(5);

fn conversations_children() -> Vec<u32> {
    let parent = std::process::id();
    let output = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat=,command="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let ppid = fields.next()?.parse::<u32>().ok()?;
            let stat = fields.next()?;
            let command = fields.collect::<Vec<_>>().join(" ");
            (ppid == parent
                && !stat.starts_with('Z')
                && command.contains("teamarness-conversations-package"))
            .then_some(pid)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_shutdown_reaps_active_backend() {
    let application = support::harness::StagedApplication::new();
    let core_path = application.root.path().join("core");
    let private = application.root.path().join("private");
    let core = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(&core_path))
            .await
            .unwrap(),
    );
    let workspace = core.create_workspace("Lifeline").unwrap();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(support::harness::PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace.id().to_string()).unwrap(),
    );
    let host =
        support::harness::open_host(&application.application_path, private, core.clone()).await;
    let _mount = support::harness::open_mount(&host, &key).await;
    let children = conversations_children();
    assert_eq!(
        children.len(),
        1,
        "expected one Conversations Backend child"
    );

    support::harness::shutdown(host).await;
    let reaped = support::harness::poll_until(EXIT_DEADLINE, || async {
        conversations_children().is_empty()
    })
    .await;
    core.shutdown().await.unwrap();
    assert!(
        reaped,
        "host shutdown left the Conversations Backend running"
    );
}
