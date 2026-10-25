use super::*;
use serde_json::json;

const SIGNATURE: &str = include_str!("../../tests/fixtures/updater/versioned.sig");
const KEY: &str = include_str!("../../tests/fixtures/updater/public-key.txt");
const BYTES: &[u8] = include_bytes!("../../tests/fixtures/updater/artifact.bin");
fn manifest(version: &str) -> serde_json::Value {
    let platform = json!({"url": format!("https://github.com/learn-ai-engineering/teamarness/releases/download/v{version}/Teamarness.app.tar.gz"), "signature": SIGNATURE.trim()});
    json!({"version": version, "notes": "A stable update", "platforms": {"darwin-aarch64": platform, "darwin-aarch64-app": platform}})
}

/// Verifies: TEAM.SYS.SHELL-R07
#[test]
fn update_metadata_requires_newer_canonical_stable_and_exact_repository() {
    let installed = semver::Version::new(1, 0, 0);
    assert!(Metadata::parse(&serde_json::to_vec(&manifest("1.2.3")).unwrap(), &installed).is_ok());
    for version in [
        "1.0.0",
        "0.9.0",
        "1.2.3-beta",
        "1.2.3+build",
        "01.2.3",
        "v1.2.3",
    ] {
        assert!(
            Metadata::parse(&serde_json::to_vec(&manifest(version)).unwrap(), &installed).is_err()
        );
    }
    for url in [
        "http://github.com/learn-ai-engineering/teamarness/releases/download/v1.2.3/x",
        "https://user@github.com/learn-ai-engineering/teamarness/releases/download/v1.2.3/x",
        "https://github.com.evil.test/x",
        "https://github.com/other/teamarness/releases/download/v1.2.3/x",
        "https://github.com/learn-ai-engineering/teamarness/releases/download/v1.2.4/x",
    ] {
        let mut feed = manifest("1.2.3");
        for target in ["darwin-aarch64", "darwin-aarch64-app"] {
            feed["platforms"][target]["url"] = json!(url);
        }
        assert!(Metadata::parse(&serde_json::to_vec(&feed).unwrap(), &installed).is_err());
    }
    assert!(Metadata::parse(b"not JSON", &installed).is_err());
}

use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
struct FixtureTransfer {
    feed: Vec<u8>,
    bytes: Vec<u8>,
    calls: AtomicUsize,
    failure: bool,
    block: Option<Arc<Notify>>,
}
impl Transfer for FixtureTransfer {
    fn fetch(
        &self,
        _url: Url,
        resource: Resource,
        cancel: Cancellation,
        progress: Progress,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(block) = &self.block {
                tokio::select! { _ = cancel.cancelled() => return Err(ErrorCategory::Cancelled), _ = block.notified() => () }
            }
            if self.failure {
                return Err(ErrorCategory::Network);
            }
            match resource {
                Resource::Feed => Ok(Some(self.feed.clone())),
                Resource::Archive => {
                    progress(self.bytes.len());
                    Ok(Some(self.bytes.clone()))
                }
            }
        })
    }
}
fn official() -> Arc<dyn Verifier> {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.package_info_mut().version = Version::new(1, 0, 0);
    context.config_mut().plugins.0.insert(
        "updater".into(),
        json!({"pubkey":KEY.trim(), "requireSignedVersion":true, "endpoints":[config::ENDPOINT]}),
    );
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    Arc::new(OfficialVerifier(
        app.updater_builder()
            .target("darwin-aarch64")
            .build()
            .unwrap(),
    ))
}
fn fixture(
    feed: serde_json::Value,
    bytes: &[u8],
    failure: bool,
    block: Option<Arc<Notify>>,
) -> (UpdateService, Arc<FixtureTransfer>) {
    let (system, transfer) = fixture_system(feed, bytes, failure, block);
    let service = UpdateService::on_runtime(
        Version::new(1, 0, 0),
        Some(system),
        tokio::runtime::Handle::current(),
    );
    (service, transfer)
}
fn fixture_system(
    feed: serde_json::Value,
    bytes: &[u8],
    failure: bool,
    block: Option<Arc<Notify>>,
) -> (System, Arc<FixtureTransfer>) {
    let transfer = Arc::new(FixtureTransfer {
        feed: serde_json::to_vec(&feed).unwrap(),
        bytes: bytes.to_vec(),
        calls: AtomicUsize::new(0),
        failure,
        block,
    });
    (
        System {
            clock: Arc::new(SystemClock),
            transfer: transfer.clone(),
            verifier: official(),
        },
        transfer,
    )
}

