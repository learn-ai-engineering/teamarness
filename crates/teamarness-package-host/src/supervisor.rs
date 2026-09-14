use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::capability::{CapabilityRequirement, CapabilityScope, GenerationCapabilities};
use crate::host_store::{HostStore, ProcessLease};
use crate::manifest::sha256;
use crate::protocol::{self, ProcessClient, WireFailure};
use crate::{
    InstallationPublicIdentity, PackageErrorCode, PackageIdentity, PackageValue, WorkspaceId,
};

pub(crate) const RESTART_DELAYS: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(4),
];
pub(crate) const CRASH_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct ProcessTemplate {
    pub(crate) package_identity: PackageIdentity,
    pub(crate) package_version: String,
    pub(crate) manifest_digest: String,
    pub(crate) backend_digest: String,
    pub(crate) ui_contract_digest: String,
    pub(crate) backend_path: PathBuf,
    pub(crate) private_data_directory: PathBuf,
    pub(crate) source_label: String,
    pub(crate) trust_label: String,
    pub(crate) protocol: (u16, u16),
    pub(crate) capabilities: Vec<CapabilityRequirement>,
}

#[derive(Clone)]
pub(crate) struct ProcessSpec {
    pub(crate) package_identity: PackageIdentity,
    pub(crate) package_version: String,
    pub(crate) manifest_digest: String,
    pub(crate) backend_digest: String,
    pub(crate) ui_contract_digest: String,
    pub(crate) backend_path: PathBuf,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) workspace_name: String,
    pub(crate) installation: InstallationPublicIdentity,
    pub(crate) data_directory: PathBuf,
    pub(crate) continuity_id: String,
    pub(crate) private_data_directory: PathBuf,
    pub(crate) source_label: String,
    pub(crate) trust_label: String,
    pub(crate) protocol: (u16, u16),
    pub(crate) capabilities: Vec<CapabilityRequirement>,
    pub(crate) core: Arc<teamarness_core::CoreRuntime>,
}

pub(crate) struct ProcessGeneration {
    pub(crate) generation: u64,
    pub(crate) client: ProcessClient,
    child: Arc<Mutex<Child>>,
    package: PackageIdentity,
    workspace: WorkspaceId,
    private_data_directory: PathBuf,
    health: tokio::task::JoinHandle<()>,
}

pub(crate) async fn spawn(
    spec: &ProcessSpec,
    generation: u64,
    recovering: bool,
) -> Result<ProcessGeneration, WireFailure> {
    let capabilities = GenerationCapabilities::bind(
        spec.core.clone(),
        CapabilityScope {
            package_identity: spec.package_identity.clone(),
            workspace_id: spec.workspace_id.clone(),
            workspace_name: spec.workspace_name.clone(),
            installation: spec.installation.clone(),
        },
        &spec.capabilities,
        generation,
    )
    .map_err(WireFailure::new)?;
    let current = fs::read(&spec.backend_path)
        .map_err(|_| WireFailure::new(PackageErrorCode::ArtifactChanged))?;
    if sha256(&current) != spec.backend_digest {
        return Err(WireFailure::new(PackageErrorCode::ArtifactChanged));
    }
    let executable = fs::canonicalize(&spec.backend_path)
        .map_err(|_| WireFailure::new(PackageErrorCode::SpawnFailed))?;
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(|_| WireFailure::new(PackageErrorCode::SpawnFailed))?;
    let launch_token = URL_SAFE_NO_PAD.encode(random);
    let mut command = Command::new(&executable);
    command
        .arg(format!("--teamarness-launch-token={launch_token}"))
        .arg("--teamarness-protocol=1")
        .current_dir(&spec.data_directory)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| WireFailure::new(PackageErrorCode::SpawnFailed))?;
    let pid = child
        .id()
        .ok_or_else(|| WireFailure::new(PackageErrorCode::SpawnFailed))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| WireFailure::new(PackageErrorCode::SpawnFailed))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WireFailure::new(PackageErrorCode::SpawnFailed))?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(capture_stderr(
            stderr,
            spec.private_data_directory.clone(),
            spec.package_identity.to_string(),
            spec.workspace_id.to_string(),
            spec.source_label.clone(),
            spec.trust_label.clone(),
            generation,
        ));
    }
    let start_time = observed_start_time(pid)
        .await
        .unwrap_or_else(|| "unknown".into());
    let lease = ProcessLease {
        package_identity: spec.package_identity.to_string(),
        workspace_id: spec.workspace_id.to_string(),
        process_generation: generation,
        pid,
        process_group: i32::try_from(pid)
            .map_err(|_| WireFailure::new(PackageErrorCode::SpawnFailed))?,
        start_time,
        canonical_executable: executable.to_string_lossy().into_owned(),
        executable_digest: spec.backend_digest.clone(),
        launch_token: launch_token.clone(),
    };
    HostStore::open(&spec.private_data_directory)
        .and_then(|store| store.record_process_lease(&lease))
        .map_err(|_| WireFailure::new(PackageErrorCode::DataDirectoryUnavailable))?;

    let client = protocol::start_with_capabilities(stdout, stdin, capabilities.clone());
    let initialize = initialize_frame(spec, generation, &launch_token, &capabilities);
    let initialized = client.initialize(initialize).await?;
    if !handshake_matches(&initialized, spec, generation, &launch_token) {
        return Err(WireFailure::new(PackageErrorCode::HandshakeMismatch));
    }
    let operation = if recovering {
        "lifecycle.recover"
    } else {
        "lifecycle.activate"
    };
    client
        .request(
            format!("h:{operation}-{generation}"),
            operation,
            None,
            PackageValue::Object(Default::default()),
            Duration::from_secs(30),
            true,
        )
        .await?;
    let child = Arc::new(Mutex::new(child));
    let health_client = client.clone();
    let health_child = child.clone();
    let health = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if health_client.health_probe().await.is_err() {
                terminate_group(health_child.lock().await.id(), false);
                return;
            }
        }
    });
    Ok(ProcessGeneration {
        generation,
        client,
        child,
        package: spec.package_identity.clone(),
        workspace: spec.workspace_id.clone(),
        private_data_directory: spec.private_data_directory.clone(),
        health,
    })
}

