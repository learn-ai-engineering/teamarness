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
    /// Holds authorized work after its leases are issued but before Provider dispatch/open.
    pub fn pause_next_port_authorized_for_tests(
        &self,
        package: PackageIdentity,
    ) -> Arc<AuthorityPause> {
        let pause = Arc::new(AuthorityPause::default());
        *self
            .inner
            .port_broker
            .authorized_pause
            .lock()
            .expect("test pause poisoned") = Some((package, pause.clone()));
        pause
    }
    /// Holds one already accepted event before forwarding it to the Consumer writer.
    pub fn pause_next_port_delivery_for_tests(
        &self,
        package: PackageIdentity,
    ) -> Arc<AuthorityPause> {
        let pause = Arc::new(AuthorityPause::default());
        *self
            .inner
            .port_broker
            .delivery_pause
            .lock()
            .expect("test pause poisoned") = Some((package, pause.clone()));
        pause
    }

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

/// Holds actual blocking selection work while it owns the operation and generation leases.
#[derive(Default)]
pub struct PortSelectionPause {
    reached: tokio::sync::Notify,
    release: std::sync::Condvar,
    outcome: std::sync::Mutex<Option<bool>>,
}

impl PortSelectionPause {
    pub async fn paused(&self) {
        self.reached.notified().await;
    }
    pub fn resume(&self) {
        self.finish(false);
    }
    /// Simulates loss of the completion acknowledgement after SQLite committed.
    pub fn fail_completion(&self) {
        self.finish(true);
    }
    fn finish(&self, failed: bool) {
        *self.outcome.lock().expect("selection pause poisoned") = Some(failed);
        self.release.notify_one();
    }
    pub(crate) fn wait(&self) -> bool {
        self.reached.notify_one();
        let mut outcome = self.outcome.lock().expect("selection pause poisoned");
        while outcome.is_none() {
            outcome = self
                .release
                .wait(outcome)
                .expect("selection pause poisoned");
        }
        outcome.expect("released outcome")
    }
}

impl PackageHost {
    pub fn pause_next_port_selection_for_tests(
        &self,
        package: PackageIdentity,
    ) -> Arc<PortSelectionPause> {
        let pause = Arc::new(PortSelectionPause::default());
        *self
            .inner
            .port_broker
            .selection_pause
            .lock()
            .expect("selection pause poisoned") = Some((package, pause.clone()));
        pause
    }
}