async fn wait_state(service: &UpdateService, state: State) {
    // Yield to the owned blocking verifier; a wall deadline reports a missing transition,
    // never causes it. No fixture delay or timing-based race trigger.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if service.status().state == state {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "missing {state:?}: {:?}",
            service.status()
        );
        tokio::task::yield_now().await;
    }
}
async fn wait_calls(transfer: &FixtureTransfer, count: usize) {
    for _ in 0..10000 {
        if transfer.calls.load(Ordering::SeqCst) >= count {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("missing transfer call {count}");
}

// Verifies: TEAM-R10, TEAM.SYS.SHELL-R07. Background discovery never owns ordinary runtime dispatch.
#[tokio::test(start_paused = true)]
async fn update_schedule_launch_six_hours_resume_coalescing_and_explicit_rate_limit() {
    let block = Arc::new(Notify::new());
    let (service, transfer) = fixture(manifest("1.2.3"), BYTES, true, Some(block.clone()));
    let scheduler = tokio::spawn(service.clone().schedule());
    wait_calls(&transfer, 1).await;
    assert_eq!(service.status().state, State::Checking);
    let revision = service.status().revision;
    assert!(!service.request_check());
    assert_eq!(service.status().revision, revision);
    block.notify_one();
    wait_state(&service, State::Idle).await;
    tokio::time::advance(PERIOD - Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(transfer.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    wait_calls(&transfer, 2).await;
    block.notify_one();
    wait_state(&service, State::Idle).await;
    // One resume check, not a queue for four missed periods.
    tokio::time::advance(PERIOD * 4).await;
    wait_calls(&transfer, 3).await;
    assert!(!service.request_check());
    block.notify_one();
    wait_state(&service, State::Idle).await;
    assert!(service.request_check());
    wait_calls(&transfer, 4).await;
    block.notify_one();
    wait_state(&service, State::Idle).await;
    assert!(!service.request_check());
    tokio::time::advance(RETRY).await;
    assert!(service.request_check());
    wait_calls(&transfer, 5).await;
    service.stop();
    scheduler.await.unwrap();
    assert_eq!(service.status().state, State::Stopped);
    assert!(service.inner.lock().unwrap().pending.is_none());
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test(start_paused = true)]
async fn update_real_signed_candidate_ready_suspends_acquisition_and_exit_loses_bytes() {
    let (service, transfer) = fixture(manifest("1.2.3"), BYTES, false, None);
    let scheduler = tokio::spawn(service.clone().schedule());
    wait_state(&service, State::Ready).await;
    let snapshot = service.status();
    assert_eq!(snapshot.candidate_version.as_deref(), Some("1.2.3"));
    assert_eq!(snapshot.downloaded_bytes, BYTES.len());
    assert!(snapshot.pending_generation.is_some());
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(!serialized.contains("https://"));
    assert!(!serialized.contains(SIGNATURE.trim()));
    assert!(!service.request_check());
    tokio::time::advance(PERIOD * 10).await;
    tokio::task::yield_now().await;
    assert_eq!(transfer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(service.status().revision, snapshot.revision);
    assert!(service.admit_apply("stale").is_err());
    service.stop();
    scheduler.await.unwrap();
    assert!(service.inner.lock().unwrap().pending.is_none());
    assert!(service.status().pending_generation.is_none());
    assert!(service.status().revision > snapshot.revision);
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn update_real_verifier_rejects_all_negative_signatures_through_service() {
    let mut altered = BYTES.to_vec();
    altered[0] ^= 1;
    let mut cases = vec![
        (manifest("1.2.3"), altered),
        (manifest("9.9.9"), BYTES.to_vec()),
    ];
    for signature in [
        "bad signature",
        include_str!("../../tests/fixtures/updater/unversioned.sig"),
        include_str!("../../tests/fixtures/updater/changed-comment.sig"),
    ] {
        let mut feed = manifest("1.2.3");
        for target in ["darwin-aarch64", "darwin-aarch64-app"] {
            feed["platforms"][target]["signature"] = json!(signature.trim());
        }
        cases.push((feed, BYTES.to_vec()));
    }
    for (feed, bytes) in cases {
        let (service, _) = fixture(feed, &bytes, false, None);
        assert!(service.request_check());
        wait_state(&service, State::Idle).await;
        assert_eq!(service.status().error, Some(ErrorCategory::Verification));
        assert!(service.status().pending_generation.is_none());
        assert!(service.inner.lock().unwrap().pending.is_none());
        service.stop();
    }
}

struct GatedVerifier {
    entered: Arc<Notify>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    actual: Arc<dyn Verifier>,
}
impl Verifier for GatedVerifier {
    fn verify(&self, metadata: &Metadata, bytes: &[u8]) -> Result<Update, ErrorCategory> {
        self.entered.notify_one();
        self.release.lock().unwrap().recv().unwrap();
        self.actual.verify(metadata, bytes)
    }
}
/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn update_progress_and_late_successful_verification_cannot_publish_ready_after_quit() {
    let entered = Arc::new(Notify::new());
    let (send, recv) = std::sync::mpsc::channel();
    let (mut service, transfer) = fixture(manifest("1.2.3"), BYTES, false, None);
    let system = Arc::get_mut(service.system.as_mut().unwrap()).unwrap();
    system.verifier = Arc::new(GatedVerifier {
        entered: entered.clone(),
        release: Mutex::new(recv),
        actual: official(),
    });
    assert!(service.request_check());
    entered.notified().await;
    assert_eq!(transfer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(service.status().downloaded_bytes, BYTES.len());
    assert_eq!(service.status().state, State::Verifying);
    assert!(service.status().pending_generation.is_none());
    service.stop();
    let revision = service.status().revision;
    send.send(()).unwrap();
    // Join observable acquisition completion, not a sleep after releasing the barrier.
    loop {
        if !service.inner.lock().unwrap().busy {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(service.status().revision, revision);
    assert_eq!(service.status().state, State::Stopped);
    assert!(service.inner.lock().unwrap().pending.is_none());
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn update_apply_admission_only_transfers_same_verified_candidate_once() {
    let (service, _) = fixture(manifest("1.2.3"), BYTES, false, None);
    assert!(service.request_check());
    wait_state(&service, State::Ready).await;
    let generation = service.status().pending_generation.unwrap();
    let pending = service.admit_apply(&generation).unwrap();
    assert_eq!(pending.bytes, BYTES);
    assert_eq!(pending.update.version, "1.2.3");
    assert!(service.admit_apply(&generation).is_err());
    assert!(!service.request_check());
}

struct DiscoveryResult(Result<Option<Vec<u8>>, ErrorCategory>);
impl Transfer for DiscoveryResult {
    fn fetch(
        &self,
        _url: Url,
        resource: Resource,
        _cancel: Cancellation,
        _progress: Progress,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>> {
        Box::pin(async move {
            assert!(
                matches!(resource, Resource::Feed),
                "invalid discovery cannot start an archive transfer"
            );
            self.0.clone()
        })
    }
}
/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn update_invalid_discovery_never_downloads_or_marks_ready() {
    let mut cases = vec![
        Ok(None),
        Err(ErrorCategory::Network),
        Ok(Some(b"invalid JSON".to_vec())),
    ];
    for version in [
        "1.0.0",
        "0.9.0",
        "1.2.3-beta",
        "1.2.3+build",
        "01.2.3",
        "v1.2.3",
    ] {
        cases.push(Ok(Some(serde_json::to_vec(&manifest(version)).unwrap())));
    }
    for mutate in 0..5 {
        let mut feed = manifest("1.2.3");
        match mutate {
            0 => feed["notes"] = json!("<script>not plain text</script>"),
            1 => feed["notes"] = json!("x".repeat(4097)),
            2 => feed["platforms"]["darwin-aarch64-app"]["signature"] = json!("different"),
            3 => feed["platforms"]["darwin-aarch64"]["signature"] = json!("x".repeat(4097)),
            _ => feed["platforms"] = json!({}),
        }
        cases.push(Ok(Some(serde_json::to_vec(&feed).unwrap())));
    }
    for result in cases {
        let first_release = matches!(&result, Ok(None));
        let service = UpdateService::new(
            Version::new(1, 0, 0),
            Some(System {
                clock: Arc::new(SystemClock),
                transfer: Arc::new(DiscoveryResult(result)),
                verifier: official(),
            }),
        );
        assert!(service.request_check());
        wait_state(&service, State::Idle).await;
        assert_eq!(service.status().error.is_none(), first_release);
        assert!(service.status().pending_generation.is_none());
        service.stop();
    }
}
struct LateArchive {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
impl Transfer for LateArchive {
    fn fetch(
        &self,
        _url: Url,
        resource: Resource,
        _cancel: Cancellation,
        progress: Progress,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, ErrorCategory>> {
        Box::pin(async move {
            match resource {
                Resource::Feed => Ok(Some(serde_json::to_vec(&manifest("1.2.3")).unwrap())),
                Resource::Archive => {
                    self.entered.notify_one();
                    self.release.notified().await;
                    progress(BYTES.len());
                    Ok(Some(BYTES.to_vec()))
                }
            }
        })
    }
}
/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn update_quit_fences_late_transfer_progress_and_candidate() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = UpdateService::new(
        Version::new(1, 0, 0),
        Some(System {
            clock: Arc::new(SystemClock),
            transfer: Arc::new(LateArchive {
                entered: entered.clone(),
                release: release.clone(),
            }),
            verifier: official(),
        }),
    );
    assert!(service.request_check());
    entered.notified().await;
    assert_eq!(service.status().state, State::Downloading);
    service.stop();
    let revision = service.status().revision;
    release.notify_one();
    loop {
        if !service.inner.lock().unwrap().busy {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(service.status().revision, revision);
    assert_eq!(service.status().state, State::Stopped);
    assert!(service.inner.lock().unwrap().pending.is_none());
}
/// Verifies: TEAM.SYS.SHELL-R07
#[test]
fn update_debug_environment_cannot_enable_production() {
    if std::env::var_os("TEAMARNESS_TEST_UPDATE_ENV_PROBE").is_some() {
        assert!(!config::enabled());
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "update::tests::update_debug_environment_cannot_enable_production",
        ])
        .env("TEAMARNESS_TEST_UPDATE_ENV_PROBE", "1")
        .env("TEAMARNESS_PRODUCTION_RELEASE", "true")
        .env("TEAMARNESS_UPDATE_PUBLIC_KEY", KEY)
        .env("TEAMARNESS_UPDATE_ENDPOINT", "https://evil.test/feed")
        .env("CARGO_FEATURE_PRODUCTION_RELEASE", "1")
        .env(
            "TAURI_CONFIG",
            json!({"plugins":{"updater":{"pubkey":KEY,"endpoints":[config::ENDPOINT]}}})
                .to_string(),
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Verifies: TEAM.SYS.SHELL-R07
#[test]
fn update_default_application_initializes_without_updater_configuration() {
    // The ordinary bundle has no updater key/configuration. Merely registering
    // the official plugin must not make Disabled startup require one.
    let app = configure(tauri::test::mock_builder())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let service = UpdateService::start(app.handle());
    assert_eq!(service.status().state, State::Disabled);
    assert!(!service.request_check());
}

/// Verifies: TEAM.SYS.SHELL-R07
#[tokio::test]
async fn update_apply_rechecks_pending_version_and_lifecycle_quit_cancels_download() {
    let (service, _) = fixture(manifest("1.2.3"), BYTES, false, None);
    service.request_check();
    wait_state(&service, State::Ready).await;
    let generation = service.status().pending_generation.unwrap();
    service.inner.lock().unwrap().snapshot.installed_version = "1.2.3".into();
    assert!(service.admit_apply(&generation).is_err());
    assert_eq!(service.status().state, State::Ready);
    service.stop();

    let gate = Arc::new(Notify::new());
    let (service, transfer) = fixture(manifest("1.2.3"), BYTES, false, Some(gate.clone()));
    service.request_check();
    while transfer.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    gate.notify_one();
    wait_state(&service, State::Downloading).await;
    struct Quit(UpdateService);
    impl crate::lifecycle::TerminalAdapter for Quit {
        fn stop_updates(&self) {
            self.0.stop();
        }
        fn cleanup(&self) -> BoxFuture<'_, crate::lifecycle::ShutdownErrors> {
            Box::pin(async { Default::default() })
        }
        fn install(
            &self,
            _: crate::lifecycle::Install,
        ) -> Result<(), crate::lifecycle::InstallFailure> {
            panic!("Quit cannot install")
        }
        fn restart(&self) {
            panic!("Quit cannot restart")
        }
        fn exit(&self) {}
    }
    let lifecycle = crate::lifecycle::Lifecycle::new();
    lifecycle.quit(Arc::new(Quit(service.clone())));
    lifecycle.finished().await;
    assert_eq!(service.status().state, State::Stopped);
    assert!(service.inner.lock().unwrap().pending.is_none());
}

pub(super) async fn ready_for_commands() -> UpdateService {
    let (service, _) = fixture(manifest("1.2.3"), BYTES, false, None);
    service.request_check();
    wait_state(&service, State::Ready).await;
    service
}

/// Verifies: TEAM.SYS.SHELL-R07
#[test]
fn native_enabled_startup_and_check_after_returned_block_on_launch_acquisition() {
    tauri::async_runtime::block_on(async {});
    assert!(tokio::runtime::Handle::try_current().is_err());
    // Same enabled scheduler launch used by start(), without installed-bundle or network access.
    let (system, transfer) = fixture_system(manifest("1.2.3"), BYTES, true, None);
    let service = UpdateService::new(Version::new(1, 0, 0), Some(system));
    service.start_scheduler();
    // Idle is also the initial state: wait for the launch publication, not just that state.
    tauri::async_runtime::block_on(async {
        wait_calls(&transfer, 1).await;
        wait_state(&service, State::Idle).await;
    });
    assert!(tokio::runtime::Handle::try_current().is_err());
    assert!(service.request_check());
    assert!(!service.request_check());
    tauri::async_runtime::block_on(async {
        wait_calls(&transfer, 2).await;
        wait_state(&service, State::Idle).await;
    });
    assert_eq!(transfer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(service.status().error, Some(ErrorCategory::Network));
    service.stop();
    assert!(!service.request_check());
}

/// Verifies: TEAM.SYS.SHELL-R07
#[test]
fn native_explicit_check_after_returned_block_on_reaches_verified_ready() {
    tauri::async_runtime::block_on(async {});
    let (system, _) = fixture_system(manifest("1.2.3"), BYTES, false, None);
    let service = UpdateService::new(Version::new(1, 0, 0), Some(system));
    assert!(tokio::runtime::Handle::try_current().is_err());
    assert!(service.request_check());
    assert!(!service.request_check());
    tauri::async_runtime::block_on(async { wait_state(&service, State::Ready).await });
    let generation = service.status().pending_generation.unwrap();
    assert!(service.admit_apply("stale").is_err());
    let pending = service.admit_apply(&generation).unwrap();
    assert_eq!(pending.bytes, BYTES);
    assert_eq!(pending.update.version, "1.2.3");
    assert!(service.admit_apply(&generation).is_err());
    assert!(!service.request_check());
}
