#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig, OnboardingError, WorkspaceInvitation};
use teamarness_package_host::{
    InstallationPublicIdentity, OpenUiMount, PackageHost, PackageHostCommand, PackageHostConfig,
    PackageHostOutcome, PackageIdentity, PackageInstanceKey, PackageValue, ShutdownHost, UiRequest,
    UiRequestId, WorkspaceId,
};

const PACKAGE_ID: &str = "dev.teamarness.conversations";
const GENERAL_ID: &str = "00000000000000000000000000000000";

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn complete_two_profile_recovery_matrix() {
    let root = tempfile::tempdir().unwrap();
    let application = stage_package(root.path());
    let alice_core_path = root.path().join("alice-core");
    let bob_core_path = root.path().join("bob-core");
    let alice_private = root.path().join("alice-private");
    let bob_private = root.path().join("bob-private");

    let alice_core = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(&alice_core_path))
            .await
            .unwrap(),
    );
    let workspace = alice_core.create_workspace("Conversations flow").unwrap();
    let workspace_id = workspace.id();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        WorkspaceId::new(workspace_id.to_string()).unwrap(),
    );
    let alice_host = open_host(&application, alice_private.clone(), alice_core.clone()).await;
    let alice_mount = open_mount(&alice_host, &key).await;

    post_message(
        &alice_host,
        &alice_mount,
        GENERAL_ID,
        "qualification-sentinel-pre-admission",
    )
    .await;
    request(
        &alice_host,
        &alice_mount,
        "pre-admission-channel",
        "conversations.channel.create",
        object([("slug", PackageValue::String("shared-channel".into()))]),
    )
    .await;
    let alice_snapshot = snapshot(&alice_host, &alice_mount, "alice-pre-admission").await;
    let shared_channel = channel_id(&alice_snapshot, "shared-channel").unwrap();

    let invitation = create_invitation(&alice_core, workspace_id).await;
    let bob_core = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(&bob_core_path))
            .await
            .unwrap(),
    );
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);
    let bob_host = open_host(&application, bob_private.clone(), bob_core.clone()).await;
    let bob_mount = open_mount(&bob_host, &key).await;

    assert!(
        wait_until(|| async {
            let bob_snapshot = snapshot(&bob_host, &bob_mount, "bob-admission-history").await;
            let bob_page = channel_page(&bob_host, &bob_mount, GENERAL_ID).await;
            has_channel(&bob_snapshot, "shared-channel")
                && has_message(&bob_page, "qualification-sentinel-pre-admission")
        })
        .await,
        "newly admitted Bob did not receive Alice's existing history"
    );

    stop_profile(alice_host, alice_core).await;
    stop_profile(bob_host, bob_core).await;

    let (alice_core, alice_host, alice_mount) =
        reopen_profile(&application, &alice_core_path, &alice_private, &key).await;
    post_message(
        &alice_host,
        &alice_mount,
        GENERAL_ID,
        "qualification-sentinel-alice-offline",
    )
    .await;
    request(
        &alice_host,
        &alice_mount,
        "alice-offline-channel",
        "conversations.channel.create",
        object([("slug", PackageValue::String("offline-team".into()))]),
    )
    .await;
    stop_profile(alice_host, alice_core).await;

    let (bob_core, bob_host, bob_mount) =
        reopen_profile(&application, &bob_core_path, &bob_private, &key).await;
    post_message(
        &bob_host,
        &bob_mount,
        GENERAL_ID,
        "qualification-sentinel-bob-offline",
    )
    .await;
    request(
        &bob_host,
        &bob_mount,
        "bob-offline-channel",
        "conversations.channel.create",
        object([("slug", PackageValue::String("offline-team".into()))]),
    )
    .await;
    stop_profile(bob_host, bob_core).await;

    let (alice_core, alice_host, alice_mount) =
        reopen_profile(&application, &alice_core_path, &alice_private, &key).await;
    let (bob_core, bob_host, bob_mount) =
        reopen_profile(&application, &bob_core_path, &bob_private, &key).await;

    assert!(
        wait_until_with_deadline(Duration::from_secs(30), || async {
            let alice_messages = message_projection(&alice_host, &alice_mount).await;
            let bob_messages = message_projection(&bob_host, &bob_mount).await;
            let alice_channels =
                channel_projection(&snapshot(&alice_host, &alice_mount, "alice-reconnected").await);
            let bob_channels =
                channel_projection(&snapshot(&bob_host, &bob_mount, "bob-reconnected").await);
            alice_messages.len() == 3
                && alice_messages == bob_messages
                && alice_channels == bob_channels
                && alice_channels
                    .iter()
                    .filter(|(_, slug, _)| slug.starts_with("offline-team"))
                    .count()
                    == 2
        })
        .await,
        "disconnected projections did not reconcile automatically"
    );
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    let bob_projection = message_projection(&bob_host, &bob_mount).await;
    assert_eq!(alice_projection, bob_projection);
    assert_projection_is_duplicate_free(&alice_projection);

    request(
        &alice_host,
        &alice_mount,
        "hide-shared-channel",
        "conversations.channel.hide",
        object([
            ("channelId", PackageValue::String(shared_channel.clone())),
            ("confirm", PackageValue::Boolean(true)),
        ]),
    )
    .await;
    assert!(
        wait_until(|| async {
            channel_is_hidden(
                &snapshot(&bob_host, &bob_mount, "bob-hidden-channel").await,
                &shared_channel,
            )
        })
        .await,
        "Channel hide did not converge"
    );
    assert!(
        request_fails(
            &bob_host,
            &bob_mount,
            "post-hidden-channel",
            "conversations.message.post",
            object([
                ("channelId", PackageValue::String(shared_channel.clone())),
                (
                    "markdown",
                    PackageValue::String("qualification-sentinel-hidden-post".into()),
                ),
            ]),
        )
        .await,
        "post-hide Message was accepted locally"
    );

    let projection_before_malformed = message_projection(&alice_host, &alice_mount).await;
    alice_core
        .append_record(
            workspace_id,
            PACKAGE_ID,
            "message.post",
            b"malformed-qualification-record",
        )
        .unwrap();
    assert!(
        wait_until(|| async {
            snapshot_has_warning(&snapshot(&alice_host, &alice_mount, "alice-warning").await)
                && snapshot_has_warning(&snapshot(&bob_host, &bob_mount, "bob-warning").await)
        })
        .await,
        "malformed record warning did not converge"
    );
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    let bob_projection = message_projection(&bob_host, &bob_mount).await;
    assert_eq!(projection_before_malformed, alice_projection);
    assert_eq!(alice_projection, bob_projection);

    stop_profile(bob_host, bob_core).await;
    let (bob_core, bob_host, bob_mount) =
        reopen_profile(&application, &bob_core_path, &bob_private, &key).await;
    assert_eq!(
        message_projection(&alice_host, &alice_mount).await,
        message_projection(&bob_host, &bob_mount).await,
        "Bob did not resume the persisted projection after restart"
    );
    assert!(snapshot_has_warning(
        &snapshot(&bob_host, &bob_mount, "bob-restart-warning").await
    ));
    post_message(
        &alice_host,
        &alice_mount,
        GENERAL_ID,
        "qualification-sentinel-after-restart",
    )
    .await;
    assert!(
        wait_until(|| async {
            has_message(
                &channel_page(&bob_host, &bob_mount, GENERAL_ID).await,
                "qualification-sentinel-after-restart",
            )
        })
        .await,
        "Bob's resumed cursor did not deliver a later Message"
    );
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    let bob_projection = message_projection(&bob_host, &bob_mount).await;
    assert_eq!(alice_projection, bob_projection);
    assert_projection_is_duplicate_free(&bob_projection);

    stop_profile(alice_host, alice_core).await;
    stop_profile(bob_host, bob_core).await;
    for directory in [alice_core_path, bob_core_path, alice_private, bob_private] {
        assert_no_plaintext_sentinel(&directory);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "ordinary-machine timing qualification; run with npm run verify:conversations"]
