//! Exclusive foreground ownership, truthful readiness, and ordered terminal cleanup.
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use teamarness_core::{CoreError, CoreRuntime, CoreRuntimeConfig};
use teamarness_package_host::{
    InstallationPublicIdentity, LifecycleState, PackageHost, PackageHostCommand, PackageHostConfig,
    PackageHostSnapshot, PackageHostState, ShutdownHost,
};
use tokio::time::{Instant, timeout_at};

use crate::ControlError;
use crate::control::{ControlServer, Drain, Operation, SHUTDOWN_TIMEOUT};

#[derive(Debug, Clone)]
pub struct ApplicationConfig {
    pub data_directory: PathBuf,
    pub application_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub readiness: Readiness,
    pub installation: String,
    pub host: PackageHostSnapshot,
    pub terminal_operation_failures: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    Degraded,
}

/// The same composition used by the executable. It never retires ownership before process exit.
pub struct Application {
    config: ApplicationConfig,
    core: Arc<CoreRuntime>,
    host: PackageHost,
    failures: Arc<AtomicU64>,
}

impl Application {
    pub async fn open(config: ApplicationConfig) -> Result<Self, String> {
        // production-addressing: only the foreground executable selects production Core transport.
        Self::open_with(config, CoreRuntime::open).await
    }

    /// Embedding seam: every opener receives the exclusive configuration. Tests supply Core's loopback opener.
    pub async fn open_with<F, O>(config: ApplicationConfig, opener: O) -> Result<Self, String>
    where
        O: FnOnce(CoreRuntimeConfig) -> F,
        F: Future<Output = Result<CoreRuntime, CoreError>>,
    {
        let core = Arc::new(
            opener(
                CoreRuntimeConfig::new(&config.data_directory)
                    .with_exclusive_application_ownership(),
            )
            .await
            .map_err(|error| format!("core_open: {error}"))?,
        );
        let host_config = PackageHostConfig::new(
            &config.application_root,
            config
                .application_root
                .join("packages/application-packages.v1.json"),
            &config.data_directory,
            InstallationPublicIdentity::new(core.public_identity().to_string())
                .map_err(|error| format!("installation_identity: {error}"))?,
        );
        let host = match PackageHost::open(host_config, core.clone()).await {
            Ok(host) => host,
            Err(error) => {
                // Keep Core and its ownership alive if rollback cannot complete safely.
                if !matches!(
                    timeout_at(Instant::now() + SHUTDOWN_TIMEOUT, core.shutdown()).await,
                    Ok(Ok(()))
                ) {
                    fatal_shutdown("startup_rollback");
                }
                return Err(format!("host_open: {error}"));
            }
        };
        Ok(Self {
            config,
            core,
            host,
            failures: Arc::default(),
        })
    }

    pub fn data_directory(&self) -> &Path {
        &self.config.data_directory
    }

    pub async fn status(&self) -> Status {
        status(
            &self.core,
            &self.host,
            self.failures.load(Ordering::Relaxed),
        )
        .await
    }

    pub(crate) fn record_terminal_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Stops admission, drains owned work, shuts down Host, then Core under one deadline.
    /// A timeout exits the process while this Application still holds the ownership lock.
    pub async fn run_until<S>(&self, stop: S) -> Result<(), ControlError>
    where
        S: Future<Output = Instant>,
    {
        tokio::pin!(stop);
        let bound = tokio::select! {
            biased;
            at = &mut stop => {
                self.shutdown_by(at + SHUTDOWN_TIMEOUT).await;
                return Ok(());
            }
            result = ControlServer::bind(self) => result,
        };
        let server = match bound {
            Ok(server) => server,
            Err(error) => {
                self.shutdown_by(Instant::now() + SHUTDOWN_TIMEOUT).await;
                return Err(error);
            }
        };
        let core = self.core.clone();
        let host = self.host.clone();
        let failures = self.failures.clone();
        let mut drain = server
            .accept_until(
                move |operation| {
                    let core = core.clone();
                    let host = host.clone();
                    let failures = failures.clone();
                    async move {
                        match operation {
                            Operation::Status {} => {
                                Ok(status(&core, &host, failures.load(Ordering::Relaxed)).await)
                            }
                        }
                    }
                },
                &mut stop,
            )
            .await;
        self.finish_shutdown(&mut drain).await;
        Ok(())
    }

    /// Completes an owner-bound transport drain before touching Host or Core.
    pub async fn finish_shutdown(&self, drain: &mut Drain<'_>) {
        let deadline = drain.stopped_at + SHUTDOWN_TIMEOUT;
        let uncertain = match timeout_at(deadline, drain.finish()).await {
            Ok(result) => result.is_err(),
            Err(_) => fatal_shutdown("operation_drain"),
        };
        self.shutdown_by(deadline).await;
        if uncertain {
            fatal_shutdown("operation_drain");
        }
    }

    /// Bounded cleanup is also used when endpoint binding fails after Host open.
    pub async fn shutdown_by(&self, deadline: Instant) {
        let host = match timeout_at(
            deadline,
            self.host
                .execute(PackageHostCommand::Shutdown(ShutdownHost)),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => fatal_shutdown("host_cleanup_timeout"),
        };
        if let Err(error) = &host {
            eprintln!("host_shutdown_failed: {error}");
        }
        let core = match timeout_at(deadline, self.core.shutdown()).await {
            Ok(result) => result,
            Err(_) => fatal_shutdown("core_cleanup_timeout"),
        };
        if let Err(error) = &core {
            eprintln!("core_shutdown_failed: {error}");
        }
        if host.is_err() || core.is_err() {
            fatal_shutdown("host_core_cleanup");
        }
    }
}

async fn status(core: &CoreRuntime, host: &PackageHost, failures: u64) -> Status {
    let snapshot = host.snapshot().await;
    let degraded = failures != 0
        || snapshot.state != PackageHostState::Running
        || !snapshot.diagnostics.is_empty()
        || snapshot.instances.iter().any(|instance| {
            matches!(
                instance.lifecycle,
                LifecycleState::Failed | LifecycleState::CrashLoop | LifecycleState::Recovering
            )
        });
    Status {
        readiness: if degraded {
            Readiness::Degraded
        } else {
            Readiness::Ready
        },
        installation: core.public_identity().to_string(),
        host: snapshot,
        terminal_operation_failures: failures,
    }
}

pub(crate) fn fatal_shutdown(stage: &str) -> ! {
    eprintln!("shutdown_failed stage={stage} outcomeUncertain=true");
    // Do not unwind/drop ownership while an operation or Package process may still use storage.
    std::process::exit(1)
}
