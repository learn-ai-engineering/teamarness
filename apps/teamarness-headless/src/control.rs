//! Closed operator protocol. Each connection carries exactly one request and one response.
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout};

use crate::application::Application;
use crate::{ControlError, ErrorCode, socket};
use serde_json::Value;
use std::path::PathBuf;
use teamarness_package_host::{PackageIdentity, PackageInstanceKey};

pub const PROTOCOL_MAJOR: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CONNECTIONS: usize = 16;
pub const MAX_OPERATIONS: usize = 8;
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);
pub const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(90);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Status {},
    IdentityShow {},
    IdentitySetName {
        name: String,
    },
    WorkspaceList {},
    WorkspaceJoin {
        invitation: String,
        person_name: String,
    },
    WorkspaceMembers {
        workspace: String,
    },
    WorkspaceRenameSelf {
        workspace: String,
        name: String,
    },
    AgentList {
        workspace: String,
    },
    AgentAdd {
        workspace: String,
        name: String,
    },
    AgentRename {
        workspace: String,
        agent: String,
        name: String,
    },
    AgentRemove {
        workspace: String,
        agent: String,
    },
    PackagesList {},
    PackagesInspect {
        directory: PathBuf,
    },
    PackagesInstall {
        directory: PathBuf,
        expected_identity: PackageIdentity,
        accepted_risks: Vec<String>,
    },
    PackagesReload {
        package: PackageIdentity,
    },
    PackagesRemove {
        package: PackageIdentity,
    },
    PackagesRestart {
        workspace: String,
        package: PackageIdentity,
    },
    PackagesPause {
        workspace: String,
        package: PackageIdentity,
    },
    PackageOperations {
        workspace: String,
        package: PackageIdentity,
    },
    PackageSchema {
        workspace: String,
        package: PackageIdentity,
        operation: String,
    },
    PackageRequest {
        workspace: String,
        package: PackageIdentity,
        operation: String,
        contract_digest: String,
        payload: Value,
    },
    PackagesResume {
        workspace: String,
        package: PackageIdentity,
    },
}

impl Operation {
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Status {}
                | Self::IdentityShow {}
                | Self::WorkspaceList {}
                | Self::WorkspaceMembers { .. }
                | Self::AgentList { .. }
                | Self::PackagesList {}
                | Self::PackagesInspect { .. }
                | Self::PackageOperations { .. }
                | Self::PackageSchema { .. }
        )
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Status {} => "status",
            Self::IdentityShow {} => "identity_show",
            Self::IdentitySetName { .. } => "identity_set_name",
            Self::WorkspaceList {} => "workspace_list",
            Self::WorkspaceJoin { .. } => "workspace_join",
            Self::WorkspaceMembers { .. } => "workspace_members",
            Self::WorkspaceRenameSelf { .. } => "workspace_rename_self",
            Self::AgentList { .. } => "agent_list",
            Self::AgentAdd { .. } => "agent_add",
            Self::AgentRename { .. } => "agent_rename",
            Self::AgentRemove { .. } => "agent_remove",
            Self::PackagesList {} => "packages_list",
            Self::PackagesInspect { .. } => "packages_inspect",
            Self::PackagesInstall { .. } => "packages_install",
            Self::PackagesReload { .. } => "packages_reload",
            Self::PackagesRemove { .. } => "packages_remove",
            Self::PackagesRestart { .. } => "packages_restart",
            Self::PackagesPause { .. } => "packages_pause",
            Self::PackagesResume { .. } => "packages_resume",
            Self::PackageOperations { .. } => "package_operations",
            Self::PackageSchema { .. } => "package_schema",
            Self::PackageRequest { .. } => "package_request",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub major: u16,
    pub id: u64,
    pub operation: Operation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response<T = Value> {
    pub major: u16,
    pub id: u64,
    pub result: Result<T, ControlError>,
}

/// Reads the complete frame under one deadline, checking the prefix before allocation.
pub async fn read_frame<T: DeserializeOwned>(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<T, ControlError> {
    timeout(IO_TIMEOUT, async {
        let length = reader
            .read_u32()
            .await
            .map_err(|_| ControlError::new(ErrorCode::Unavailable))? as usize;
        if length > MAX_FRAME_BYTES {
            return Err(ControlError::new(ErrorCode::FrameTooLarge));
        }
        let mut bytes = vec![0; length];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(|_| ControlError::new(ErrorCode::Unavailable))?;
        serde_json::from_slice(&bytes).map_err(|_| ControlError::new(ErrorCode::InvalidInput))
    })
    .await
    .map_err(|_| ControlError::new(ErrorCode::Timeout))?
}

/// Writes prefix and body under a single deadline, even when the peer makes slow progress.
pub async fn write_frame<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &T,
) -> Result<(), ControlError> {
    let bytes = encode_frame(value)?;
    write_encoded(writer, &bytes).await
}