async fn ordinary_machine_qualification_meets_delivery_and_catch_up_targets() {
    const QUEUED_MESSAGES: usize = 1_000;
    const REACHABLE_TARGET: Duration = Duration::from_secs(5);
    const CATCH_UP_TARGET: Duration = Duration::from_secs(30);

    eprintln!(
        "conversations qualification environment: os={} arch={} cpus={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(1, usize::from)
    );

    let root = tempfile::tempdir().unwrap();
    let application = stage_package(root.path());
    let alice_core_path = root.path().join("alice-core");
    let bob_core_path = root.path().join("bob-core");
    let alice_private = root.path().join("alice-private");
    let bob_private = root.path().join("bob-private");
    let alice_core = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(&alice_core_path))
            .await
            .unwrap(),
    );
    let workspace = alice_core.create_workspace("Qualification").unwrap();
    let workspace_id = WorkspaceId::new(workspace.id().to_string()).unwrap();
    let key = PackageInstanceKey::new(
        PackageIdentity::from_str(PACKAGE_ID).unwrap(),
        workspace_id.clone(),
    );
    let invitation = create_invitation(&alice_core, workspace.id()).await;
    let bob_core = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(&bob_core_path))
            .await
            .unwrap(),
    );
    bob_core.join_workspace(&invitation).await.unwrap();
    drop(invitation);

    let alice_host = open_host(&application, alice_private.clone(), alice_core.clone()).await;
    let alice_mount = open_mount(&alice_host, &key).await;
    let bob_host = open_host(&application, bob_private.clone(), bob_core.clone()).await;
    let bob_mount = open_mount(&bob_host, &key).await;

    post_message(
        &alice_host,
        &alice_mount,
        GENERAL_ID,
        "qualification-sentinel-timing-seed",
    )
    .await;
    assert!(
        wait_until(|| async {
            unread_count(&snapshot(&bob_host, &bob_mount, "timing-seed").await) == 1
        })
        .await,
        "Bob did not establish the initial synchronized scope"
    );
    stop_profile(bob_host, bob_core).await;

    for index in 0..QUEUED_MESSAGES {
        request(
            &alice_host,
            &alice_mount,
            &format!("qualification-queued-{index}"),
            "conversations.message.post",
            object([
                ("channelId", PackageValue::String(GENERAL_ID.into())),
                (
                    "markdown",
                    PackageValue::String(format!("qualification-sentinel-{index:04}")),
                ),
            ]),
        )
        .await;
    }
    let alice_projection = message_projection(&alice_host, &alice_mount).await;
    assert_eq!(alice_projection.len(), QUEUED_MESSAGES + 1);
    assert_projection_is_duplicate_free(&alice_projection);

    let catch_up_started = Instant::now();
    let (bob_core, bob_host, bob_mount) =
        reopen_profile(&application, &bob_core_path, &bob_private, &key).await;
    let caught_up = wait_until_with_deadline(CATCH_UP_TARGET, || async {
        unread_count(
            &request(
                &bob_host,
                &bob_mount,
                "qualification-bob-snapshot",
                "conversations.snapshot",
                object([]),
            )
            .await,
        ) == QUEUED_MESSAGES + 1
    })
    .await;
    if !caught_up {
        let final_snapshot = request(
            &bob_host,
            &bob_mount,
            "qualification-bob-final-snapshot",
            "conversations.snapshot",
            object([]),
        )
        .await;
        panic!(
            "queued projection did not converge before the qualification deadline: projected_records={}",
            unread_count(&final_snapshot)
        );
    }
    let bob_projection = message_projection(&bob_host, &bob_mount).await;
    assert_eq!(bob_projection, alice_projection);
    assert_projection_is_duplicate_free(&bob_projection);
    let catch_up_elapsed = catch_up_started.elapsed();
    assert!(
        catch_up_elapsed < CATCH_UP_TARGET,
        "queued projection exceeded the qualification deadline"
    );
    eprintln!(
        "conversations qualification timing: queued_records={} catch_up_ms={} target_ms={}",
        QUEUED_MESSAGES,
        catch_up_elapsed.as_millis(),
        CATCH_UP_TARGET.as_millis()
    );

    let reachable_started = Instant::now();
    request(
        &bob_host,
        &bob_mount,
        "qualification-reachable-message",
        "conversations.message.post",
        object([
            ("channelId", PackageValue::String(GENERAL_ID.into())),
            (
                "markdown",
                PackageValue::String("qualification-reachable-sentinel".into()),
            ),
        ]),
    )
    .await;
    let delivered = wait_until_with_deadline(REACHABLE_TARGET, || async {
        has_message(
            &request(
                &alice_host,
                &alice_mount,
                "qualification-alice-page",
                "conversations.channel.page",
                object([
                    ("channelId", PackageValue::String(GENERAL_ID.into())),
                    ("limit", PackageValue::Unsigned(50)),
                ]),
            )
            .await,
            "qualification-reachable-sentinel",
        )
    })
    .await;
    assert!(
        delivered,
        "reachable delivery exceeded the qualification deadline"
    );
    let reachable_elapsed = reachable_started.elapsed();
    assert!(reachable_elapsed < REACHABLE_TARGET);
    eprintln!(
        "conversations qualification timing: reachable_delivery_ms={} target_ms={}",
        reachable_elapsed.as_millis(),
        REACHABLE_TARGET.as_millis()
    );

    shutdown(alice_host).await;
    shutdown(bob_host).await;
    alice_core.shutdown().await.unwrap();
    bob_core.shutdown().await.unwrap();
    drop(alice_core);
    drop(bob_core);

    for directory in [alice_core_path, bob_core_path, alice_private, bob_private] {
        assert_no_plaintext_sentinel(&directory);
    }
    eprintln!("conversations qualification persistence scan: plaintext_sentinel=absent");
}

