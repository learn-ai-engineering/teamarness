use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use resonance_core::{CoreRuntime, CoreRuntimeConfig};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "resonance-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

#[tokio::test]
async fn accepted_set_version_is_content_derived_and_restart_stable() {
    let directory = private_data_directory("membership-version");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open(config.clone()).await.unwrap();
    let workspace = runtime.create_workspace("Convergent").unwrap();
    let before = runtime.membership_snapshot(workspace.id()).unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open(config).await.unwrap();
    let after = reopened.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(after.version(), before.version());
    assert_eq!(after.members(), before.members());

    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}
