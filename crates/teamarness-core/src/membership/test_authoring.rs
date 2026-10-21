//! Signs membership operations with test-held keys, so tests in other crates
//! can exercise replay authority and the runtime's application path with
//! actors Core itself would never let them impersonate. Enabled only through
//! the `test-hooks` feature; nothing here ships in the application.

use std::collections::BTreeMap;

use iroh_base::SecretKey;

use crate::error::MembershipError;
use crate::{MembershipOperationId, MembershipSnapshot, PublicIdentity, WorkspaceId};

use super::{AdmissionProof, AgentKeyProof, MembershipAction, MembershipEnvelope};

/// A membership action described with test-held secrets.
#[doc(hidden)]
pub enum TestAction {
    Create,
    CreatePerson,
    AttachPerson {
        group: MembershipOperationId,
    },
    /// An admission: `invitee_secret` claims a token the author issues over
    /// `[issued_at, expires_at]` with `nonce`. The admission depends on the
    /// invitee's group create, which `sign_admission` returns beside it.
    Revoke {
        target: PublicIdentity,
        group: MembershipOperationId,
    },
    AddAgentKey {
        group: MembershipOperationId,
        agent_secret: [u8; 32],
    },
    /// An add with an arbitrary proof, for forgery tests.
    AddAgentKeyWithProof {
        group: MembershipOperationId,
        agent: PublicIdentity,
        proof: [u8; 64],
    },
    RemoveAgentKey {
        group: MembershipOperationId,
        agent: PublicIdentity,
    },
}

fn secret(bytes: &[u8; 32]) -> SecretKey {
    SecretKey::from_bytes(bytes)
}

/// The public identity of a test-held secret.
#[doc(hidden)]
#[must_use]
pub fn identity_of(secret_bytes: &[u8; 32]) -> PublicIdentity {
    PublicIdentity::from_bytes(*secret(secret_bytes).public().as_bytes())
        .expect("a generated key is a valid identity")
}

/// Signs one operation and returns its canonical bytes.
///
/// # Panics
///
/// Panics when the action cannot be encoded, which is a test construction error.
#[doc(hidden)]
#[must_use]
pub fn sign_operation(
    author_secret: &[u8; 32],
    workspace_id: WorkspaceId,
    authored_at_unix_ms: i64,
    action: TestAction,
    dependencies: &[MembershipOperationId],
) -> Vec<u8> {
    try_sign_operation(
        author_secret,
        workspace_id,
        authored_at_unix_ms,
        action,
        dependencies,
    )
    .expect("a test operation signs")
}

/// Signs one operation, reporting why the codec refused it instead of
/// panicking, for tests that build deliberately invalid operations.
///
/// # Errors
///
/// Returns the codec's verdict, for example an invalid agent-key proof.
#[doc(hidden)]
pub fn try_sign_operation(
    author_secret: &[u8; 32],
    workspace_id: WorkspaceId,
    authored_at_unix_ms: i64,
    action: TestAction,
    dependencies: &[MembershipOperationId],
) -> Result<Vec<u8>, MembershipError> {
    let author = secret(author_secret);
    let steward = identity_of(author_secret);
    let action = match action {
        TestAction::Create => MembershipAction::Create,
        TestAction::CreatePerson => MembershipAction::CreatePerson,
        TestAction::AttachPerson { group } => MembershipAction::AttachPerson { group },
        TestAction::Revoke { target, group } => MembershipAction::Revoke { target, group },
        TestAction::AddAgentKey {
            group,
            agent_secret,
        } => MembershipAction::AddAgentKey {
            group,
            agent: identity_of(&agent_secret),
            proof: AgentKeyProof::sign(&secret(&agent_secret), workspace_id, group, steward)
                .expect("an agent proof signs"),
        },
        TestAction::AddAgentKeyWithProof {
            group,
            agent,
            proof,
        } => MembershipAction::AddAgentKey {
            group,
            agent,
            proof: AgentKeyProof::from_bytes(proof),
        },
        TestAction::RemoveAgentKey { group, agent } => {
            MembershipAction::RemoveAgentKey { group, agent }
        }
    };
    MembershipEnvelope::sign(
        &author,
        workspace_id,
        authored_at_unix_ms,
        action,
        dependencies.to_vec(),
    )
    .map(|envelope| envelope.canonical_bytes().to_vec())
}

/// Signs an agent-key proof for `agent_secret` over the given binding, so a
/// test can present a proof whose binding differs from the operation's.
#[doc(hidden)]
#[must_use]
pub fn sign_agent_key_proof(
    agent_secret: &[u8; 32],
    workspace_id: WorkspaceId,
    group: MembershipOperationId,
    steward: PublicIdentity,
) -> [u8; 64] {
    AgentKeyProof::sign(&secret(agent_secret), workspace_id, group, steward)
        .expect("an agent proof signs")
        .to_bytes()
}

/// Signs the invitee's group create and the issuer's admission of it.
///
/// Returns `(create_person, admit)` canonical bytes. The admission depends on
/// `dependencies` plus the group create.
#[doc(hidden)]
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn sign_admission(
    issuer_secret: &[u8; 32],
    invitee_secret: &[u8; 32],
    workspace_id: WorkspaceId,
    genesis_id: MembershipOperationId,
    issued_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    nonce: [u8; 32],
    authored_at_unix_ms: i64,
    dependencies: &[MembershipOperationId],
) -> (Vec<u8>, Vec<u8>) {
    let token = crate::invitation::sign_token(
        &secret(issuer_secret),
        workspace_id,
        issued_at_unix_ms,
        expires_at_unix_ms,
        nonce,
    )
    .expect("a test token signs");
    let claim = crate::invitation::sign_claim(&secret(invitee_secret), &token, genesis_id)
        .expect("a test claim signs");
    let create_person = claim.create_person.canonical_bytes().to_vec();
    let mut admit_dependencies = dependencies.to_vec();
    admit_dependencies.push(claim.create_person.id);
    let admit = MembershipEnvelope::sign(
        &secret(issuer_secret),
        workspace_id,
        authored_at_unix_ms,
        MembershipAction::Admit {
            target: claim.invitee,
            proof: Box::new(AdmissionProof { claim }),
        },
        admit_dependencies,
    )
    .expect("a test admission signs")
    .canonical_bytes()
    .to_vec();
    (create_person, admit)
}

/// The content-derived identifier of canonical operation bytes.
///
/// # Panics
///
/// Panics when the bytes are not an authentic envelope.
#[doc(hidden)]
#[must_use]
pub fn operation_id(canonical_envelope: &[u8]) -> MembershipOperationId {
    super::authenticate(canonical_envelope)
        .expect("test operation bytes authenticate")
        .id
}

/// Replays an operation set from genesis without storage, exactly as the
/// runtime does before it accepts a candidate.
///
/// # Errors
///
/// Returns the first replay error, which is the verdict a runtime would give.
#[doc(hidden)]
pub fn replay_for_tests(
    workspace_id: WorkspaceId,
    creator: PublicIdentity,
    operations: &[Vec<u8>],
) -> Result<MembershipSnapshot, MembershipError> {
    let mut map = BTreeMap::new();
    let mut genesis_id = None;
    for bytes in operations {
        let envelope = super::authenticate(bytes)?;
        if matches!(envelope.action, MembershipAction::Create) {
            genesis_id = Some(envelope.id);
        }
        map.insert(envelope.id, envelope);
    }
    let genesis_id = genesis_id.ok_or(MembershipError::ConflictingGenesis)?;
    super::projection::replay(workspace_id, creator, genesis_id, map)
        .map(|projection| projection.snapshot())
}