async fn create_invitation(
    core: &CoreRuntime,
    workspace: teamarness_core::WorkspaceId,
) -> WorkspaceInvitation {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match core.create_workspace_invitation(workspace).await {
            Ok(invitation) => return invitation,
            Err(OnboardingError::Readiness) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => panic!("workspace invitation failed during qualification: {error}"),
        }
    }
}

async fn open_host(application: &Path, private: PathBuf, core: Arc<CoreRuntime>) -> PackageHost {
    fs::create_dir_all(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    let config = PackageHostConfig::new(
        application,
        application.join("application-packages.v1.json"),
        private,
        InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap(),
    );
    PackageHost::open(config, core).await.unwrap()
}

async fn reopen_profile(
    application: &Path,
    core_path: &Path,
    private_path: &Path,
    key: &PackageInstanceKey,
) -> (
    Arc<CoreRuntime>,
    PackageHost,
    teamarness_package_host::UiMountId,
) {
    let core = Arc::new(
        CoreRuntime::open(CoreRuntimeConfig::new(core_path))
            .await
            .unwrap(),
    );
    let host = open_host(application, private_path.to_owned(), core.clone()).await;
    let mount = open_mount(&host, key).await;
    (core, host, mount)
}

async fn stop_profile(host: PackageHost, core: Arc<CoreRuntime>) {
    shutdown(host).await;
    core.shutdown().await.unwrap();
    drop(core);
}

async fn open_mount(
    host: &PackageHost,
    key: &PackageInstanceKey,
) -> teamarness_package_host::UiMountId {
    match host
        .execute(PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: key.clone(),
        }))
        .await
        .unwrap()
    {
        PackageHostOutcome::UiMountOpened { mount, .. } => mount,
        outcome => panic!("unexpected mount outcome: {outcome:?}"),
    }
}

