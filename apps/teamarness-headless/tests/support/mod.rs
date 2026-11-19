#![allow(dead_code)]
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
