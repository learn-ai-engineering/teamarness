use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use teamarness_package_protocol::{
    INITIAL_STREAM_CREDIT, MAX_FRAME_BYTES, MAX_IN_FLIGHT_EACH_DIRECTION, MAX_WRITER_BYTES,
    MAX_WRITER_FRAMES,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::capability::{
    AppendFence, CapabilityRequirement, CapabilityScope, GenerationCapabilities, GenerationGuards,
};
use crate::diag::fault;
use crate::host_store::{HostStore, HostStoreHandle, ProcessLease};
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
    pub(crate) activation: teamarness_package_contract::manifest::ActivationPolicy,
}

/// The Host-wide call environment every Package process shares: one blocking
/// permit pool and one set of protocol bounds.
#[derive(Clone)]
pub(crate) struct CallEnvironment {
    pub(crate) limiter: crate::capability::CoreCallLimiter,
    pub(crate) limits: crate::protocol::ProtocolLimits,
}

#[derive(Clone)]
pub(crate) struct ProcessSpec {
    pub(crate) authority: crate::authority::AuthorityGeneration,
    pub(crate) template: ProcessTemplate,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) workspace_name: String,
    pub(crate) installation: InstallationPublicIdentity,
    pub(crate) data_directory: PathBuf,
    pub(crate) continuity_id: String,
    pub(crate) core: Arc<teamarness_core::CoreRuntime>,
    pub(crate) calls: CallEnvironment,
    pub(crate) store: HostStoreHandle,
    pub(crate) broker: std::sync::Weak<crate::broker::PortBroker>,
}

pub(crate) struct ProcessGeneration {
    pub(crate) generation: u64,
    pub(crate) client: ProcessClient,
    pub(crate) port_routes: Arc<tokio::sync::Semaphore>,
    child: Arc<Mutex<Child>>,
    process_group: i32,
    package: PackageIdentity,
    workspace: WorkspaceId,
    store: HostStoreHandle,
    health: tokio::task::JoinHandle<()>,
}