async fn snapshot(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    request_id: &str,
) -> PackageValue {
    request(
        host,
        mount,
        request_id,
        "conversations.snapshot",
        object([]),
    )
    .await
}

async fn channel_page(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    channel_id: &str,
) -> PackageValue {
    request(
        host,
        mount,
        "channel-page",
        "conversations.channel.page",
        object([
            ("channelId", PackageValue::String(channel_id.into())),
            ("limit", PackageValue::Unsigned(32)),
        ]),
    )
    .await
}

async fn post_message(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    channel_id: &str,
    markdown: &str,
) {
    request(
        host,
        mount,
        "post-message",
        "conversations.message.post",
        object([
            ("channelId", PackageValue::String(channel_id.into())),
            ("markdown", PackageValue::String(markdown.into())),
        ]),
    )
    .await;
}

async fn request_fails(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    request_id: &str,
    operation: &str,
    payload: PackageValue,
) -> bool {
    host.execute(PackageHostCommand::UiRequest(UiRequest {
        mount: mount.clone(),
        request: UiRequestId::new(format!(
            "{request_id}-{}",
            NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap(),
        operation: operation.into(),
        payload,
    }))
    .await
    .is_err()
}

async fn request(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
    request_id: &str,
    operation: &str,
    payload: PackageValue,
) -> PackageValue {
    match host
        .execute(PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: UiRequestId::new(format!(
                "{request_id}-{}",
                NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
            ))
            .unwrap(),
            operation: operation.into(),
            payload,
        }))
        .await
        .unwrap_or_else(|error| panic!("request {request_id} ({operation}) failed: {error}"))
    {
        PackageHostOutcome::UiResponse { payload } => payload,
        outcome => panic!("unexpected request outcome: {outcome:?}"),
    }
}

