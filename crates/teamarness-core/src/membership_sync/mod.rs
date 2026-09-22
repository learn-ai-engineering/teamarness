//! Canonical membership replication over `teamarness/membership-sync/1`.
//!
//! Membership moves between Installations on its own Core protocol, with no
//! dependence on whether any Package has records to synchronize. One finite
//! session per `(Workspace, peer)` reconciles sorted operation-ID sets and then
//! exchanges the canonical envelopes each side lacks, in causal order.
//!
//! This is a crate-root module, not a child of `network`: `network` owns the
//! Endpoint and Router and names no protocol or membership type.

pub(crate) mod handler;
pub(crate) mod session;
pub(crate) mod wire;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::membership::replication::Disclosure;
use crate::{MembershipOperationId, PublicIdentity, WorkspaceId};

pub(crate) const MEMBERSHIP_SYNC_ALPN: &[u8] = b"teamarness/membership-sync/1";
pub(crate) const MEMBERSHIP_SESSION_TIMEOUT: Duration = Duration::from_secs(30);
/// Bounds the dial alone, so an unreachable peer is discovered well before the
/// whole session budget is spent.
pub(crate) const MEMBERSHIP_DIAL_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const MAX_MEMBERSHIP_HANDLERS: usize = 8;

pub(crate) use handler::MembershipSyncHandler;
pub(crate) use session::run_membership_session;

/// The membership-sync registration the runtime composition point hands to
/// `network`, which names neither this protocol nor its handler.
pub(crate) fn membership_sync_registration(
    replica: Arc<dyn MembershipReplica>,
) -> crate::network::ProtocolRegistration {
    (
        MEMBERSHIP_SYNC_ALPN.to_vec(),
        Box::new(MembershipSyncHandler::new(replica))
            as Box<dyn iroh::protocol::DynProtocolHandler>,
    )
}

/// Reconciles one Workspace's canonical membership with one peer, inside the
/// session timeout and the second outbound peer budget.
pub(crate) async fn run_membership_session_on(
    network: &crate::network::Network,
    address: iroh_base::EndpointAddr,
    replica: Arc<dyn MembershipReplica>,
    workspace: WorkspaceId,
    peer: PublicIdentity,
) -> Result<MembershipSessionReport, MembershipSessionError> {
    let overloaded = MembershipSessionError::Transport("no outbound peer slot is free".to_owned());
    network
        .run_extra_outbound(overloaded, move |endpoint| async move {
            match tokio::time::timeout(
                MEMBERSHIP_SESSION_TIMEOUT,
                run_membership_session(&endpoint, address, replica, workspace, peer),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(MembershipSessionError::Timeout),
            }
        })
        .await
}

/// What one Installation advertises about a Workspace it knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MembershipSummary {
    pub(crate) version: [u8; 32],
    /// Sorted, duplicate free.
    pub(crate) operation_ids: Vec<MembershipOperationId>,
}

/// What applying one received operation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    Accepted,
    Duplicate,
    Quarantined,
}

/// Why a replica call did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplicaError {
    /// The operation is not acceptable. The session ends.
    Invalid,
    /// The local runtime cannot answer right now. The session ends without
    /// blaming the peer, and a later pass retries.
    Unavailable,
    /// Durable storage failed.
    Storage,
}

/// The Core seam every membership session speaks to.
///
/// Sessions never name runtime state, storage, or projections; the runtime
/// implements this trait and every call runs on a blocking thread.
pub(crate) trait MembershipReplica: Send + Sync + std::fmt::Debug + 'static {
    /// `Ok(None)` when this Installation does not know the Workspace. A
    /// replication session never creates one.
    fn summary(&self, workspace: WorkspaceId) -> Result<Option<MembershipSummary>, ReplicaError>;

    fn disclosure(
        &self,
        workspace: WorkspaceId,
        peer: PublicIdentity,
        peer_has: BTreeSet<MembershipOperationId>,
    ) -> Result<Disclosure, ReplicaError>;

    fn apply(
        &self,
        peer: PublicIdentity,
        workspace: WorkspaceId,
        canonical_envelope: Vec<u8>,
    ) -> Result<ApplyOutcome, ReplicaError>;
}

/// How one membership session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MembershipSessionReport {
    pub(crate) sent: usize,
    pub(crate) received: usize,
    pub(crate) refused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MembershipSessionError {
    Transport(String),
    Protocol(String),
    Invalid,
    Timeout,
}

impl std::fmt::Display for MembershipSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "membership sync transport: {error}"),
            Self::Protocol(error) => write!(formatter, "membership sync protocol: {error}"),
            Self::Invalid => formatter.write_str("membership sync received an invalid operation"),
            Self::Timeout => formatter.write_str("membership sync timed out"),
        }
    }
}

impl From<wire::WireError> for MembershipSessionError {
    fn from(error: wire::WireError) -> Self {
        match error {
            wire::WireError::Closed => Self::Transport(error.to_string()),
            wire::WireError::TooLarge | wire::WireError::Malformed => {
                Self::Protocol(error.to_string())
            }
        }
    }
}

/// Workspaces and peers the worker should run a membership session with before
/// its next record pass.
///
/// A package session that is refused, or that finds the two membership versions
/// disagree, records the peer here: the refused Installation can then prove its
/// own admission by pushing it, instead of waiting for the idle sweep.
#[derive(Debug)]
pub(crate) struct MembershipDemand {
    peers: Mutex<BTreeSet<(WorkspaceId, PublicIdentity)>>,
    changed: tokio::sync::Notify,
}

impl MembershipDemand {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            peers: Mutex::new(BTreeSet::new()),
            changed: tokio::sync::Notify::new(),
        })
    }

    pub(crate) fn note(&self, workspace: WorkspaceId, peer: PublicIdentity) {
        let inserted = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((workspace, peer));
        if inserted {
            self.changed.notify_one();
        }
    }

    /// Takes the demands the caller can serve now and leaves the rest in place,
    /// without notifying: a peer that is waiting out a backoff keeps its demand
    /// until a later pass can act on it, and re-arming the notification here
    /// would spin the loop that just declined it.
    pub(crate) fn take_ready(
        &self,
        ready: impl Fn(&(WorkspaceId, PublicIdentity)) -> bool,
    ) -> BTreeSet<(WorkspaceId, PublicIdentity)> {
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let taken = peers.iter().filter(|key| ready(key)).copied().collect();
        peers.retain(|key| !ready(key));
        taken
    }

    pub(crate) async fn changed(&self) {
        self.changed.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_demand_is_recorded_once_and_taken_once() {
        let demand = MembershipDemand::new();
        let workspace = WorkspaceId::from_bytes([2; 16]);
        let peer = PublicIdentity::from_bytes(
            *iroh_base::SecretKey::from_bytes(&[3; 32])
                .public()
                .as_bytes(),
        )
        .unwrap();

        demand.note(workspace, peer);
        demand.note(workspace, peer);
        assert!(
            demand.take_ready(|_| false).is_empty(),
            "a demand the caller cannot serve stays recorded"
        );
        assert_eq!(
            demand.take_ready(|_| true),
            BTreeSet::from([(workspace, peer)])
        );
        assert!(demand.take_ready(|_| true).is_empty());
    }
}