fn initialize_frame(
    spec: &ProcessSpec,
    generation: u64,
    nonce: &str,
    bound_capabilities: &GenerationCapabilities,
) -> Value {
    let capabilities = bound_capabilities
        .advertised()
        .iter()
        .map(|capability| {
            json!({
                "grant":capability.grant,
                "identity":capability.identity,
                "major":capability.major,
                "minor":capability.minor
            })
        })
        .collect::<Vec<_>>();
    json!({
        "kind":"initialize",
        "nonce":nonce,
        "package":{
            "identity":spec.package_identity.as_str(),
            "version":spec.package_version,
            "manifestDigest":spec.manifest_digest,
            "backendDigest":spec.backend_digest,
        },
        "uiContractDigest":spec.ui_contract_digest,
        "processGeneration":generation.to_string(),
        "workspace":{"id":spec.workspace_id.as_str(),"name":spec.workspace_name},
        "installation":{"identity":spec.installation.as_str(),"displayName":spec.installation.as_str()},
        "dataDirectory":{"path":spec.data_directory.to_string_lossy(),"continuityId":spec.continuity_id},
        "protocol":{"major":spec.protocol.0,"minor":spec.protocol.1},
        "capabilities":capabilities,
        "limits":{
            "maxFrameBytes":1048576,
            "maxInFlightEachDirection":32,
            "maxWriterFrames":64,
            "maxWriterBytes":8388608,
            "initialStreamCredit":32
        }
    })
}

fn handshake_matches(
    frame: &teamarness_package_protocol::Frame,
    spec: &ProcessSpec,
    generation: u64,
    nonce: &str,
) -> bool {
    frame.kind() == teamarness_package_protocol::FrameKind::Initialized
        && frame.field("nonce").and_then(Value::as_str) == Some(nonce)
        && frame.field("processGeneration").and_then(Value::as_str)
            == Some(generation.to_string().as_str())
        && frame.field("uiContractDigest").and_then(Value::as_str)
            == Some(spec.ui_contract_digest.as_str())
        && frame
            .field("package")
            .and_then(Value::as_object)
            .and_then(|package| package.get("identity"))
            .and_then(Value::as_str)
            == Some(spec.package_identity.as_str())
}

impl ProcessGeneration {
    pub(crate) async fn shutdown(&self) {
        self.health.abort();
        self.client.shutdown().await;
        if wait_for_exit(&self.child, Duration::from_secs(2)).await {
            self.remove_lease();
            return;
        }
        terminate_group(self.child.lock().await.id(), false);
        if !wait_for_exit(&self.child, Duration::from_secs(2)).await {
            terminate_group(self.child.lock().await.id(), true);
            let _ = self.child.lock().await.wait().await;
        }
        self.remove_lease();
    }

    pub(crate) async fn terminate(&self) {
        self.health.abort();
        terminate_group(self.child.lock().await.id(), false);
        if !wait_for_exit(&self.child, Duration::from_secs(2)).await {
            terminate_group(self.child.lock().await.id(), true);
            let _ = self.child.lock().await.wait().await;
        }
        self.remove_lease();
    }

    fn remove_lease(&self) {
        if let Ok(store) = HostStore::open(&self.private_data_directory) {
            let _ = store.delete_process_lease(self.package.as_str(), self.workspace.as_str());
        }
    }
}

async fn wait_for_exit(child: &Mutex<Child>, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if child.lock().await.try_wait().ok().flatten().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(unix)]
fn terminate_group(pid: Option<u32>, force: bool) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        let _ = killpg(
            Pid::from_raw(pid),
            if force {
                Signal::SIGKILL
            } else {
                Signal::SIGTERM
            },
        );
    }
}