async fn wait_until<F, Fut>(condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    wait_until_with_deadline(Duration::from_secs(15), condition).await
}

async fn wait_until_with_deadline<F, Fut>(timeout: Duration, mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if condition().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn message_projection(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
) -> Vec<(String, String)> {
    let mut before: Option<String> = None;
    let mut projection = Vec::new();
    loop {
        let mut payload = BTreeMap::from([
            (
                "channelId".to_owned(),
                PackageValue::String(GENERAL_ID.into()),
            ),
            ("limit".to_owned(), PackageValue::Unsigned(50)),
        ]);
        if let Some(order_key) = before.as_ref() {
            payload.insert(
                "beforeOrderKey".to_owned(),
                PackageValue::String(order_key.clone()),
            );
        }
        let page = request(
            host,
            mount,
            "qualification-projection-page",
            "conversations.channel.page",
            PackageValue::Object(payload),
        )
        .await;
        let PackageValue::Object(fields) = page else {
            panic!("conversation page must be an object")
        };
        let Some(PackageValue::Array(messages)) = fields.get("messages") else {
            panic!("conversation page must contain messages")
        };
        for message in messages {
            let PackageValue::Object(message) = message else {
                panic!("conversation message must be an object")
            };
            let Some(PackageValue::String(id)) = message.get("id") else {
                panic!("conversation message must contain an id")
            };
            let Some(PackageValue::String(order_key)) = message.get("orderKey") else {
                panic!("conversation message must contain an order key")
            };
            projection.push((id.clone(), order_key.clone()));
        }
        let has_more = matches!(fields.get("hasMore"), Some(PackageValue::Boolean(true)));
        if !has_more {
            break;
        }
        before = projection.last().map(|(_, order_key)| order_key.clone());
        assert!(before.is_some(), "a continued page must contain a message");
    }
    projection
}

fn unread_count(snapshot: &PackageValue) -> usize {
    let PackageValue::Object(fields) = snapshot else {
        return 0;
    };
    let Some(PackageValue::Array(channels)) = fields.get("channels") else {
        return 0;
    };
    channels
        .iter()
        .find_map(|channel| {
            let PackageValue::Object(channel) = channel else {
                return None;
            };
            if channel.get("id") != Some(&PackageValue::String(GENERAL_ID.into())) {
                return None;
            }
            match channel.get("unreadCount") {
                Some(PackageValue::String(value)) => value.parse().ok(),
                Some(PackageValue::Unsigned(value)) => usize::try_from(*value).ok(),
                Some(PackageValue::Integer(value)) => usize::try_from(*value).ok(),
                _ => None,
            }
        })
        .unwrap_or(0)
}

fn assert_projection_is_duplicate_free(projection: &[(String, String)]) {
    let ids = projection
        .iter()
        .map(|(id, _)| id)
        .collect::<std::collections::BTreeSet<_>>();
    let order_keys = projection
        .iter()
        .map(|(_, order_key)| order_key)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), projection.len());
    assert_eq!(order_keys.len(), projection.len());
}

fn assert_no_plaintext_sentinel(path: &Path) {
    let sentinel = b"qualification-sentinel";
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_plaintext_sentinel(&path);
        } else {
            let bytes = fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(sentinel.len())
                    .any(|window| window == sentinel),
                "plaintext qualification sentinel persisted"
            );
        }
    }
}

fn channel_projection(value: &PackageValue) -> Vec<(String, String, bool)> {
    let PackageValue::Object(fields) = value else {
        return Vec::new();
    };
    let Some(PackageValue::Array(channels)) = fields.get("channels") else {
        return Vec::new();
    };
    let mut projection = channels
        .iter()
        .filter_map(|channel| {
            let PackageValue::Object(fields) = channel else {
                return None;
            };
            let PackageValue::String(id) = fields.get("id")? else {
                return None;
            };
            let PackageValue::String(slug) = fields.get("slug")? else {
                return None;
            };
            let PackageValue::Boolean(hidden) = fields.get("hidden")? else {
                return None;
            };
            Some((id.clone(), slug.clone(), *hidden))
        })
        .collect::<Vec<_>>();
    projection.sort();
    projection
}

