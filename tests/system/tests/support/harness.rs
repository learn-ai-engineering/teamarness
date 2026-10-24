use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use teamarness_core::{CoreRuntime, OnboardingError, WorkspaceInvitation};
use teamarness_package_host::{
    InstallationPublicIdentity, OpenUiMount, PackageHost, PackageHostCommand, PackageHostConfig,
    PackageHostOutcome, PackageInstanceKey, PackageValue, ShutdownHost, UiRequest, UiRequestId,
};

pub const PACKAGE_ID: &str = "dev.teamarness.conversations";
pub const GENERAL_ID: &str = "00000000000000000000000000000000";
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// A single Profile owns one CoreRuntime, one PackageHost, and one UiMountId
/// for one peer in a qualification scenario.
#[allow(dead_code)]
pub struct Profile {
    pub core: Arc<CoreRuntime>,
    pub host: PackageHost,
    pub mount: teamarness_package_host::UiMountId,
    pub core_path: PathBuf,
    pub private_path: PathBuf,
}

impl Profile {
    #[allow(dead_code)]
    pub fn client(&self) -> ConversationClient<'_> {
        ConversationClient {
            host: &self.host,
            mount: &self.mount,
        }
    }
}

/// Staged application tree with a real Conversations Backend binary,
/// a synthetic ESM entrypoint, and the manifest.
pub struct StagedApplication {
    pub root: tempfile::TempDir,
    pub application_path: PathBuf,
}

impl StagedApplication {
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let application_path = stage_package(root.path());
        Self {
            root,
            application_path,
        }
    }
}

/// Typed request/response client for a mounted Conversations Package.
pub struct ConversationClient<'a> {
    pub host: &'a PackageHost,
    pub mount: &'a teamarness_package_host::UiMountId,
}

impl ConversationClient<'_> {
    pub async fn request(
        &self,
        request_id: &str,
        operation: &str,
        payload: PackageValue,
    ) -> PackageValue {
        let id = format!(
            "{request_id}-{}",
            NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
        );
        match self
            .host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: self.mount.clone(),
                request: UiRequestId::new(id).expect("valid request id"),
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

    pub async fn request_fails(
        &self,
        request_id: &str,
        operation: &str,
        payload: PackageValue,
    ) -> bool {
        let id = format!(
            "{request_id}-{}",
            NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
        );
        self.host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: self.mount.clone(),
                request: UiRequestId::new(id).expect("valid request id"),
                operation: operation.into(),
                payload,
            }))
            .await
            .is_err()
    }

    pub async fn snapshot(&self, request_id: &str) -> PackageValue {
        self.request(request_id, "conversations.snapshot", object([]))
            .await
    }

    pub async fn channel_open(
        &self,
        request_id: &str,
        channel_id: &str,
        limit: u64,
    ) -> PackageValue {
        self.request(
            request_id,
            "conversations.channel.open",
            object([
                ("channelId", PackageValue::String(channel_id.into())),
                ("limit", PackageValue::Unsigned(limit)),
            ]),
        )
        .await
    }

    pub async fn channel_page(
        &self,
        request_id: &str,
        channel_id: &str,
        before: Option<&str>,
        limit: u64,
    ) -> PackageValue {
        let mut payload = BTreeMap::from([
            (
                "channelId".to_owned(),
                PackageValue::String(channel_id.into()),
            ),
            ("limit".to_owned(), PackageValue::Unsigned(limit)),
        ]);
        if let Some(key) = before {
            payload.insert("before".to_owned(), PackageValue::String(key.into()));
        }
        self.request(
            request_id,
            "conversations.channel.page",
            PackageValue::Object(payload),
        )
        .await
    }

    pub async fn post_message(&self, request_id: &str, channel_id: &str, markdown: &str) {
        self.request(
            request_id,
            "conversations.message.post",
            object([
                ("channelId", PackageValue::String(channel_id.into())),
                ("markdown", PackageValue::String(markdown.into())),
            ]),
        )
        .await;
    }

    pub async fn create_channel(&self, request_id: &str, slug: &str) {
        self.request(
            request_id,
            "conversations.channel.create",
            object([("slug", PackageValue::String(slug.into()))]),
        )
        .await;
    }

    pub async fn hide_channel(&self, request_id: &str, channel_id: &str) {
        self.request(
            request_id,
            "conversations.channel.hide",
            object([
                ("channelId", PackageValue::String(channel_id.into())),
                ("confirm", PackageValue::Boolean(true)),
            ]),
        )
        .await;
    }
}

// ------------------------------------------------------------------
// Assertion helpers
// ------------------------------------------------------------------

/// Poll `condition` every 50ms until it returns true or `timeout` expires.
/// Each attempt is bounded by the remaining deadline.
pub async fn poll_until<F, Fut>(timeout: Duration, mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return false;
        }
        if condition().await {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let sleep = Duration::from_millis(50).min(remaining);
        if sleep.is_zero() {
            return false;
        }
        tokio::time::sleep(sleep).await;
    }
}

