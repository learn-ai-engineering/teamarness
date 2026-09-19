use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{CoreRuntime, CoreRuntimeConfig, MembershipEpoch};
use rusqlite::Connection;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

fn now_unix_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// Verifies: TEAM.SYS.MEM-R03
#[tokio::test]
async fn concurrent_exact_redemption_recovers_one_shared_admission() {
    let issuer_directory = private_data_directory("invitation-race-issuer");
    let invitee_directory = private_data_directory("invitation-race-invitee");
    let config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Race").unwrap();
    let invitee = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let token = issuer
        .issue_invitation(workspace.id(), now_unix_ms() + 60_000)
        .unwrap();
    let claim = invitee.create_invitation_claim(&token).unwrap();
    let other_issuer = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    let barrier = Arc::new(Barrier::new(2));

    let first_barrier = Arc::clone(&barrier);
    let first_claim = claim.clone();
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        let result = issuer.redeem_invitation(&first_claim);
        (issuer, result)
    });
    let second_barrier = Arc::clone(&barrier);
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        let result = other_issuer.redeem_invitation(&claim);
        (other_issuer, result)
    });
    let (issuer, first_result) = first.join().unwrap();
    let (other_issuer, second_result) = second.join().unwrap();
    let results = [first_result, second_result];

    assert!(results.iter().all(Result::is_ok));
    let causes = results
        .iter()
        .map(|result| result.as_ref().unwrap().cause())
        .collect::<Vec<_>>();
    assert_eq!(causes[0], causes[1]);

    let connection = Connection::open(issuer_directory.join("core.sqlite3")).unwrap();
    let counts: (i64, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM used_invitation_nonces),
                (SELECT COUNT(*) FROM membership_operations WHERE action = 1),
                (SELECT epoch FROM workspace_membership_state LIMIT 1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1, 1));
    let transition = results.into_iter().next().unwrap().unwrap();
    assert_eq!(transition.epoch(), MembershipEpoch::new(1));

    drop(connection);
    issuer.shutdown().await.unwrap();
    other_issuer.shutdown().await.unwrap();
    invitee.shutdown().await.unwrap();
    drop(invitee);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}
