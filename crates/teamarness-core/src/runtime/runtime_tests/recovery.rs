//! Restart validation is incremental: it checks what was committed since the
//! last validated start, not the whole history.
//!
//! Real components: real [`CoreRuntime`](super::super::CoreRuntime) instances on
//! temporary directories with real SQLite. Nothing is mocked.

use super::super::{CoreRuntime, CoreRuntimeConfig, SystemClock};
use super::cluster::cluster_root;

const PACKAGE: &str = "teamarness.test";

/// Verifies: TEAM.SYS.TRANS-R07
#[tokio::test]
async fn restart_validates_only_records_committed_since_the_last_validated_start() {
    let root = cluster_root("recovery-validation-mark");
    let _ = std::fs::remove_dir_all(&root);
    let config = CoreRuntimeConfig::new(root.join("node"));

    let open = || async {
        CoreRuntime::open_with_clock_and_readiness(config.clone(), Box::new(SystemClock), true)
            .await
            .expect("the Installation opens")
    };

    let runtime = open().await;
    let workspace = runtime.create_workspace("Recovery").unwrap();
    for index in 0..10_u8 {
        runtime
            .append_record(workspace.id(), PACKAGE, "message.post", &[index; 8])
            .unwrap();
    }
    assert_eq!(
        runtime.recovery_report_for_test().records_validated,
        0,
        "a first open has nothing committed to validate"
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let runtime = open().await;
    let first = runtime.recovery_report_for_test();
    assert_eq!(first.records_validated, 10);
    assert_eq!(first.scopes_quarantined, 0);
    assert_eq!(first.records_quarantined, 0);
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let runtime = open().await;
    assert_eq!(
        runtime.recovery_report_for_test().records_validated,
        0,
        "nothing new was committed, so nothing is re-read"
    );
    for index in 0..2_u8 {
        runtime
            .append_record(workspace.id(), PACKAGE, "message.post", &[index + 100; 8])
            .unwrap();
    }
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let runtime = open().await;
    assert_eq!(
        runtime.recovery_report_for_test().records_validated,
        2,
        "only the records committed since the last validated start"
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let _ = std::fs::remove_dir_all(&root);
}