async fn capture_stderr(
    stderr: tokio::process::ChildStderr,
    private: PathBuf,
    package: String,
    workspace: String,
    source: String,
    trust: String,
    generation: u64,
) {
    let directory = private.join("packages/logs");
    if tokio::fs::create_dir_all(&directory).await.is_err() {
        return;
    }
    let stem = format!("{package}-{workspace}-{source}-{trust}-g{generation}");
    let path = directory.join(format!("{stem}.log"));
    let mut reader = BufReader::new(stderr);
    let mut window = Instant::now();
    let mut accepted = 0_u16;
    loop {
        let mut line = Vec::with_capacity(16 * 1024 + 1);
        let read = match (&mut reader)
            .take(16 * 1024 + 1)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(read) => read,
            Err(_) => return,
        };
        if read == 0 {
            return;
        }
        if !line.ends_with(b"\n") {
            loop {
                let (consumed, complete) = match reader.fill_buf().await {
                    Ok([]) => (0, true),
                    Ok(bytes) => match bytes.iter().position(|byte| *byte == b'\n') {
                        Some(index) => (index + 1, true),
                        None => (bytes.len(), false),
                    },
                    Err(_) => return,
                };
                reader.consume(consumed);
                if complete {
                    break;
                }
            }
        }
        line.truncate(line.len().min(16 * 1024));
        while line
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            line.pop();
        }
        if window.elapsed() >= Duration::from_secs(1) {
            window = Instant::now();
            accepted = 0;
        }
        if accepted >= 100 {
            continue;
        }
        accepted += 1;
        if rotate_logs(&path).is_err() {
            return;
        }
        use std::io::Write as _;
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "{}", String::from_utf8_lossy(&line));
        }
    }
}

fn rotate_logs(path: &Path) -> std::io::Result<()> {
    if fs::metadata(path).map_or(0, |metadata| metadata.len()) < 1024 * 1024 {
        return Ok(());
    }
    for index in (1..4).rev() {
        let older = path.with_extension(format!("log.{index}"));
        let newer = path.with_extension(format!("log.{}", index + 1));
        if older.exists() {
            fs::rename(older, newer)?;
        }
    }
    fs::rename(path, path.with_extension("log.1"))
}

#[cfg(target_os = "macos")]
async fn observed_start_time(pid: u32) -> Option<String> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut system = System::new_all();
    loop {
        system.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
        if let Some(process) = system.process(Pid::from_u32(pid)) {
            return Some(process.start_time().to_string());
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(not(target_os = "macos"))]
async fn observed_start_time(_pid: u32) -> Option<String> {
    Some("unsupported-target".into())
}

pub(crate) fn reconcile_orphans(private: &Path) -> Result<Vec<ProcessLease>, PackageErrorCode> {
    let store = HostStore::open(private).map_err(|_| PackageErrorCode::DataDirectoryUnavailable)?;
    let leases = store
        .process_leases()
        .map_err(|_| PackageErrorCode::DataDirectoryUnavailable)?;
    let mut unresolved = Vec::new();
    for lease in leases {
        let evidence = inspect_lease(&lease);
        match evidence {
            LeaseEvidence::Gone | LeaseEvidence::Exact => {
                if matches!(evidence, LeaseEvidence::Exact) {
                    terminate_group(Some(lease.pid), false);
                    std::thread::sleep(Duration::from_millis(100));
                    terminate_group(Some(lease.pid), true);
                }
                store
                    .delete_process_lease(&lease.package_identity, &lease.workspace_id)
                    .map_err(|_| PackageErrorCode::DataDirectoryUnavailable)?;
            }
            LeaseEvidence::Ambiguous => unresolved.push(lease),
        }
    }
    Ok(unresolved)
}

#[derive(Clone, Copy)]
enum LeaseEvidence {
    Gone,
    Exact,
    Ambiguous,
}

#[cfg(target_os = "macos")]
fn inspect_lease(lease: &ProcessLease) -> LeaseEvidence {
    use nix::unistd::{Pid as NixPid, getpgid};
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut system = System::new_all();
    let pid = Pid::from_u32(lease.pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let Some(process) = system.process(pid) else {
        return LeaseEvidence::Gone;
    };
    let token = format!("--teamarness-launch-token={}", lease.launch_token);
    let executable = process.exe().and_then(|path| fs::canonicalize(path).ok());
    let expected = fs::canonicalize(&lease.canonical_executable).ok();
    let digest_matches = executable
        .as_deref()
        .and_then(|path| fs::read(path).ok())
        .is_some_and(|bytes| sha256(&bytes) == lease.executable_digest);
    let group_matches = i32::try_from(lease.pid)
        .ok()
        .and_then(|pid| getpgid(Some(NixPid::from_raw(pid))).ok())
        .is_some_and(|group| group.as_raw() == lease.process_group);
    if process.start_time().to_string() == lease.start_time
        && executable == expected
        && digest_matches
        && group_matches
        && process.cmd().iter().any(|argument| {
            argument
                .to_string_lossy()
                .lines()
                .next()
                .is_some_and(|value| value == token)
        })
    {
        LeaseEvidence::Exact
    } else {
        LeaseEvidence::Ambiguous
    }
}

#[cfg(not(target_os = "macos"))]
fn inspect_lease(lease: &ProcessLease) -> LeaseEvidence {
    if Path::new(&format!("/proc/{}", lease.pid)).exists() {
        LeaseEvidence::Ambiguous
    } else {
        LeaseEvidence::Gone
    }
}