fn encode_frame(value: &impl Serialize) -> Result<Vec<u8>, ControlError> {
    let mut buffer = FrameBuffer(Vec::new());
    serde_json::to_writer(&mut buffer, value).map_err(|error| {
        ControlError::new(if error.is_io() {
            ErrorCode::FrameTooLarge
        } else {
            ErrorCode::Internal
        })
    })?;
    Ok(buffer.0)
}

async fn write_encoded(
    writer: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
) -> Result<(), ControlError> {
    timeout(IO_TIMEOUT, async {
        writer
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await?;
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| ControlError::new(ErrorCode::Timeout))?
    .map_err(|_| ControlError::new(ErrorCode::Unavailable))
}

struct FrameBuffer(Vec<u8>);
impl std::io::Write for FrameBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_FRAME_BYTES - self.0.len() {
            return Err(std::io::Error::other("frame_too_large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One owner-bound listener. Dropping the observer does not cancel an admitted dispatch.
pub struct ControlServer<'a> {
    _owner: &'a Application,
    endpoint: socket::Endpoint,
}

impl<'a> ControlServer<'a> {
    pub async fn bind(owner: &'a Application) -> Result<Self, ControlError> {
        Ok(Self {
            endpoint: socket::Endpoint::bind(owner.data_directory()).await?,
            _owner: owner,
        })
    }

    /// Stops admission at the first stop event. The returned drain owns every accepted task.
    pub async fn accept_until<D, F, S, T>(mut self, dispatch: D, stop: S) -> Drain<'a>
    where
        D: Fn(Operation) -> F + Send + Sync + 'static,
        F: Future<Output = Result<T, ControlError>> + Send + 'static,
        T: Serialize + Send + Sync + 'static,
        S: Future<Output = Instant>,
    {
        let listener = self.endpoint.listener.take().expect("bound listener");
        let dispatch = Arc::new(dispatch);
        let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let operations = Arc::new(Semaphore::new(MAX_OPERATIONS));
        let package_requests = Arc::new(OperatorRequestOwners::default());
        let mut tasks = JoinSet::new();
        let mut failed = false;
        tokio::pin!(stop);
        let stopped_at = loop {
            tokio::select! {
                biased;
                at = &mut stop => break at,
                completion = tasks.join_next(), if !tasks.is_empty() => {
                    if completion.is_some_and(|result| result.is_err()) { failed = true; self._owner.record_terminal_failure(); }
                }
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else { failed = true; break Instant::now(); };
                    let Ok(connection) = connections.clone().try_acquire_owned() else {
                        // No task or queue is created for a refused connection.
                        drop(stream);
                        continue;
                    };
                    let operations = operations.clone();
                    let dispatch = dispatch.clone();
                    let package_requests = package_requests.clone();
                    tasks.spawn(async move {
                        let _connection = connection;
                        if socket::check_peer(&stream).is_err() { return; }
                        let request: Request = match read_frame(&mut stream).await {
                            Ok(request) => request,
                            Err(error) => {
                                let _ = write_frame(&mut stream, &Response::<Value> { major: PROTOCOL_MAJOR, id: 0, result: Err(error) }).await;
                                return;
                            }
                        };
                        let result = if request.major != PROTOCOL_MAJOR {
                            Err(ControlError::new(ErrorCode::UnsupportedVersion))
                        } else {
                            match operations.clone().try_acquire_owned() {
                                Ok(permit) => {
                                    // This task owns the permit through dispatch AND terminal response writing.
                                    let started = Instant::now();
                                    let name = request.operation.name();
                                    let (result, owner) = match OperatorRequestOwner::acquire(&request.operation, package_requests) {
                                        Ok(owner) => (dispatch(request.operation).await, owner),
                                        Err(error) => (Err(error), None),
                                    };
                                    let outcome = match &result { Ok(_) => "ok".to_owned(), Err(error) => format!("{:?} uncertain={}", error.code, error.outcome_uncertain) };
                                    eprintln!("operator_complete id={} operation={} outcome={} durationMs={}", request.id, name, outcome, started.elapsed().as_millis());
                                    let response: Response<T> = Response { major: PROTOCOL_MAJOR, id: request.id, result };
                                    send_response(&mut stream, response).await;
                                    drop(owner);
                                    drop(permit);
                                    return;
                                }
                                Err(_) => Err(ControlError::new(if operations.is_closed() { ErrorCode::ShuttingDown } else { ErrorCode::Busy })),
                            }
                        };
                        send_response(&mut stream, Response::<T> { major: PROTOCOL_MAJOR, id: request.id, result }).await;
                    });
                }
            }
        };
        // Close admission even for connected peers which have not yet supplied a complete request.
        operations.close();
        drop(listener);
        Drain {
            tasks,
            stopped_at,
            failed,
            _server: self,
        }
    }
}

/// Only operator requests share this gate. Port traffic and Package background work do not.
#[derive(Default)]
struct OperatorRequestOwners {
    instances: std::sync::Mutex<std::collections::BTreeSet<PackageInstanceKey>>,
}
struct OperatorRequestOwner {
    key: PackageInstanceKey,
    owners: Arc<OperatorRequestOwners>,
}
impl OperatorRequestOwner {
    fn acquire(
        operation: &Operation,
        owners: Arc<OperatorRequestOwners>,
    ) -> Result<Option<Self>, ControlError> {
        let Operation::PackageRequest {
            workspace, package, ..
        } = operation
        else {
            return Ok(None);
        };
        let key = crate::administration::instance(workspace, package.clone())?;
        if !owners
            .instances
            .lock()
            .expect("operator ownership poisoned")
            .insert(key.clone())
        {
            return Err(ControlError::new(ErrorCode::Busy));
        }
        Ok(Some(Self { key, owners }))
    }
}
impl Drop for OperatorRequestOwner {
    fn drop(&mut self) {
        self.owners
            .instances
            .lock()
            .expect("operator ownership poisoned")
            .remove(&self.key);
    }
}

async fn send_response<T: Serialize>(stream: &mut UnixStream, response: Response<T>) {
    if let Err(error) = write_frame(stream, &response).await
        && error.code == ErrorCode::FrameTooLarge
    {
        let mut error = ControlError::new(ErrorCode::ResponseTooLarge);
        error.outcome_uncertain = true;
        let _ = write_frame(
            stream,
            &Response::<Value> {
                major: PROTOCOL_MAJOR,
                id: response.id,
                result: Err(error),
            },
        )
        .await;
    }
}

/// Retains owner lifetime and task handles until all admitted work terminates.
pub struct Drain<'a> {
    tasks: JoinSet<()>,
    pub stopped_at: Instant,
    failed: bool,
    _server: ControlServer<'a>,
}
impl Drain<'_> {
    pub async fn finish(&mut self) -> Result<(), ControlError> {
        while let Some(result) = self.tasks.join_next().await {
            if result.is_err() {
                self.failed = true;
            }
        }
        if self.failed {
            let mut error = ControlError::new(ErrorCode::Internal);
            error.outcome_uncertain = true;
            Err(error)
        } else {
            Ok(())
        }
    }
}

