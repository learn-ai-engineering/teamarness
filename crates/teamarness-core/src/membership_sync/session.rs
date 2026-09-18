//! The initiator half of `teamarness/membership-sync/1`.

use std::collections::BTreeSet;
use std::sync::Arc;

use iroh_base::EndpointAddr;

use crate::membership::replication::{MAX_REPLICATION_BYTES, MAX_REPLICATION_OPERATIONS};
use crate::{PublicIdentity, WorkspaceId};

use super::wire::{
    FIRST_FRAME_MAX, MembershipSyncFrame, OPERATION_FRAME_MAX, read_frame, typed_ids, wire_ids,
    write_frame,
};
use super::{
    MEMBERSHIP_DIAL_TIMEOUT, MEMBERSHIP_SYNC_ALPN, MembershipReplica, MembershipSessionError,
    MembershipSessionReport, ReplicaError,
};

/// QUIC close codes, from the design's table. A `Refused` frame reason is a
/// different value space and never doubles as a close code.
pub(crate) const CLOSE_DONE: u8 = 0;
pub(crate) const CLOSE_UNAVAILABLE: u8 = 1;
pub(crate) const CLOSE_OVERLOADED: u8 = 2;
pub(crate) const CLOSE_PEER_BUSY: u8 = 3;
pub(crate) const CLOSE_INVALID: u8 = 4;

/// Runs one replica call on a blocking thread, so no session ever holds the
/// runtime mutex on a Tokio worker.
pub(crate) async fn on_replica<T, F>(call: F) -> Result<T, MembershipSessionError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ReplicaError> + Send + 'static,
{
    match tokio::task::spawn_blocking(call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(ReplicaError::Invalid)) => Err(MembershipSessionError::Invalid),
        Ok(Err(ReplicaError::Unavailable)) => Err(MembershipSessionError::Transport(
            "local membership state is unavailable".to_owned(),
        )),
        Ok(Err(ReplicaError::Storage)) => Err(MembershipSessionError::Transport(
            "local membership storage failed".to_owned(),
        )),
        Err(_) => Err(MembershipSessionError::Transport(
            "membership replica task failed".to_owned(),
        )),
    }
}

/// Counts what one direction of a session carried, against both bounds.
pub(crate) struct ReceiveBudget {
    operations: usize,
    bytes: usize,
}

impl ReceiveBudget {
    pub(crate) const fn new() -> Self {
        Self {
            operations: 0,
            bytes: 0,
        }
    }

    pub(crate) fn admit(&mut self, envelope_len: usize) -> Result<(), MembershipSessionError> {
        self.operations += 1;
        self.bytes = self
            .bytes
            .checked_add(envelope_len)
            .ok_or(MembershipSessionError::Invalid)?;
        if self.operations > MAX_REPLICATION_OPERATIONS || self.bytes > MAX_REPLICATION_BYTES {
            return Err(MembershipSessionError::Invalid);
        }
        Ok(())
    }

    pub(crate) const fn operations(&self) -> usize {
        self.operations
    }
}

pub(crate) async fn send_operations(
    send: &mut iroh::endpoint::SendStream,
    envelopes: Vec<Vec<u8>>,
) -> Result<usize, MembershipSessionError> {
    let mut sent = 0;
    for canonical_envelope in envelopes {
        write_frame(
            send,
            &MembershipSyncFrame::Operation { canonical_envelope },
            OPERATION_FRAME_MAX,
        )
        .await?;
        sent += 1;
    }
    Ok(sent)
}

/// Dials `peer` and reconciles one Workspace's membership with it.
pub(crate) async fn run_membership_session(
    endpoint: &iroh::Endpoint,
    address: EndpointAddr,
    replica: Arc<dyn MembershipReplica>,
    workspace: WorkspaceId,
    peer: PublicIdentity,
) -> Result<MembershipSessionReport, MembershipSessionError> {
    let summary = {
        let replica = Arc::clone(&replica);
        on_replica(move || replica.summary(workspace)).await?
    };
    // An Installation only dials for a Workspace it holds; anything else is a
    // scheduling bug, not a peer problem.
    let summary = summary.ok_or(MembershipSessionError::Invalid)?;
    // A peer that has gone away is only discovered by a dial that does not
    // complete, so the dial is bounded well inside the session budget.
    let connection = tokio::time::timeout(
        MEMBERSHIP_DIAL_TIMEOUT,
        endpoint.connect(address, MEMBERSHIP_SYNC_ALPN),
    )
    .await
    .map_err(|_| MembershipSessionError::Timeout)?
    .map_err(|error| MembershipSessionError::Transport(error.to_string()))?;
    let result = exchange(&connection, &replica, workspace, peer, summary).await;
    match &result {
        Ok(_) => connection.close(CLOSE_DONE.into(), b"done"),
        Err(MembershipSessionError::Invalid | MembershipSessionError::Protocol(_)) => {
            connection.close(CLOSE_INVALID.into(), b"membership sync invalid");
        }
        Err(_) => connection.close(CLOSE_UNAVAILABLE.into(), b"membership sync unavailable"),
    }
    result
}