/// Convenience wrapper with a 15-second default deadline.
pub async fn poll_short<F, Fut>(condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    poll_until(Duration::from_secs(15), condition).await
}

/// Read the full message projection for the General channel by paging through
/// every page.
pub async fn message_projection(
    host: &PackageHost,
    mount: &teamarness_package_host::UiMountId,
) -> Vec<(String, String)> {
    let client = ConversationClient { host, mount };
    let mut before: Option<String> = None;
    let mut projection = Vec::new();

    // Use channel.open for the first page (no before token required).
    let first_page = client
        .channel_open("qualification-projection-first", GENERAL_ID, 50)
        .await;
    let mut more = accumulate_page(&first_page, &mut projection, &mut before);

    while more {
        let page = client
            .channel_page(
                "qualification-projection-page",
                GENERAL_ID,
                before.as_deref(),
                50,
            )
            .await;
        more = accumulate_page(&page, &mut projection, &mut before);
    }
    projection
}

fn accumulate_page(
    page: &PackageValue,
    projection: &mut Vec<(String, String)>,
    before: &mut Option<String>,
) -> bool {
    let PackageValue::Object(fields) = page else {
        panic!("conversation page must be an object")
    };
    let Some(PackageValue::Array(messages)) = fields.get("messages") else {
        panic!("conversation page must contain messages")
    };
    for message in messages {
        let PackageValue::Object(msg) = message else {
            panic!("conversation message must be an object")
        };
        let Some(PackageValue::String(id)) = msg.get("id") else {
            panic!("conversation message must contain an id")
        };
        let order_key = format!("{:082x}", projection.len());
        projection.push((id.clone(), order_key));
    }
    // Use the actual nextBefore token from the response for pagination.
    if let Some(PackageValue::String(token)) = fields.get("nextBefore") {
        *before = Some(token.clone());
        true
    } else {
        false
    }
}

pub fn assert_projection_is_duplicate_free(projection: &[(String, String)]) {
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

pub fn assert_no_plaintext_sentinel(path: &Path) {
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

// ------------------------------------------------------------------
// Extraction helpers for PackageValue snapshots/pages
// ------------------------------------------------------------------

/// The Channels a snapshot shows, as identity and slug.
///
/// `canHide` is deliberately left out: TEAM.SYS.CONV-R06 makes it depend on which Installation
/// is asking, so two profiles that have converged still disagree about it.
pub fn channel_projection(value: &PackageValue) -> Vec<(String, String)> {
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
            Some((id.clone(), slug.clone()))
        })
        .collect::<Vec<_>>();
    projection.sort();
    projection
}

pub fn channel_id(value: &PackageValue, slug: &str) -> Option<String> {
    channel_projection(value)
        .into_iter()
        .find_map(|(id, candidate)| (candidate == slug).then_some(id))
}

pub fn channel_is_hidden(value: &PackageValue, channel_id: &str) -> bool {
    let PackageValue::Object(fields) = value else {
        return false;
    };
    let Some(PackageValue::Array(channels)) = fields.get("channels") else {
        return false;
    };
    !channels.iter().any(|c| {
        matches!(c, PackageValue::Object(f) if f.get("id") == Some(&PackageValue::String(channel_id.into())))
    })
}

pub fn snapshot_has_warning(value: &PackageValue) -> bool {
    let PackageValue::Object(fields) = value else {
        return false;
    };
    matches!(fields.get("warnings"), Some(PackageValue::Array(warnings)) if !warnings.is_empty())
}

pub fn has_channel(value: &PackageValue, slug: &str) -> bool {
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

pub fn has_message_on_page(value: &PackageValue, markdown: &str) -> bool {
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

pub async fn shutdown(host: PackageHost) {
    host.execute(PackageHostCommand::Shutdown(ShutdownHost))
        .await
        .unwrap();
}

pub fn object<const N: usize>(entries: [(&str, PackageValue); N]) -> PackageValue {
    PackageValue::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<BTreeMap<_, _>>(),
    )
}

// ------------------------------------------------------------------
// Host and Core lifecycle helpers
// ------------------------------------------------------------------

/// How the Installations in a scenario find each other.
///
/// Both modes run the same real components — a real `CoreRuntime`, a real Iroh
/// endpoint and Router, real SQLite — and differ only in addressing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Addressing {
    /// Endpoints bound to `127.0.0.1` with no relay and no discovery, so the
    /// scenario is hermetic and belongs in `npm run check`. Peers that have
    /// never dialled each other are introduced with [`introduce`].
    Loopback,
    /// The production binding: `RelayMode::Default`, public relays, relay
    /// addressing. This needs the public internet, so a test that asks for it
    /// is `#[ignore]`d and runs under `npm run verify:conversations`.
    ProductionRelay,
}

