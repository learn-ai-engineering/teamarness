//! Workspace-local identity presentation, derived only from canonical membership anchors.
pub(crate) mod edit;

use std::fmt;
use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UnicodeSegmentation;

use crate::membership::WorkspaceProjection;
use crate::{MembershipOperationId, PublicIdentity, StorageError, ValueParseError, WorkspaceId};

const PERSON_DOMAIN: &[u8] = b"teamarness.workspace-person/v1";

/// Opaque Workspace-local identifier, independent of an Installation key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspacePersonId([u8; 32]);

impl WorkspacePersonId {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Display for WorkspacePersonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::encoding::write_hex(&self.0, f)
    }
}
impl FromStr for WorkspacePersonId {
    type Err = ValueParseError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        crate::encoding::hex_decode_fixed(value)
            .map(Self)
            .ok_or_else(|| ValueParseError::new("Workspace Person ID"))
    }
}

/// Current member presentation. An unnamed older Workspace has a fingerprint fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspacePerson {
    id: WorkspacePersonId,
    name: Option<String>,
    fingerprint: String,
}
impl WorkspacePerson {
    pub const fn id(&self) -> WorkspacePersonId {
        self.id
    }
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// A rejected name, an unauthorized owner, or a durable storage failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonNameError {
    InvalidName,
    WorkspaceNotFound,
    NotOwnPerson,
    Storage(StorageError),
}
impl fmt::Display for PersonNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("invalid Workspace Person name"),
            Self::WorkspaceNotFound => f.write_str("Workspace not found"),
            Self::NotOwnPerson => f.write_str("only the current member may set their own name"),
            Self::Storage(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for PersonNameError {}

/// Reject format controls except U+200D (ZWJ, needed inside emoji sequences).
/// Variation selectors and combining marks are not controls. Do not normalize
/// user text: trim is validation, not silent mutation of the chosen name.
pub(crate) fn validate(name: &str) -> Result<(), PersonNameError> {
    if name.trim().is_empty()
        || name.trim() != name
        || name.len() > 256
        || UnicodeSegmentation::graphemes(name, true).count() > 64
        || !name.chars().any(|c| {
            matches!(
                get_general_category(c),
                GeneralCategory::UppercaseLetter
                    | GeneralCategory::LowercaseLetter
                    | GeneralCategory::TitlecaseLetter
                    | GeneralCategory::ModifierLetter
                    | GeneralCategory::OtherLetter
                    | GeneralCategory::DecimalNumber
                    | GeneralCategory::LetterNumber
                    | GeneralCategory::OtherNumber
                    | GeneralCategory::OtherSymbol
                    | GeneralCategory::MathSymbol
                    | GeneralCategory::CurrencySymbol
                    | GeneralCategory::ModifierSymbol
                    | GeneralCategory::OtherPunctuation
                    | GeneralCategory::ConnectorPunctuation
                    | GeneralCategory::DashPunctuation
                    | GeneralCategory::OpenPunctuation
                    | GeneralCategory::ClosePunctuation
                    | GeneralCategory::InitialPunctuation
                    | GeneralCategory::FinalPunctuation
            )
        })
        || name.chars().any(|c| {
            c.is_control()
                || (get_general_category(c) == GeneralCategory::Format && c != '\u{200d}')
        })
    {
        return Err(PersonNameError::InvalidName);
    }
    Ok(())
}

pub(crate) fn anchor(
    projection: &WorkspaceProjection,
    identity: PublicIdentity,
) -> Option<MembershipOperationId> {
    if !projection.contains_member(identity) {
        return None;
    }
    if projection.creator() == identity {
        return Some(projection.genesis_id());
    }
    // Admission is immutable. Causal re-admission is forbidden in membership v1.
    projection.first_admission(identity)
}

pub(crate) fn derive(workspace: WorkspaceId, anchor: MembershipOperationId) -> WorkspacePersonId {
    let mut hasher = Sha256::new();
    hasher.update(PERSON_DOMAIN);
    hasher.update(workspace.as_bytes());
    hasher.update(anchor.as_bytes());
    WorkspacePersonId(hasher.finalize().into())
}

pub(crate) fn lookup(
    connection: &Connection,
    projection: &WorkspaceProjection,
    identity: PublicIdentity,
) -> Result<Option<WorkspacePerson>, PersonNameError> {
    let Some(anchor) = anchor(projection, identity) else {
        return Ok(None);
    };
    let id = derive(projection.workspace_id(), anchor);
    let stored: Option<(Vec<u8>, Vec<u8>, String)> = connection.query_row(
        "SELECT anchor_operation_id, owner_public_identity, name FROM workspace_person_names WHERE workspace_id = ?1 AND person_id = ?2",
        params![projection.workspace_id().as_bytes().as_slice(), id.0.as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    let name = stored
        .map(|(stored_anchor, owner, name)| {
            if stored_anchor != anchor.as_bytes()
                || owner != identity.as_bytes()
                || validate(&name).is_err()
            {
                return Err(PersonNameError::Storage(StorageError::CorruptWorkspace));
            }
            Ok(name)
        })
        .transpose()?;
    Ok(Some(WorkspacePerson {
        id,
        name,
        fingerprint: identity.to_string().chars().take(12).collect(),
    }))
}

pub(crate) fn set(
    connection: &mut Connection,
    projection: &WorkspaceProjection,
    identity: PublicIdentity,
    name: &str,
) -> Result<(), PersonNameError> {
    validate(name)?;
    let anchor = anchor(projection, identity).ok_or(PersonNameError::NotOwnPerson)?;
    let id = derive(projection.workspace_id(), anchor);
    let transaction = connection
        .transaction()
        .map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    let written = transaction.execute(
        "INSERT INTO workspace_person_names (workspace_id, person_id, anchor_operation_id, owner_public_identity, name)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, person_id) DO UPDATE SET name = excluded.name
         WHERE anchor_operation_id = excluded.anchor_operation_id AND owner_public_identity = excluded.owner_public_identity",
        params![projection.workspace_id().as_bytes().as_slice(), id.0.as_slice(), anchor.as_bytes().as_slice(), identity.as_bytes().as_slice(), name],
    ).map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    if written != 1 {
        return Err(PersonNameError::Storage(StorageError::CorruptWorkspace));
    }
    transaction.execute(
        "INSERT INTO pending_person_name_edits (workspace_id, person_id, name) VALUES (?1, ?2, ?3)
         ON CONFLICT(workspace_id, person_id) DO UPDATE SET name = excluded.name",
        params![projection.workspace_id().as_bytes().as_slice(), id.0.as_slice(), name],
    ).map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    edit::advance_generation(&transaction, projection.workspace_id()).map_err(
        |error| match error {
            edit::EditError::Storage(storage) => PersonNameError::Storage(storage),
            _ => PersonNameError::Storage(StorageError::CorruptWorkspace),
        },
    )?;
    transaction
        .commit()
        .map_err(|_| PersonNameError::Storage(StorageError::UncertainOutcome))
}

pub(crate) fn stage_join(
    connection: &mut Connection,
    workspace: WorkspaceId,
    token: &[u8],
    name: &str,
) -> Result<String, PersonNameError> {
    validate(name)?;
    let transaction = connection
        .transaction()
        .map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    transaction.execute(
        "INSERT OR IGNORE INTO pending_person_join_names (workspace_id, invitation_token_id, name) VALUES (?1, ?2, ?3)",
        params![workspace.as_bytes().as_slice(), token, name],
    ).map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    let original = transaction.query_row(
        "SELECT name FROM pending_person_join_names WHERE workspace_id = ?1 AND invitation_token_id = ?2",
        params![workspace.as_bytes().as_slice(), token], |row| row.get(0),
    ).map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    transaction
        .commit()
        .map_err(|_| PersonNameError::Storage(StorageError::UncertainOutcome))?;
    Ok(original)
}

pub(crate) fn finish_join(
    connection: &mut Connection,
    projection: &WorkspaceProjection,
    identity: PublicIdentity,
    token: &[u8],
) -> Result<(), PersonNameError> {
    // Never apply a pending intent to an unrelated admission or a former member.
    let admitted = projection.has_admission_for_token(
        identity,
        token
            .try_into()
            .map_err(|_| PersonNameError::NotOwnPerson)?,
    );
    if !admitted || anchor(projection, identity).is_none() {
        return Err(PersonNameError::NotOwnPerson);
    }
    let pending: Option<String> = connection.query_row(
        "SELECT name FROM pending_person_join_names WHERE workspace_id = ?1 AND invitation_token_id = ?2",
        params![projection.workspace_id().as_bytes().as_slice(), token], |row| row.get(0),
    ).optional().map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    if let Some(name) = pending {
        if lookup(connection, projection, identity)?.is_some_and(|person| person.name().is_none()) {
            set(connection, projection, identity, &name)?;
        }
        connection.execute("DELETE FROM pending_person_join_names WHERE workspace_id = ?1 AND invitation_token_id = ?2",
            params![projection.workspace_id().as_bytes().as_slice(), token])
            .map_err(|_| PersonNameError::Storage(StorageError::Database))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::{AdmissionProof, MembershipAction, MembershipEnvelope};
    use iroh_base::SecretKey;
    use std::collections::BTreeMap;

    #[test]
    fn first_accepted_admission_anchors_person_even_when_later_id_sorts_first() {
        let manager = SecretKey::from_bytes(&[42; 32]);
        let joiner = SecretKey::from_bytes(&[43; 32]);
        let identity = PublicIdentity::from_bytes(*joiner.public().as_bytes()).unwrap();
        let workspace = WorkspaceId::from_bytes([5; 16]);
        let genesis =
            MembershipEnvelope::sign(&manager, workspace, 10, MembershipAction::Create, vec![])
                .unwrap();
        let admission = |nonce: u8, dependencies: Vec<MembershipOperationId>| {
            let token =
                crate::invitation::sign_token(&manager, workspace, 10, 200, [nonce; 32]).unwrap();
            let claim = crate::invitation::sign_claim(&joiner, &token, genesis.id).unwrap();
            MembershipEnvelope::sign(
                &manager,
                workspace,
                100,
                MembershipAction::Admit {
                    target: identity,
                    proof: Box::new(AdmissionProof { claim }),
                },
                dependencies,
            )
            .unwrap()
        };
        let first = admission(1, vec![genesis.id]);
        let second = (2..=255)
            .map(|nonce| admission(nonce, vec![first.id]))
            .find(|operation| operation.id < first.id)
            .expect("an ID that sorts before the first");
        let projection = WorkspaceProjection::for_replication_test(
            workspace,
            PublicIdentity::from_bytes(*manager.public().as_bytes()).unwrap(),
            genesis.id,
            BTreeMap::from([
                (genesis.id, genesis),
                (first.id, first.clone()),
                (second.id, second),
            ]),
            vec![
                PublicIdentity::from_bytes(*manager.public().as_bytes()).unwrap(),
                identity,
            ],
        );
        assert_eq!(anchor(&projection, identity), Some(first.id));
    }
}