async fn exchange(
    connection: &iroh::endpoint::Connection,
    replica: &Arc<dyn MembershipReplica>,
    workspace: WorkspaceId,
    peer: PublicIdentity,
    summary: super::MembershipSummary,
) -> Result<MembershipSessionReport, MembershipSessionError> {
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .map_err(|error| MembershipSessionError::Transport(error.to_string()))?;
    write_frame(
        &mut send,
        &MembershipSyncFrame::Hello {
            workspace: *workspace.as_bytes(),
            version: summary.version,
            operation_ids: wire_ids(&summary.operation_ids),
        },
        FIRST_FRAME_MAX,
    )
    .await?;

    let want = match read_frame(&mut receive, FIRST_FRAME_MAX).await? {
        MembershipSyncFrame::Want { operation_ids } => operation_ids,
        MembershipSyncFrame::Refused { .. } => {
            return Ok(MembershipSessionReport {
                sent: 0,
                received: 0,
                refused: true,
            });
        }
        _ => {
            return Err(MembershipSessionError::Protocol(
                "expected a want".to_owned(),
            ));
        }
    };

    // What the responder demonstrably holds of what this Installation holds.
    // Disclosing against that set yields exactly the wanted operations this
    // peer is entitled to, with no second filter.
    let wanted = typed_ids(&want).into_iter().collect::<BTreeSet<_>>();
    let peer_has = summary
        .operation_ids
        .iter()
        .filter(|id| !wanted.contains(*id))
        .copied()
        .collect::<BTreeSet<_>>();
    let disclosed = {
        let replica = Arc::clone(replica);
        on_replica(move || replica.disclosure(workspace, peer, peer_has)).await?
    };
    let sent = send_operations(&mut send, disclosed.operations().to_vec()).await?;
    write_frame(
        &mut send,
        &MembershipSyncFrame::EndOfOperations,
        FIRST_FRAME_MAX,
    )
    .await?;

    let mut budget = ReceiveBudget::new();
    let mut refused = false;
    loop {
        match read_frame(&mut receive, OPERATION_FRAME_MAX).await? {
            MembershipSyncFrame::Operation { canonical_envelope } => {
                budget.admit(canonical_envelope.len())?;
                let replica = Arc::clone(replica);
                on_replica(move || replica.apply(peer, workspace, canonical_envelope)).await?;
            }
            MembershipSyncFrame::Done { .. } => break,
            MembershipSyncFrame::Refused { .. } => {
                refused = true;
                break;
            }
            _ => {
                return Err(MembershipSessionError::Protocol(
                    "expected an operation, done, or refusal".to_owned(),
                ));
            }
        }
    }
    let _ = send.finish();
    Ok(MembershipSessionReport {
        sent,
        received: budget.operations(),
        refused,
    })
}

/// A raw initiator that pushes nothing and records what it is served.
///
/// It runs the real handshake against a real responder, so a test can observe
/// exactly what an Installation discloses to an arbitrary peer.
#[cfg(test)]
pub(crate) async fn probe_membership_sync(
    endpoint: &iroh::Endpoint,
    address: EndpointAddr,
    workspace: WorkspaceId,
    operation_ids: Vec<[u8; 32]>,
) -> (Vec<Vec<u8>>, bool) {
    let Ok(connection) = endpoint.connect(address, MEMBERSHIP_SYNC_ALPN).await else {
        return (Vec::new(), false);
    };
    let outcome = probe_exchange(&connection, workspace, operation_ids).await;
    connection.close(CLOSE_DONE.into(), b"done");
    outcome.unwrap_or((Vec::new(), false))
}

#[cfg(test)]
async fn probe_exchange(
    connection: &iroh::endpoint::Connection,
    workspace: WorkspaceId,
    operation_ids: Vec<[u8; 32]>,
) -> Result<(Vec<Vec<u8>>, bool), MembershipSessionError> {
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .map_err(|error| MembershipSessionError::Transport(error.to_string()))?;
    write_frame(
        &mut send,
        &MembershipSyncFrame::Hello {
            workspace: *workspace.as_bytes(),
            version: [0; 32],
            operation_ids,
        },
        FIRST_FRAME_MAX,
    )
    .await?;
    match read_frame(&mut receive, FIRST_FRAME_MAX).await? {
        MembershipSyncFrame::Want { .. } => {}
        MembershipSyncFrame::Refused { .. } => return Ok((Vec::new(), true)),
        _ => {
            return Err(MembershipSessionError::Protocol(
                "expected a want".to_owned(),
            ));
        }
    }
    write_frame(
        &mut send,
        &MembershipSyncFrame::EndOfOperations,
        FIRST_FRAME_MAX,
    )
    .await?;
    let mut received = Vec::new();
    loop {
        match read_frame(&mut receive, OPERATION_FRAME_MAX).await? {
            MembershipSyncFrame::Operation { canonical_envelope } => {
                received.push(canonical_envelope);
            }
            MembershipSyncFrame::Done { .. } => return Ok((received, false)),
            MembershipSyncFrame::Refused { .. } => return Ok((received, true)),
            _ => {
                return Err(MembershipSessionError::Protocol(
                    "expected an operation, done, or refusal".to_owned(),
                ));
            }
        }
    }
}
