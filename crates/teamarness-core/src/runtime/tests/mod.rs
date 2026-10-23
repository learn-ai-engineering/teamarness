//! Runtime tests that drive one `CoreRuntime` at a time.
//!
//! Every test here is hermetic and exercises real components: a real
//! [`CoreRuntime`](crate::CoreRuntime) on a temporary directory, real Iroh
//! endpoints on loopback, and real SQLite. Fakes appear only to inject a fault
//! or to read a clock.

mod lifecycle;
mod membership;
mod sync;

use super::*;
use crate::invitation::{sign_claim, sign_token};
use crate::membership::{AdmissionProof, MembershipAction, MembershipEnvelope};
use crate::{RecordEvent, RecordId};
use iroh_base::SecretKey;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

pub(super) struct RecordingClock {
    calls: Arc<AtomicU64>,
}

impl Clock for RecordingClock {
    fn now_unix_ms(&self) -> i64 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        1_234
    }
}

pub(super) async fn wait_for_record(
    runtime: &CoreRuntime,
    workspace: WorkspaceId,
    package: &str,
    record_id: RecordId,
) -> RecordEvent {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(mut subscription) = runtime.subscribe_records(workspace, package, None) {
            while let Some(event) = subscription
                .next_event_timeout(std::time::Duration::ZERO)
                .unwrap()
            {
                if event.record_id() == record_id {
                    return event;
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "record did not arrive before the test deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

pub(super) fn member_secret(byte: u8) -> SecretKey {
    SecretKey::from_bytes(&[byte; 32])
}

pub(super) fn runtime_secret(runtime: &CoreRuntime) -> SecretKey {
    runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .secret_key
        .clone()
}

pub(super) fn runtime_heads(
    runtime: &CoreRuntime,
    workspace_id: WorkspaceId,
) -> Vec<MembershipOperationId> {
    runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .memberships
        .get(&workspace_id)
        .unwrap()
        .heads()
}

/// An admission as the issuer commits it: the invitee's group create (which
/// depends on the genesis) and the admission that depends on `dependencies`
/// plus that group create.
pub(super) fn admit(
    manager: &SecretKey,
    invitee: &SecretKey,
    workspace_id: WorkspaceId,
    genesis: MembershipOperationId,
    timestamp: i64,
    mut dependencies: Vec<MembershipOperationId>,
    nonce: u8,
) -> (MembershipEnvelope, MembershipEnvelope) {
    let token = sign_token(
        manager,
        workspace_id,
        timestamp - 1,
        timestamp + 100_000,
        [nonce; 32],
    )
    .unwrap();
    let claim = sign_claim(invitee, &token, genesis).unwrap();
    let target = claim.invitee;
    let create_person = claim.create_person.clone();
    dependencies.push(create_person.id);
    let admission = MembershipEnvelope::sign(
        manager,
        workspace_id,
        timestamp,
        MembershipAction::Admit {
            target,
            proof: Box::new(AdmissionProof { claim }),
        },
        dependencies,
    )
    .unwrap();
    (create_person, admission)
}

/// The genesis identifier of a runtime's Workspace.
pub(super) fn runtime_genesis(
    runtime: &CoreRuntime,
    workspace_id: WorkspaceId,
) -> MembershipOperationId {
    runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .memberships
        .get(&workspace_id)
        .unwrap()
        .genesis_id()
}

pub(super) struct AdjustableClock(pub(super) Arc<AtomicI64>);

impl Clock for AdjustableClock {
    fn now_unix_ms(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}