pub(crate) async fn spawn(
    spec: &ProcessSpec,
    generation: u64,
    recovering: bool,
    append_fence: Arc<AppendFence>,
) -> Result<ProcessGeneration, WireFailure> {
    let authority = spec.authority.advance().await.map_err(WireFailure::new)?;
    let capabilities = GenerationCapabilities::bind(
        spec.core.clone(),
        CapabilityScope {
            package_identity: spec.template.package_identity.clone(),
            workspace_id: spec.workspace_id.clone(),
            workspace_name: spec.workspace_name.clone(),
            installation: spec.installation.clone(),
        },
        &spec.template.capabilities,
        generation,
        spec.calls.limiter.clone(),
        GenerationGuards {
            append_fence,
            authority,
        },
        spec.broker.clone(),
    )
    .map_err(WireFailure::new)?;
    // Reading and hashing the Backend executable is blocking file work.
    let backend_path = spec.template.backend_path.clone();
    let expected_digest = spec.template.backend_digest.clone();
    let executable = tokio::task::spawn_blocking(move || {
        let current = fs::read(&backend_path)
            .map_err(fault!(WireFailure::new(PackageErrorCode::ArtifactChanged)))?;
        if sha256(&current) != expected_digest {
            return Err(WireFailure::new(PackageErrorCode::ArtifactChanged));
        }
        fs::canonicalize(&backend_path)
            .map_err(fault!(WireFailure::new(PackageErrorCode::SpawnFailed)))
    })
    .await
    .map_err(fault!(WireFailure::new(PackageErrorCode::SpawnFailed)))??;
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random)
        .map_err(fault!(WireFailure::new(PackageErrorCode::SpawnFailed)))?;
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
    let child = match command.spawn() {
        Ok(child) => Arc::new(Mutex::new(child)),
        Err(_) => return Err(WireFailure::new(PackageErrorCode::SpawnFailed)),
    };
    let pid = match child.lock().await.id() {
        Some(pid) => pid,
        None => return Err(WireFailure::new(PackageErrorCode::SpawnFailed)),
    };
    let process_group = match i32::try_from(pid) {
        Ok(process_group) => process_group,
        Err(_) => return Err(WireFailure::new(PackageErrorCode::SpawnFailed)),
    };
    let stdin = {
        let mut child = child.lock().await;
        child.stdin.take()
    };
    let Some(stdin) = stdin else {
        cleanup_failed_spawn(&child, spec, process_group, None).await;
        return Err(WireFailure::new(PackageErrorCode::SpawnFailed));
    };
    let stdout = {
        let mut child = child.lock().await;
        child.stdout.take()
    };
    let Some(stdout) = stdout else {
        cleanup_failed_spawn(&child, spec, process_group, None).await;
        return Err(WireFailure::new(PackageErrorCode::SpawnFailed));
    };
    let stderr = {
        let mut child = child.lock().await;
        child.stderr.take()
    };
    if let Some(stderr) = stderr {
        tokio::spawn(capture_stderr(
            stderr,
            spec.template.private_data_directory.clone(),
            spec.template.package_identity.to_string(),
            spec.workspace_id.to_string(),
            spec.template.source_label.clone(),
            spec.template.trust_label.clone(),
            generation,
        ));
    }
    let start_time = observed_start_time(pid)
        .await
        .unwrap_or_else(|| "unknown".into());
    let lease = ProcessLease {
        package_identity: spec.template.package_identity.to_string(),
        workspace_id: spec.workspace_id.to_string(),
        process_generation: generation,
        pid,
        process_group,
        start_time,
        canonical_executable: executable.to_string_lossy().into_owned(),
        executable_digest: spec.template.backend_digest.clone(),
        launch_token: launch_token.clone(),
    };
    let recorded = lease.clone();
    let stored = spec
        .store
        .run(move |store| store.record_process_lease(&recorded))
        .await;
    if !matches!(stored, Ok(Ok(()))) {
        cleanup_failed_spawn(&child, spec, process_group, None).await;
        return Err(WireFailure::new(PackageErrorCode::DataDirectoryUnavailable));
    }

    let client =
        protocol::start_with_capabilities(stdout, stdin, capabilities.clone(), spec.calls.limits);
    let initialize = initialize_frame(spec, generation, &launch_token, &capabilities);
    let initialized = match client.initialize(initialize).await {
        Ok(initialized) => initialized,
        Err(error) => {
            cleanup_failed_spawn(&child, spec, process_group, Some(&lease)).await;
            return Err(error);
        }
    };
    if !handshake_matches(&initialized, spec, generation, &launch_token) {
        cleanup_failed_spawn(&child, spec, process_group, Some(&lease)).await;
        return Err(WireFailure::new(PackageErrorCode::HandshakeMismatch));
    }
    let operation = if recovering {
        "lifecycle.recover"
    } else {
        "lifecycle.activate"
    };
    if let Err(error) = client
        .request(
            format!("h:{operation}-{generation}"),
            operation,
            None,
            PackageValue::Object(Default::default()),
            Duration::from_secs(30),
            true,
        )
        .await
    {
        cleanup_failed_spawn(&child, spec, process_group, Some(&lease)).await;
        return Err(error);
    }
    let health_client = client.clone();
    let health_process_group = process_group;
    let health = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if health_client.health_probe().await.is_err() {
                terminate_group(health_process_group, false);
                return;
            }
        }
    });
    Ok(ProcessGeneration {
        generation,
        client,
        port_routes: Arc::new(tokio::sync::Semaphore::new(crate::broker::PROVIDER_LIMIT)),
        child,
        process_group,
        package: spec.template.package_identity.clone(),
        workspace: spec.workspace_id.clone(),
        store: spec.store.clone(),
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
            "identity":spec.template.package_identity.as_str(),
            "version":spec.template.package_version,
            "manifestDigest":spec.template.manifest_digest,
            "backendDigest":spec.template.backend_digest,
        },
        "uiContractDigest":spec.template.ui_contract_digest,
        "processGeneration":generation.to_string(),
        "workspace":{"id":spec.workspace_id.as_str(),"name":spec.workspace_name},
        "installation":{"identity":spec.installation.as_str(),"displayName":spec.installation.as_str()},
        "dataDirectory":{"path":spec.data_directory.to_string_lossy(),"continuityId":spec.continuity_id},
        "protocol":{"major":spec.template.protocol.0,"minor":spec.template.protocol.1},
        "capabilities":capabilities,
        // The handshake publishes the contract's own limits table; the
        // constants are read here so the wire and the code cannot drift.
        "limits":{
            "maxFrameBytes":MAX_FRAME_BYTES,
            "maxInFlightEachDirection":MAX_IN_FLIGHT_EACH_DIRECTION,
            "maxWriterFrames":MAX_WRITER_FRAMES,
            "maxWriterBytes":MAX_WRITER_BYTES,
            "initialStreamCredit":INITIAL_STREAM_CREDIT
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
            == Some(spec.template.ui_contract_digest.as_str())
        && frame
            .field("package")
            .and_then(Value::as_object)
            .and_then(|package| package.get("identity"))
            .and_then(Value::as_str)
            == Some(spec.template.package_identity.as_str())
}

impl ProcessGeneration {
    pub(crate) async fn shutdown(&self) {
        self.health.abort();
        self.client.shutdown().await;
        stop_process_group(&self.child, self.process_group).await;
        self.remove_lease().await;
    }

    pub(crate) async fn terminate(&self) {
        self.health.abort();
        stop_process_group(&self.child, self.process_group).await;
        self.remove_lease().await;
    }

    async fn remove_lease(&self) {
        let package = self.package.to_string();
        let workspace = self.workspace.to_string();
        let _ = self
            .store
            .run(move |store| store.delete_process_lease(&package, &workspace))
            .await;
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

async fn stop_process_group(child: &Mutex<Child>, process_group: i32) {
    terminate_group(process_group, false);
    let leader_exited = wait_for_exit(child, Duration::from_secs(2)).await;
    // The leader may have exited while descendants remained in its process group.
    terminate_group(process_group, false);
    terminate_group(process_group, true);
    if !leader_exited {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let _ = child.lock().await.wait().await;
        })
        .await;
    }
}

async fn cleanup_failed_spawn(
    child: &Arc<Mutex<Child>>,
    spec: &ProcessSpec,
    process_group: i32,
    lease: Option<&ProcessLease>,
) {
    stop_process_group(child, process_group).await;
    if let Some(lease) = lease {
        let package = lease.package_identity.clone();
        let workspace = lease.workspace_id.clone();
        let _ = spec
            .store
            .run(move |store| store.delete_process_lease(&package, &workspace))
            .await;
    }
}

#[cfg(unix)]
fn terminate_group(process_group: i32, force: bool) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    let _ = killpg(
        Pid::from_raw(process_group),
        if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        },
    );
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[tokio::test]
    async fn failed_handshake_reaps_the_process_group_and_removes_the_lease() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        let data = root.path().join("data");
        let backend_path = root.path().join("backend/orphan.sh");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(backend_path.parent().unwrap()).unwrap();
        let backend = b"#!/bin/sh\nsleep 60 &\nprintf '\\000\\000\\000\\001{'\n".to_vec();
        std::fs::write(&backend_path, &backend).unwrap();
        std::fs::set_permissions(&backend_path, std::fs::Permissions::from_mode(0o700)).unwrap();

        let core = std::sync::Arc::new(
            teamarness_core::CoreRuntime::open_loopback_for_tests(
                teamarness_core::CoreRuntimeConfig::new(root.path().join("core")),
            )
            .await
            .unwrap(),
        );
        let spec = ProcessSpec {
            authority: crate::authority::AuthorityGeneration::default(),
            template: ProcessTemplate {
                package_identity: "dev.teamarness.orphan".parse().unwrap(),
                package_version: "1.0.0".into(),
                manifest_digest: "a".repeat(64),
                backend_digest: sha256(&backend),
                ui_contract_digest: "c".repeat(64),
                backend_path,
                private_data_directory: private.clone(),
                source_label: "bundled".into(),
                trust_label: "application_bundled".into(),
                protocol: (1, 0),
                capabilities: vec![],
                activation: teamarness_package_contract::manifest::ActivationPolicy::Eager,
            },
            workspace_id: WorkspaceId::new("00112233445566778899aabbccddeeff").unwrap(),
            workspace_name: "Orphan test".into(),
            installation: InstallationPublicIdentity::new(core.public_identity().to_string())
                .unwrap(),
            data_directory: data.clone(),
            continuity_id: "continuity".into(),
            core,
            calls: CallEnvironment {
                limiter: crate::capability::CoreCallLimiter::new(),
                limits: crate::protocol::ProtocolLimits::CURRENT,
            },
            store: HostStoreHandle::open(private.clone()).await.unwrap(),
            broker: std::sync::Weak::new(),
        };

        let error = match spawn(&spec, 1, false, Arc::new(AppendFence::default())).await {
            Ok(_) => panic!("malformed handshake unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.code, PackageErrorCode::ProtocolViolation);
        let lease_count: i64 =
            rusqlite::Connection::open(private.join("packages/package-host.sqlite3"))
                .unwrap()
                .query_row("SELECT COUNT(*) FROM process_leases", [], |row| row.get(0))
                .unwrap();
        assert_eq!(lease_count, 0);
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

pub(crate) fn reconcile_orphans(store: &HostStore) -> Result<Vec<ProcessLease>, PackageErrorCode> {
    let leases = store
        .process_leases()
        .map_err(|_| PackageErrorCode::DataDirectoryUnavailable)?;
    let mut unresolved = Vec::new();
    for lease in leases {
        let evidence = inspect_lease(&lease);
        match evidence {
            LeaseEvidence::Gone | LeaseEvidence::Exact => {
                if matches!(evidence, LeaseEvidence::Exact) {
                    terminate_group(lease.process_group, false);
                    std::thread::sleep(Duration::from_millis(100));
                    terminate_group(lease.process_group, true);
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
