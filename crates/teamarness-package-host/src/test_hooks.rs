//! Test-only, payload-free scheduling gates; never the nonblocking observer.
use std::sync::Arc;

use crate::{PackageHost, PackageIdentity};

/// A test-only scheduling gate, separate from nonblocking Port observations.
#[derive(Default)]
pub struct AuthorityPause {
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl AuthorityPause {
    pub async fn paused(&self) {
        self.reached.notified().await;
    }
    pub fn resume(&self) {
        self.release.notify_one();
    }
    pub(crate) async fn wait(&self) {
        self.reached.notify_one();
        self.release.notified().await;
    }
}

impl PackageHost {
    /// Holds the next record stream after its empty-page availability read, before another page.
    pub fn pause_next_record_availability_for_tests(
        &self,
        package: PackageIdentity,
    ) -> Arc<AuthorityPause> {
        let pause = Arc::new(AuthorityPause::default());
        *self
            .inner
            .port_broker
            .record_availability_pause
            .lock()
            .expect("test pause poisoned") = Some((package, pause.clone()));
        pause
    }

    /// Holds the next Agent binding write before Core work. Test builds only.
    pub fn pause_next_binding_write_for_tests(
        &self,
        package: PackageIdentity,
    ) -> Arc<AuthorityPause> {
        let pause = Arc::new(AuthorityPause::default());
        *self
            .inner
            .port_broker
            .binding_pause
            .lock()
            .expect("test pause poisoned") = Some((package, pause.clone()));
        pause
    }

    /// Holds the next command from this Package before Host actor lookup. No payload is retained.
    pub fn pause_next_command_actor_for_tests(
        &self,
        package: PackageIdentity,
    ) -> Arc<AuthorityPause> {
        let pause = Arc::new(AuthorityPause::default());
        *self
            .inner
            .port_broker
            .actor_pause
            .lock()
            .expect("test pause poisoned") = Some((package, pause.clone()));
        pause
    }
}
