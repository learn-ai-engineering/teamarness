//! The responder half of `teamarness/membership-sync/1`.
//!
//! Pushed operations are accepted from any authenticated peer, because a
//! canonical membership operation authenticates itself. Disclosure is decided
//! afterwards, against the peer's standing in the updated local view, so a
//! newly admitted Member can prove its own admission and then be served.

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_base::PublicKey;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use crate::membership::replication::Disclosure;
use crate::{PublicIdentity, WorkspaceId};

use super::session::{
    CLOSE_DONE, CLOSE_INVALID, CLOSE_OVERLOADED, CLOSE_PEER_BUSY, CLOSE_UNAVAILABLE, ReceiveBudget,
    on_replica, send_operations,
};
use super::wire::{
    FIRST_FRAME_MAX, MembershipSyncFrame, OPERATION_FRAME_MAX, REFUSED_NOT_AUTHORIZED, read_frame,
    typed_ids, wire_ids, write_frame,
};
use super::{
    MAX_MEMBERSHIP_HANDLERS, MEMBERSHIP_SESSION_TIMEOUT, MembershipReplica, MembershipSessionError,
    MembershipSummary,
};

pub(crate) struct MembershipSyncHandler {
    stopping: AtomicBool,
    inbound: Arc<Semaphore>,
    peers: Arc<Mutex<HashSet<PublicKey>>>,
    replica: Arc<dyn MembershipReplica>,
}

impl std::fmt::Debug for MembershipSyncHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipSyncHandler")
            .field("stopping", &self.stopping.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl MembershipSyncHandler {
    pub(crate) fn new(replica: Arc<dyn MembershipReplica>) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            inbound: Arc::new(Semaphore::new(MAX_MEMBERSHIP_HANDLERS)),
            peers: Arc::new(Mutex::new(HashSet::new())),
            replica,
        }
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    /// One inbound session per peer, eight in total.
    fn acquire(&self, peer: PublicKey) -> Result<InboundPermit, u8> {
        let global = Arc::clone(&self.inbound)
            .try_acquire_owned()
            .map_err(|_| CLOSE_OVERLOADED)?;
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !peers.insert(peer) {
            return Err(CLOSE_PEER_BUSY);
        }
        Ok(InboundPermit {
            _global: global,
            peer,
            peers: Arc::clone(&self.peers),
        })
    }
}

struct InboundPermit {
    _global: OwnedSemaphorePermit,
    peer: PublicKey,
    peers: Arc<Mutex<HashSet<PublicKey>>>,
}

impl Drop for InboundPermit {
    fn drop(&mut self) {
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.peer);
    }
}

impl ProtocolHandler for MembershipSyncHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if self.stopping.load(Ordering::Acquire) {
            connection.close(CLOSE_UNAVAILABLE.into(), b"membership sync unavailable");
            return Ok(());
        }
        let remote = connection.remote_id();
        let _permit = match self.acquire(remote) {
            Ok(permit) => permit,
            Err(code) => {
                connection.close(code.into(), b"membership sync busy");
                return Ok(());
            }
        };
        let peer = PublicIdentity::from_bytes(*remote.as_bytes())
            .map_err(|_| AcceptError::from(std::io::Error::other("invalid peer")))?;
        let result = timeout(
            MEMBERSHIP_SESSION_TIMEOUT,
            respond(&connection, &self.replica, peer),
        )
        .await;
        match result {
            Ok(Ok(())) => connection.close(CLOSE_DONE.into(), b"done"),
            Ok(Err(MembershipSessionError::Transport(_))) => {
                connection.close(CLOSE_UNAVAILABLE.into(), b"membership sync unavailable");
            }
            Ok(Err(_)) | Err(_) => {
                connection.close(
                    CLOSE_INVALID.into(),
                    b"membership sync invalid or timed out",
                );
            }
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.stop();
    }
}

async fn respond(
    connection: &Connection,
    replica: &Arc<dyn MembershipReplica>,
    peer: PublicIdentity,
) -> Result<(), MembershipSessionError> {
    let (mut send, mut receive) = connection
        .accept_bi()
        .await
        .map_err(|error| MembershipSessionError::Transport(error.to_string()))?;
    let (workspace, remote_ids) = match read_frame(&mut receive, FIRST_FRAME_MAX).await? {
        MembershipSyncFrame::Hello {
            workspace,
            operation_ids,
            ..
        } => (
            WorkspaceId::from_bytes(workspace),
            typed_ids(&operation_ids),
        ),
        _ => {
            return Err(MembershipSessionError::Protocol("missing hello".to_owned()));
        }
    };

    let summary: Option<MembershipSummary> = {
        let replica = Arc::clone(replica);
        on_replica(move || replica.summary(workspace)).await?
    };
    let local_ids = summary
        .as_ref()
        .map(|summary| {
            summary
                .operation_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    // The want set never reveals which operations the responder holds beyond
    // the ones the initiator advertised, and an unknown Workspace looks exactly
    // like one this Installation holds nothing of.
    let want = crate::membership::replication::want(&local_ids, &remote_ids);
    write_frame(
        &mut send,
        &MembershipSyncFrame::Want {
            operation_ids: wire_ids(&want),
        },
        FIRST_FRAME_MAX,
    )
    .await?;

    let known = summary.is_some();
    let mut budget = ReceiveBudget::new();
    loop {
        match read_frame(&mut receive, OPERATION_FRAME_MAX).await? {
            MembershipSyncFrame::Operation { canonical_envelope } => {
                budget.admit(canonical_envelope.len())?;
                // A Workspace is never created from replication, so operations
                // for an unknown Workspace are read and dropped: the refusal
                // below is then identical to a non-member's.
                if known {
                    let replica = Arc::clone(replica);
                    on_replica(move || replica.apply(peer, workspace, canonical_envelope)).await?;
                }
            }
            MembershipSyncFrame::EndOfOperations => break,
            _ => {
                return Err(MembershipSessionError::Protocol(
                    "expected an operation or end of operations".to_owned(),
                ));
            }
        }
    }

    let peer_has = remote_ids.into_iter().collect::<BTreeSet<_>>();
    let disclosure = if known {
        let replica = Arc::clone(replica);
        on_replica(move || replica.disclosure(workspace, peer, peer_has)).await?
    } else {
        Disclosure::None
    };
    match disclosure {
        Disclosure::None => {
            write_frame(
                &mut send,
                &MembershipSyncFrame::Refused {
                    reason: REFUSED_NOT_AUTHORIZED,
                },
                FIRST_FRAME_MAX,
            )
            .await?;
        }
        Disclosure::CurrentMember(operations) | Disclosure::FormerMember(operations) => {
            send_operations(&mut send, operations).await?;
            let version = {
                let replica = Arc::clone(replica);
                on_replica(move || replica.summary(workspace)).await?
            }
            .map_or([0; 32], |summary| summary.version);
            write_frame(
                &mut send,
                &MembershipSyncFrame::Done { version },
                FIRST_FRAME_MAX,
            )
            .await?;
        }
    }
    let _ = send.finish();
    let _ = send.stopped().await;
    Ok(())
}
