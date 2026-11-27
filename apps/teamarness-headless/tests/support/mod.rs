#![allow(dead_code)]
#[path = "../../../../crates/teamarness-package-host/tests/common/mod.rs"]
pub mod host_fixture;
use host_fixture::tempfile;
use std::path::PathBuf;
use teamarness_core::CoreRuntime;
use teamarness_headless::application::{Application, ApplicationConfig};

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub config: ApplicationConfig,
}
impl Fixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        let application_root = root.path().join("app");
        std::fs::create_dir_all(application_root.join("packages")).unwrap();
        std::fs::write(
            application_root.join("packages/application-packages.v1.json"),
            br#"{"schemaVersion":1,"packages":[]}"#,
        )
        .unwrap();
        let config = ApplicationConfig {
            data_directory: root.path().join("data"),
            application_root,
        };
        Self { root, config }
    }
    pub async fn open(&self) -> Application {
        Application::open_with(self.config.clone(), CoreRuntime::open_loopback_for_tests)
            .await
            .unwrap()
    }
    pub fn socket(&self) -> PathBuf {
        self.config.data_directory.join("control.sock")
    }
}

pub fn request(id: u64) -> teamarness_headless::control::Request {
    teamarness_headless::control::Request {
        major: 1,
        id,
        operation: teamarness_headless::control::Operation::Status {},
    }
}

pub struct Running {
    pub app: std::sync::Arc<Application>,
    stop: tokio::sync::oneshot::Sender<tokio::time::Instant>,
    task: tokio::task::JoinHandle<()>,
}
impl Running {
    pub async fn start(app: Application) -> Self {
        let app = std::sync::Arc::new(app);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let serving = app.clone();
        let task = tokio::spawn(async move {
            serving
                .run_until(async { stopped.await.unwrap() })
                .await
                .unwrap();
        });
        let running = Self { app, stop, task };
        // Read-only readiness observation is not mutation replay.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while teamarness_headless::control::request(running.app.data_directory(), request(0))
                .await
                .is_err()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        running
    }
    pub async fn call(
        &self,
        operation: teamarness_headless::control::Operation,
    ) -> Result<serde_json::Value, teamarness_headless::ControlError> {
        teamarness_headless::control::request(
            self.app.data_directory(),
            teamarness_headless::control::Request {
                major: 1,
                id: 1,
                operation,
            },
        )
        .await
        .unwrap()
        .result
    }
    pub async fn cli(&self, arguments: &[&str]) -> std::process::Output {
        let directory = self.app.data_directory().to_owned();
        let arguments: Vec<String> = arguments.iter().map(|s| (*s).to_owned()).collect();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_teamarness-headless"))
                .arg("--data-dir")
                .arg(directory)
                .args(arguments)
                .output()
                .unwrap()
        })
        .await
        .unwrap()
    }
    pub async fn stop(self) {
        self.stop.send(tokio::time::Instant::now()).unwrap();
        self.task.await.unwrap();
    }
}

pub async fn inviter(fixture: &Fixture) -> (Application, String, String) {
    inviter_with_expiry(fixture, false).await
}

pub async fn inviter_with_expiry(
    fixture: &Fixture,
    expired: bool,
) -> (Application, String, String) {
    let (send, receive) = tokio::sync::oneshot::channel();
    let app = Application::open_with(fixture.config.clone(), |config| async {
        let core = CoreRuntime::open_loopback_for_tests(config).await?;
        let workspace = core
            .create_workspace_with_person_name("Team", "Issuer")
            .unwrap();
        let invitation = if expired {
            core.expired_workspace_invitation_for_tests(workspace.id())
                .unwrap()
        } else {
            core.create_workspace_invitation(workspace.id())
                .await
                .unwrap()
        };
        send.send((workspace.id().to_string(), invitation.encoded().to_owned()))
            .unwrap();
        Ok(core)
    })
    .await
    .unwrap();
    let (workspace, invitation) = receive.await.unwrap();
    (app, workspace, invitation)
}

pub mod packages;
