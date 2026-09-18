//! A cluster of real Core runtimes on one machine.
//!
//! Nodes are real [`CoreRuntime`]s with loopback Iroh endpoints, real SQLite
//! storage, and the system clock. Addresses are exchanged through the loopback
//! address book, so a node can dial a peer it has never met.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::super::{CoreRuntime, CoreRuntimeConfig, PublicIdentity, SystemClock, WorkspaceId};
use crate::RecordId;
use crate::synchronization::limits::SessionLimits;

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// A unique temporary root per test, so parallel tests never share a directory.
pub(super) fn cluster_root(test: &str) -> PathBuf {
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-cluster-{test}-{}-{sequence}",
        std::process::id()
    ))
}

pub(super) const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);

pub(super) struct SyncCluster {
    root: PathBuf,
    names: Vec<&'static str>,
    nodes: Vec<Option<CoreRuntime>>,
    /// An Installation keeps its identity across a restart, so a test can name
    /// a stopped node's identity while it is away.
    identities: Vec<PublicIdentity>,
    /// A node keeps the bounds it was started with across a restart.
    limits: Vec<SessionLimits>,
}

impl SyncCluster {
    pub(super) async fn start(test: &str, names: &[&'static str]) -> Self {
        Self::start_with_limits(test, names, &[]).await
    }

    /// Starts a cluster where the named nodes run under injected bounds.
    ///
    /// Everything else is the real thing; only the numbers are smaller, so a
    /// bound is reached in a few records instead of a few thousand.
    pub(super) async fn start_with_limits(
        test: &str,
        names: &[&'static str],
        overrides: &[(&str, SessionLimits)],
    ) -> Self {
        let root = cluster_root(test);
        let _ = std::fs::remove_dir_all(&root);
        let limits = names
            .iter()
            .map(|name| {
                overrides
                    .iter()
                    .find(|(named, _)| named == name)
                    .map_or(SessionLimits::CURRENT, |(_, limits)| *limits)
            })
            .collect::<Vec<_>>();
        let mut nodes = Vec::with_capacity(names.len());
        for (name, limits) in names.iter().zip(&limits) {
            nodes.push(Some(Self::open(&root, name, *limits).await));
        }
        let identities = nodes
            .iter()
            .flatten()
            .map(CoreRuntime::public_identity)
            .collect();
        let cluster = Self {
            root,
            names: names.to_vec(),
            nodes,
            identities,
            limits,
        };
        cluster.exchange_addresses();
        cluster
    }

    async fn open(root: &std::path::Path, name: &str, limits: SessionLimits) -> CoreRuntime {
        CoreRuntime::open_with_clock_and_readiness(
            CoreRuntimeConfig::new(root.join(name)).with_session_limits_for_test(limits),
            Box::new(SystemClock),
            true,
        )
        .await
        .expect("a cluster node opens on a fresh temporary directory")
    }

    /// Teaches every live node how to reach every other live node.
    fn exchange_addresses(&self) {
        for left in self.nodes.iter().flatten() {
            for right in self.nodes.iter().flatten() {
                if left.public_identity() != right.public_identity() {
                    left.remember_peer_address_for_test(right.endpoint_address_for_test());
                }
            }
        }
    }

    fn index(&self, name: &str) -> usize {
        self.names
            .iter()
            .position(|candidate| *candidate == name)
            .unwrap_or_else(|| panic!("unknown cluster node {name}"))
    }

    pub(super) fn node(&self, name: &str) -> &CoreRuntime {
        self.nodes[self.index(name)]
            .as_ref()
            .unwrap_or_else(|| panic!("cluster node {name} is stopped"))
    }

    pub(super) fn address(&self, name: &str) -> iroh_base::EndpointAddr {
        self.node(name).endpoint_address_for_test()
    }

    pub(super) fn identity(&self, name: &str) -> PublicIdentity {
        self.identities[self.index(name)]
    }

    /// A stopped node holds no state, so it reports no members.
    pub(super) fn is_running(&self, name: &str) -> bool {
        self.nodes[self.index(name)].is_some()
    }

    /// Shuts a node down and drops it, keeping its data directory.
    pub(super) async fn stop(&mut self, name: &str) {
        let index = self.index(name);
        if let Some(node) = self.nodes[index].take() {
            node.shutdown().await.expect("a node shuts down cleanly");
        }
    }

    /// Changes the bounds a node runs under from its next start.
    pub(super) fn set_limits_for_test(&mut self, name: &str, limits: SessionLimits) {
        let index = self.index(name);
        self.limits[index] = limits;
    }

    /// Reopens a stopped node's data directory and re-exchanges addresses.
    pub(super) async fn restart(&mut self, name: &str) {
        let index = self.index(name);
        assert!(self.nodes[index].is_none(), "{name} is still running");
        self.nodes[index] = Some(Self::open(&self.root, name, self.limits[index]).await);
        self.exchange_addresses();
    }

    /// Admits `joiner` through the public invitation flow.
    pub(super) async fn admit(&self, manager: &str, joiner: &str, workspace: WorkspaceId) {
        let now = crate::runtime::Clock::now_unix_ms(&SystemClock);
        let invitation = self
            .node(manager)
            .create_workspace_invitation_for_test(workspace, now, now.saturating_add(600_000))
            .expect("the manager issues an invitation");
        self.node(joiner)
            .join_workspace(&invitation)
            .await
            .expect("the joiner redeems the invitation");
        self.exchange_addresses();
    }

    /// Runs one real membership session from `from` to `to`.
    ///
    /// This is the production protocol, not a shim: a test uses it when it
    /// needs one specific exchange to have happened before it continues.
    pub(super) async fn replicate_membership(&self, from: &str, to: &str, workspace: WorkspaceId) {
        self.node(from)
            .membership_session_for_test(self.address(to), workspace, self.identity(to))
            .await
            .expect("a membership session between two members completes");
    }

    /// Every accepted membership operation identifier one node holds.
    pub(super) fn membership_operation_ids(
        &self,
        name: &str,
        workspace: WorkspaceId,
    ) -> BTreeSet<crate::MembershipOperationId> {
        self.node(name).membership_operation_ids_for_test(workspace)
    }

    pub(super) fn members(&self, name: &str, workspace: WorkspaceId) -> BTreeSet<PublicIdentity> {
        if !self.is_running(name) {
            return BTreeSet::new();
        }
        self.node(name)
            .membership_snapshot(workspace)
            .map(|snapshot| {
                snapshot
                    .members()
                    .iter()
                    .map(super::super::MemberSnapshot::identity)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn record_ids(
        &self,
        name: &str,
        workspace: WorkspaceId,
        package: &str,
    ) -> BTreeSet<RecordId> {
        let mut ids = BTreeSet::new();
        if !self.is_running(name) {
            return ids;
        }
        let Ok(mut subscription) = self.node(name).subscribe_records(workspace, package, None)
        else {
            return ids;
        };
        while let Ok(Some(event)) = subscription.next_event_timeout(Duration::ZERO) {
            ids.insert(event.record_id());
        }
        ids
    }

    /// Polls `check` until it holds, waking every live node's worker each second.
    pub(super) async fn wait_until(
        &self,
        what: &str,
        deadline: Duration,
        mut check: impl FnMut(&Self) -> bool,
    ) {
        let expiry = Instant::now() + deadline;
        let mut last_wake = Instant::now() - Duration::from_secs(1);
        loop {
            if check(self) {
                return;
            }
            assert!(Instant::now() < expiry, "timed out waiting until {what}");
            if last_wake.elapsed() >= Duration::from_secs(1) {
                for node in self.nodes.iter().flatten() {
                    node.sync_now_for_test();
                }
                last_wake = Instant::now();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub(super) async fn finish(mut self) {
        for index in 0..self.nodes.len() {
            if let Some(node) = self.nodes[index].take() {
                let _ = node.shutdown().await;
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