/// Opens one Installation on `path` under the scenario's addressing.
pub async fn open_core(addressing: Addressing, path: &Path) -> Arc<CoreRuntime> {
    let config = teamarness_core::CoreRuntimeConfig::new(path);
    let core = match addressing {
        Addressing::Loopback => CoreRuntime::open_loopback_for_tests(config).await,
        // production-addressing: the one seam that binds public relays, for
        // the `#[ignore]`d scenarios that qualify relay-mediated addressing.
        Addressing::ProductionRelay => CoreRuntime::open(config).await,
    };
    Arc::new(core.expect("an Installation opens on its private data directory"))
}

/// Teaches every Installation in `peers` how to reach every other one.
///
/// Loopback endpoints publish no address anywhere, so this stands in for the
/// relay. It is a no-op under [`Addressing::ProductionRelay`], where the relay
/// does the same job. A reopened Installation binds a fresh port, so callers
/// repeat this after every restart.
pub fn introduce(addressing: Addressing, peers: &[&CoreRuntime]) {
    if addressing != Addressing::Loopback {
        return;
    }
    for left in peers {
        for right in peers {
            if left.public_identity() != right.public_identity() {
                left.remember_loopback_peer_for_tests(right);
            }
        }
    }
}

pub async fn create_invitation(
    core: &CoreRuntime,
    workspace: teamarness_core::WorkspaceId,
) -> WorkspaceInvitation {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match core.create_workspace_invitation(workspace).await {
            Ok(invitation) => return invitation,
            Err(OnboardingError::Readiness) if Instant::now() < deadline => {
                // negative-wait: backoff between retries; the endpoint offers no readiness event.
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => panic!("workspace invitation failed during qualification: {error}"),
        }
    }
}

pub async fn open_host(
    application: &Path,
    private: PathBuf,
    core: Arc<CoreRuntime>,
) -> PackageHost {
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

pub async fn open_mount(
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

/// The repository root. `CARGO_MANIFEST_DIR` is `<root>/tests/system`.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}

/// The Conversations Backend executable under test.
///
/// This crate declares the Package's own `src/main.rs` as a bin target (see its
/// `Cargo.toml`), so cargo builds that executable from current sources as a
/// prerequisite of this test binary and `CARGO_BIN_EXE_*` names the result.
/// Resolving a prebuilt artifact instead — `npm run build`'s staged Package
/// Directory — meant a Backend edit was never compiled for these tests, and a
/// Backend that could not even start still passed them (CR-012).
pub fn backend_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_conversations-backend-under-test"))
}

pub fn stage_package(root: &Path) -> PathBuf {
    let application = root.join("application");
    let package = application.join(PACKAGE_ID);
    fs::create_dir_all(package.join("backend")).unwrap();
    fs::create_dir_all(package.join("ui")).unwrap();

    let backend = fs::read(backend_executable()).unwrap();
    let backend_path =
        package.join("backend/teamarness-conversations-package-aarch64-apple-darwin");
    fs::write(&backend_path, &backend).unwrap();
    fs::set_permissions(&backend_path, fs::Permissions::from_mode(0o700)).unwrap();
    let ui = b"export function mount() { return async () => {}; }";
    let css = b"";
    let contract =
        fs::read(workspace_root().join("packages/conversations-package/ui-contract.json")).unwrap();
    fs::write(package.join("ui/index.js"), ui).unwrap();
    fs::write(package.join("ui/style.css"), css).unwrap();
    fs::write(package.join("ui-contract.json"), &contract).unwrap();
    let ports =
        fs::read(workspace_root().join("packages/conversations-package/ports.json")).unwrap();
    fs::write(package.join("ports.json"), &ports).unwrap();
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
            "portContract":{"path":"ports.json","sha256":digest(&ports)},
            "capabilities":[
                {"identity":"teamarness.workspace-context","major":1,"minMinor":0,"maxMinor":0},
                {"identity":"teamarness.membership","major":1,"minMinor":0,"maxMinor":0},
                {"identity":"teamarness.principals","major":1,"minMinor":0,"maxMinor":0},
                {"identity":"teamarness.signing","major":1,"minMinor":0,"maxMinor":0},
                {"identity":"teamarness.synchronization.immutable-record-log","major":1,"minMinor":1,"maxMinor":1}
            ],
            "extensions":{
                "dev.teamarness.build":{
                    "generatedFields":[
                        "backends.0.size","backends.0.sha256",
                        "ui.entrypoint.size","ui.entrypoint.sha256",
                        "ui.assets.0.size","ui.assets.0.sha256",
                        "ui.contract.sha256", "portContract.sha256"
                    ]
                }
            }
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