/// Connects without touching storage or retrying an exchange.
pub async fn request(directory: &Path, request: Request) -> Result<Response, ControlError> {
    let encoded = encode_frame(&request)?;
    let path = socket::validate_endpoint(directory)?;
    let mut stream = timeout(IO_TIMEOUT, UnixStream::connect(path))
        .await
        .map_err(|_| ControlError::new(ErrorCode::Timeout))?
        .map_err(|_| ControlError::new(ErrorCode::Unavailable))?;
    socket::check_peer(&stream)?;
    socket::validate_endpoint(directory)?;
    let exchanged = async {
        write_encoded(&mut stream, &encoded).await?;
        let response: Response = timeout(OBSERVATION_TIMEOUT, read_frame_observation(&mut stream))
            .await
            .map_err(|_| ControlError::new(ErrorCode::Timeout))??;
        if response.major != PROTOCOL_MAJOR {
            return Err(ControlError::new(ErrorCode::UnsupportedVersion));
        }
        if response.id != request.id {
            return Err(ControlError::new(ErrorCode::InvalidInput));
        }
        Ok(response)
    }
    .await;
    let response = exchanged.map_err(|mut error| {
        error.outcome_uncertain |= request.operation.is_mutation();
        error
    })?;
    Ok(response)
}

// Response observation has its own 90s budget, not the server's 5s incoming-request budget.
async fn read_frame_observation(reader: &mut UnixStream) -> Result<Response, ControlError> {
    let length = reader
        .read_u32()
        .await
        .map_err(|_| ControlError::new(ErrorCode::Unavailable))? as usize;
    if length > MAX_FRAME_BYTES {
        return Err(ControlError::new(ErrorCode::FrameTooLarge));
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|_| ControlError::new(ErrorCode::Unavailable))?;
    serde_json::from_slice(&bytes).map_err(|_| ControlError::new(ErrorCode::InvalidInput))
}