fn channel_id(value: &PackageValue, slug: &str) -> Option<String> {
    channel_projection(value)
        .into_iter()
        .find_map(|(id, candidate, _)| (candidate == slug).then_some(id))
}

fn channel_is_hidden(value: &PackageValue, channel_id: &str) -> bool {
    channel_projection(value)
        .iter()
        .any(|(id, _, hidden)| id == channel_id && *hidden)
}

fn snapshot_has_warning(value: &PackageValue) -> bool {
    let PackageValue::Object(fields) = value else {
        return false;
    };
    matches!(fields.get("warnings"), Some(PackageValue::Array(warnings)) if !warnings.is_empty())
}

fn has_channel(value: &PackageValue, slug: &str) -> bool {
    let PackageValue::Object(fields) = value else {
        return false;
    };
    let Some(PackageValue::Array(channels)) = fields.get("channels") else {
        return false;
    };
    channels.iter().any(|channel| {
        matches!(channel, PackageValue::Object(fields) if fields.get("slug") == Some(&PackageValue::String(slug.into())))
    })
}

fn has_message(value: &PackageValue, markdown: &str) -> bool {
    let PackageValue::Object(fields) = value else {
        return false;
    };
    let Some(PackageValue::Array(messages)) = fields.get("messages") else {
        return false;
    };
    messages.iter().any(|message| {
        matches!(message, PackageValue::Object(fields) if fields.get("markdown") == Some(&PackageValue::String(markdown.into())))
    })
}

async fn shutdown(host: PackageHost) {
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
}

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

fn object<const N: usize>(entries: [(&str, PackageValue); N]) -> PackageValue {
    PackageValue::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn stage_package(root: &Path) -> PathBuf {
    let application = root.join("application");
    let package = application.join(PACKAGE_ID);
    fs::create_dir_all(package.join("backend")).unwrap();
    fs::create_dir_all(package.join("ui")).unwrap();

    let backend = fs::read(env!("CARGO_BIN_EXE_teamarness-conversations-package")).unwrap();
    let backend_path =
        package.join("backend/teamarness-conversations-package-aarch64-apple-darwin");
    fs::write(&backend_path, &backend).unwrap();
    fs::set_permissions(&backend_path, fs::Permissions::from_mode(0o700)).unwrap();
    let ui = b"export function mount() { return async () => {}; }";
    let css = b"";
    let contract =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui-contract.json")).unwrap();
    fs::write(package.join("ui/index.js"), ui).unwrap();
    fs::write(package.join("ui/style.css"), css).unwrap();
    fs::write(package.join("ui-contract.json"), &contract).unwrap();
    fs::write(
        package.join("teamarness.package.json"),
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion":1,
            "identity":PACKAGE_ID,
            "version":"1.0.0",
            "presentation":{"displayName":"Conversations"},
            "protocol":{"major":1,"minMinor":0,"maxMinor":0},
            "backends":[{"target":"aarch64-apple-darwin","path":"backend/teamarness-conversations-package-aarch64-apple-darwin","size":backend.len(),"sha256":digest(&backend)}],
            "ui":{
                "entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},
                "assets":[{"path":"ui/style.css","mediaType":"text/css","size":css.len(),"sha256":digest(css)}],
                "contract":{"path":"ui-contract.json","sha256":digest(&contract)}
            },
            "capabilities":[
                {"identity":"teamarness.workspace-context","major":1,"minMinor":0,"maxMinor":0},
                {"identity":"teamarness.membership","major":1,"minMinor":0,"maxMinor":0},
                {"identity":"teamarness.signing","major":1,"minMinor":0,"maxMinor":0},
                {"identity":"teamarness.synchronization.immutable-record-log","major":1,"minMinor":0,"maxMinor":0}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        application.join("application-packages.v1.json"),
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion":1,
            "packages":[{"directory":PACKAGE_ID}]
        }))
        .unwrap(),
    )
    .unwrap();
    application
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
