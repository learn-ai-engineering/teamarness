//! Native, source-current compositions. Only Core addressing is replaced by loopback.
#![allow(dead_code)] // Each named target uses a different subset of these shared helpers.
#[path = "../../../support/temp_dir.rs"]
mod tempfile;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use teamarness_core::{CoreRuntime, CoreRuntimeConfig};
use teamarness_headless::{
    application::{Application, ApplicationConfig},
    control::{self, Operation, Request},
};
use teamarness_package_host::{
    CloseUiMount, InstallationPublicIdentity, OpenUiMount, PackageHost, PackageHostCommand,
    PackageHostConfig, PackageHostOutcome, PackageInstanceKey, ShutdownHost, UiRequest,
    UiRequestId,
};

pub const DOCUMENTS: &str = "dev.teamarness.documents";
pub const BACKLOG: &str = "dev.teamarness.backlog";
pub const CONVERSATIONS: &str = "dev.teamarness.conversations";
pub const DRIVER: &str = "org.independent.mesh-driver";
pub const GENERAL: &str = "00000000000000000000000000000000";
pub const TEAM: [&str; 3] = [DOCUMENTS, BACKLOG, CONVERSATIONS];
pub const DEADLINE: Duration = Duration::from_secs(60);

pub fn fixture_root() -> tempfile::TempDir {
    let root = ::tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    assert_eq!(root.path().parent(), Some(std::env::temp_dir().as_path()));
    assert_eq!(
        fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let canonical = root.path().canonicalize().unwrap();
    let socket_bytes = canonical
        .join("a/control.sock")
        .as_os_str()
        .as_encoded_bytes()
        .len();
    let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
    assert!(
        socket_bytes < limit,
        "configured temporary root must fit the real Unix socket limit"
    );
    eprintln!(
        "Headless fixture temporary root={} canonical={} socketBytes={socket_bytes}",
        std::env::temp_dir().display(),
        canonical.display()
    );
    root.into()
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// Real source manifests/contracts/Backend code; UI bytes are inert admission fixtures.
/// No prebuilt application resources, external directories, or Package databases are read.
pub fn stage(root: &Path) -> PathBuf {
    let application = root.join("application");
    let packages = application.join("packages");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (name, backend) in [
        (
            "documents",
            env!("CARGO_BIN_EXE_documents-backend-under-test"),
        ),
        ("backlog", env!("CARGO_BIN_EXE_backlog-backend-under-test")),
        (
            "conversations",
            env!("CARGO_BIN_EXE_conversations-backend-under-test"),
        ),
    ] {
        let source = source.join(format!("packages/{name}-package"));
        let identity = format!("dev.teamarness.{name}");
        let destination = packages.join(&identity);
        fs::create_dir_all(destination.join("backend")).unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(source.join("teamarness.package.json")).unwrap())
                .unwrap();
        let bytes = fs::read(backend).unwrap();
        private_file(&destination.join("backend/native"), &bytes);
        fs::set_permissions(
            destination.join("backend/native"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        manifest["backends"] = json!([{"target":teamarness_package_host::NATIVE_BACKEND_TARGET,"path":"backend/native","size":bytes.len(),"sha256":digest(&bytes)}]);
        {
            let entry = &mut manifest["ui"]["entrypoint"];
            let path = destination.join(entry["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let inert = b"throw new Error('UI_MUST_NOT_EXECUTE');";
            fs::write(path, inert).unwrap();
            entry["size"] = json!(inert.len());
            entry["sha256"] = digest(inert).into();
        }
        for entry in manifest["ui"]["assets"].as_array_mut().unwrap() {
            let path = destination.join(entry["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
            entry["size"] = json!(0);
            entry["sha256"] = digest(b"").into();
        }
        for field in ["ui", "portContract"] {
            let entry = if field == "ui" {
                &mut manifest["ui"]["contract"]
            } else {
                &mut manifest["portContract"]
            };
            let path = entry["path"].as_str().unwrap();
            let bytes = fs::read(source.join(path)).unwrap();
            fs::write(destination.join(path), &bytes).unwrap();
            entry["sha256"] = digest(&bytes).into();
        }
        fs::write(
            destination.join("teamarness.package.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }
    fs::write(packages.join("application-packages.v1.json"), serde_json::to_vec(&json!({"schemaVersion":1,"packages":TEAM.map(|id|json!({"directory":format!("packages/{id}")}))})).unwrap()).unwrap();
    application
}

pub fn no_external_configuration() {
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        assert!(
            !name.starts_with("TEAMARNESS_AGENT_")
                && !name.starts_with("TEAMARNESS_EXTERNAL_GATEWAY_"),
            "external Package configuration is forbidden: {name}"
        );
    }
}

pub struct Desktop {
    pub core: Arc<CoreRuntime>,
    pub host: PackageHost,
}
impl Desktop {
    pub async fn open(application: &Path, data: &Path) -> Self {
        let core = Arc::new(
            CoreRuntime::open_loopback_for_tests(
                CoreRuntimeConfig::new(data).with_exclusive_application_ownership(),
            )
            .await
            .unwrap(),
        );
        Self::with_core(application, data, core).await
    }
    pub async fn with_core(application: &Path, data: &Path, core: Arc<CoreRuntime>) -> Self {
        let host = PackageHost::open(
            PackageHostConfig::new(
                application,
                application.join("packages/application-packages.v1.json"),
                data,
                InstallationPublicIdentity::new(core.public_identity().to_string()).unwrap(),
            ),
            core.clone(),
        )
        .await
        .unwrap();
        assert!(
            host.snapshot().await.diagnostics.is_empty(),
            "admission: {:?}",
            host.snapshot().await.diagnostics
        );
        Self { core, host }
    }
    pub async fn request(
        &self,
        workspace: &str,
        package: &str,
        operation: &str,
        payload: Value,
    ) -> Value {
        self.try_request(workspace, package, operation, payload)
            .await
            .unwrap_or_else(|error| panic!("{operation}: {error}"))
    }
    pub async fn try_request(
        &self,
        workspace: &str,
        package: &str,
        operation: &str,
        payload: Value,
    ) -> Result<Value, String> {
        // A short ordinary Host request lease, closed before any background observation.
        let opened = self
            .host
            .execute(PackageHostCommand::OpenUiMount(OpenUiMount::main(key(
                workspace, package,
            ))))
            .await
            .unwrap();
        let PackageHostOutcome::UiMountOpened { mount, .. } = opened else {
            panic!("mount: {opened:?}")
        };
        let result = self
            .host
            .execute(PackageHostCommand::UiRequest(UiRequest {
                mount: mount.clone(),
                request: UiRequestId::new("desktop-fixture-request").unwrap(),
                operation: operation.into(),
                payload: serde_json::from_value(payload).unwrap(),
            }))
            .await;
        self.host
            .execute(PackageHostCommand::CloseUiMount(CloseUiMount { mount }))
            .await
            .unwrap();
        let PackageHostOutcome::UiResponse { payload } =
            result.map_err(|error| format!("{error:?}"))?
        else {
            panic!("response")
        };
        Ok(serde_json::to_value(payload).unwrap())
    }
    pub async fn stop(self) {
        self.host
            .execute(PackageHostCommand::Shutdown(ShutdownHost))
            .await
            .unwrap();
        self.core.shutdown().await.unwrap();
    }
}
pub fn key(workspace: &str, package: &str) -> PackageInstanceKey {
    PackageInstanceKey::new(
        package.parse().unwrap(),
        teamarness_package_host::WorkspaceId::new(workspace).unwrap(),
    )
}

pub struct Headless {
    pub app: Arc<Application>,
    stop: tokio::sync::oneshot::Sender<tokio::time::Instant>,
    task: tokio::task::JoinHandle<()>,
}
impl Headless {
    pub async fn open(application: &Path, data: &Path, desktop: &CoreRuntime) -> Self {
        let app = Application::open_with(
            ApplicationConfig {
                application_root: application.to_owned(),
                data_directory: data.to_owned(),
            },
            |config| async {
                let core = CoreRuntime::open_loopback_for_tests(config).await?;
                core.remember_loopback_peer_for_tests(desktop);
                desktop.remember_loopback_peer_for_tests(&core);
                Ok(core)
            },
        )
        .await
        .unwrap();
        let app = Arc::new(app);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let serving = app.clone();
        let task = tokio::spawn(async move {
            serving
                .run_until(async { stopped.await.unwrap() })
                .await
                .unwrap();
        });
        let running = Self { app, stop, task };
        tokio::time::timeout(Duration::from_secs(10), async {
            while control::request(
                data,
                Request {
                    major: 1,
                    id: 0,
                    operation: Operation::Status {},
                },
            )
            .await
            .is_err()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("control ready");
        running
    }
    pub async fn call(&self, operation: Operation) -> Value {
        control::request(
            self.app.data_directory(),
            Request {
                major: 1,
                id: 1,
                operation,
            },
        )
        .await
        .unwrap()
        .result
        .unwrap()
    }
    pub async fn cli(&self, arguments: &[&str], input: Option<Value>) -> Value {
        use std::process::Stdio;
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_headless-cli-under-test"))
            .arg("--data-dir")
            .arg(self.app.data_directory())
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&input).unwrap())
                .await
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "CLI {arguments:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    pub async fn join(
        &self,
        desktop: &CoreRuntime,
        workspace: teamarness_core::WorkspaceId,
        name: &str,
    ) {
        let invitation = desktop
            .create_workspace_invitation(workspace)
            .await
            .unwrap();
        let file = self.app.data_directory().join("invitation-input");
        private_file(&file, invitation.encoded().as_bytes());
        let answer = self
            .cli(
                &[
                    "workspace",
                    "join",
                    "--person-name",
                    name,
                    "--invitation-file",
                    file.to_str().unwrap(),
                ],
                None,
            )
            .await;
        fs::remove_file(file).unwrap();
        assert_eq!(answer["membershipCommitted"], true);
    }
    pub async fn request(
        &self,
        workspace: &str,
        package: &str,
        operation: &str,
        payload: Value,
    ) -> Value {
        let schema = self
            .cli(
                &[
                    "package",
                    "schema",
                    "--workspace",
                    workspace,
                    "--package",
                    package,
                    "--operation",
                    operation,
                ],
                None,
            )
            .await;
        self.cli(
            &[
                "package",
                "request",
                "--workspace",
                workspace,
                "--package",
                package,
                "--operation",
                operation,
                "--contract-digest",
                schema["contractDigest"].as_str().unwrap(),
                "--input",
                "-",
            ],
            Some(payload),
        )
        .await
    }
    pub async fn observe(
        &self,
        workspace: &str,
        package: &str,
        operation: &str,
        payload: Value,
    ) -> Result<Value, teamarness_headless::ControlError> {
        let schema = self
            .call(Operation::PackageSchema {
                workspace: workspace.into(),
                package: package.parse().unwrap(),
                operation: operation.into(),
            })
            .await;
        control::request(
            self.app.data_directory(),
            Request {
                major: 1,
                id: 2,
                operation: Operation::PackageRequest {
                    workspace: workspace.into(),
                    package: package.parse().unwrap(),
                    operation: operation.into(),
                    contract_digest: schema["contractDigest"].as_str().unwrap().into(),
                    payload,
                },
            },
        )
        .await
        .unwrap()
        .result
    }
    pub async fn stop(self) {
        self.stop.send(tokio::time::Instant::now()).unwrap();
        self.task.await.unwrap();
    }
}

pub async fn wait_headless(
    node: &Headless,
    workspace: &str,
    package: &str,
    operation: &str,
    payload: Value,
    expected: impl Fn(&Value) -> bool,
) -> Value {
    let mut last = String::new();
    tokio::time::timeout(DEADLINE, async {
        loop {
            match node
                .observe(workspace, package, operation, payload.clone())
                .await
            {
                Ok(value) => {
                    last = value.to_string();
                    if expected(&value) {
                        return value;
                    }
                }
                Err(error) => {
                    assert!(error.outcome_uncertain, "invalid read preflight: {error:?}");
                    last = format!("{error:?}");
                }
            }
            // poll-interval: only read declared Package state, never replay a mutation.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("{package} {operation}: content did not converge; last observation: {last}")
    })
}
pub async fn wait_desktop(
    node: &Desktop,
    workspace: &str,
    package: &str,
    operation: &str,
    payload: Value,
    expected: impl Fn(&Value) -> bool,
) -> Value {
    let mut last = String::new();
    tokio::time::timeout(DEADLINE, async {
        loop {
            match node
                .try_request(workspace, package, operation, payload.clone())
                .await
            {
                Ok(value) => {
                    last = value.to_string();
                    if expected(&value) {
                        return value;
                    }
                }
                Err(error) => {
                    last = error;
                }
            }
            // poll-interval: read-only convergence observation.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("desktop {package} {operation}: content did not converge; last observation: {last}")
    })
}

pub fn contains(value: &Value, field: &str, expected: &str) -> bool {
    match value {
        Value::Object(fields) => {
            fields.get(field) == Some(&json!(expected))
                || fields.values().any(|v| contains(v, field, expected))
        }
        Value::Array(items) => items.iter().any(|v| contains(v, field, expected)),
        _ => false,
    }
}
pub async fn team_generations(node: &Headless) -> Value {
    let status = node.call(Operation::Status {}).await;
    assert!(
        status["host"]["registrations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| !row.to_string().contains("agent") && !row.to_string().contains("gateway"))
    );
    let instances: Vec<_> = status["host"]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| TEAM.contains(&row["key"]["packageIdentity"].as_str().unwrap()))
        .cloned()
        .collect();
    assert_eq!(
        instances.len(),
        6,
        "all three eager Backends in both Workspaces"
    );
    for instance in &instances {
        assert_eq!(instance["lifecycle"], "active", "{instance}");
        assert!(
            instance["mountState"].is_null(),
            "background has no lease: {instance}"
        );
        assert!(instance["processGeneration"].is_number());
    }
    json!(instances)
}
