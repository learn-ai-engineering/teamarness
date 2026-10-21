//! Workspace-local identity presentation, derived only from canonical membership anchors.
pub(crate) mod agent_keys;
pub(crate) mod edit;

use std::fmt;
use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UnicodeSegmentation;

use crate::membership::WorkspaceProjection;
use crate::{MembershipOperationId, PublicIdentity, StorageError, ValueParseError, WorkspaceId};

const PERSON_DOMAIN: &[u8] = b"teamarness.workspace-person/v2";
const AGENT_DOMAIN: &[u8] = b"teamarness.workspace-agent/v1";

pub type WorkspacePersonId = PrincipalId;
pub type WorkspacePerson = WorkspacePrincipal;
pub type PersonNameError = PrincipalError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalKind {
    Person,
    Agent,
}

/// Opaque Workspace-local identifier, independent of an Installation key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrincipalId([u8; 32]);

impl PrincipalId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::encoding::write_hex(&self.0, f)
    }
}
impl FromStr for PrincipalId {
    type Err = ValueParseError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        crate::encoding::hex_decode_fixed(value)
            .map(Self)
            .ok_or_else(|| ValueParseError::new("Workspace Person ID"))
    }
}

/// Current member presentation. An unnamed older Workspace has a fingerprint fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspacePrincipal {
    id: PrincipalId,
    name: Option<String>,
    fingerprint: String,
    kind: PrincipalKind,
    steward: Option<PrincipalId>,
    keys: Vec<PublicIdentity>,
}
impl WorkspacePrincipal {
    pub const fn id(&self) -> PrincipalId {
        self.id
    }
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    pub const fn kind(&self) -> PrincipalKind {
        self.kind
    }
    pub const fn steward_principal_id(&self) -> Option<PrincipalId> {
        self.steward
    }
    pub fn keys(&self) -> &[PublicIdentity] {
        &self.keys
    }
}

/// A rejected name, an unauthorized owner, or a durable storage failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalError {
    InvalidName,
    InvalidPackageIdentity,
    WorkspaceNotFound,
    NotOwnPerson,
    AgentNotFound,
    Membership(crate::MembershipError),
    Storage(StorageError),
}
impl fmt::Display for PrincipalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("invalid Principal name"),
            Self::InvalidPackageIdentity => f.write_str("invalid Package Identity"),
            Self::WorkspaceNotFound => f.write_str("Workspace not found"),
            Self::NotOwnPerson => f.write_str("only the steward may set this Principal name"),
            Self::AgentNotFound => f.write_str("Agent not found"),
            Self::Membership(error) => error.fmt(f),
            Self::Storage(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for PrincipalError {}

/// Reject format controls except U+200D (ZWJ, needed inside emoji sequences).
/// Variation selectors and combining marks are not controls. Do not normalize
/// user text: trim is validation, not silent mutation of the chosen name.
pub(crate) fn validate(name: &str) -> Result<(), PrincipalError> {
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
        return Err(PrincipalError::InvalidName);
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
    projection
        .principal_anchors()
        .into_iter()
        .find(|(_, key, steward)| *key == identity && steward.is_none())
        .map(|(id, _, _)| id)
}

pub(crate) fn derive(workspace: WorkspaceId, anchor: MembershipOperationId) -> PrincipalId {
    derive_with_domain(PERSON_DOMAIN, workspace, anchor)
}

pub(crate) fn derive_agent(workspace: WorkspaceId, anchor: MembershipOperationId) -> PrincipalId {
    derive_with_domain(AGENT_DOMAIN, workspace, anchor)
}

fn derive_with_domain(
    domain: &[u8],
    workspace: WorkspaceId,
    anchor: MembershipOperationId,
) -> PrincipalId {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(workspace.as_bytes());
    hasher.update(anchor.as_bytes());
    PrincipalId(hasher.finalize().into())
}

pub(crate) fn registry(
    projection: &WorkspaceProjection,
) -> Vec<(
    PrincipalId,
    PublicIdentity,
    PrincipalKind,
    Option<PrincipalId>,
    MembershipOperationId,
)> {
    let workspace = projection.workspace_id();
    projection
        .principal_anchors()
        .into_iter()
        .map(|(anchor, key, agent)| match agent {
            Some((group, _)) => (
                derive_agent(workspace, anchor),
                key,
                PrincipalKind::Agent,
                Some(derive(workspace, group)),
                anchor,
            ),
            None => (
                derive(workspace, anchor),
                key,
                PrincipalKind::Person,
                None,
                anchor,
            ),
        })
        .collect()
}

pub(crate) fn refresh(
    connection: &Connection,
    projection: &WorkspaceProjection,
) -> Result<(), StorageError> {
    let workspace = projection.workspace_id();
    connection
        .execute(
            "DELETE FROM workspace_principals WHERE workspace_id = ?1",
            [workspace.as_bytes().as_slice()],
        )
        .map_err(|_| StorageError::Database)?;
    for (id, key, kind, steward, anchor) in registry(projection) {
        connection.execute(
            "INSERT INTO workspace_principals (workspace_id,principal_id,kind,steward_principal_id,public_identity,anchor_operation_id) VALUES (?1,?2,?3,?4,?5,?6)",
            params![workspace.as_bytes().as_slice(), id.as_bytes().as_slice(), if kind == PrincipalKind::Person { "person" } else { "agent" }, steward.map(|id| id.0.to_vec()), key.as_bytes().as_slice(), anchor.as_bytes().as_slice()],
        ).map_err(|_| StorageError::Database)?;
    }
    Ok(())
}

pub(crate) fn lookup_id(
    connection: &Connection,
    projection: &WorkspaceProjection,
    id: PrincipalId,
) -> Result<Option<WorkspacePrincipal>, PrincipalError> {
    let Some((_, key, kind, steward, anchor)) = registry(projection)
        .into_iter()
        .find(|(candidate, _, _, _, _)| *candidate == id)
    else {
        return Ok(None);
    };
    let stored: Option<(Vec<u8>, Vec<u8>, String)> = connection.query_row(
        "SELECT anchor_operation_id, owner_public_identity, name FROM principal_names WHERE workspace_id=?1 AND principal_id=?2",
        params![projection.workspace_id().as_bytes().as_slice(), id.0.as_slice()],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional().map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    let name = stored
        .map(|(operation, owner, name)| {
            if operation != anchor.as_bytes() || owner != key.as_bytes() || validate(&name).is_err()
            {
                return Err(PrincipalError::Storage(StorageError::CorruptWorkspace));
            }
            Ok(name)
        })
        .transpose()?;
    Ok(Some(WorkspacePrincipal {
        id,
        name,
        fingerprint: key.to_string().chars().take(12).collect(),
        kind,
        steward,
        keys: vec![key],
    }))
}

pub(crate) fn lookup(
    connection: &Connection,
    projection: &WorkspaceProjection,
    identity: PublicIdentity,
) -> Result<Option<WorkspacePrincipal>, PrincipalError> {
    let Some((id, _, _, _, _)) = registry(projection)
        .into_iter()
        .find(|(_, key, _, _, _)| *key == identity)
    else {
        return Ok(None);
    };
    lookup_id(connection, projection, id)
}

pub(crate) fn set(
    connection: &mut Connection,
    projection: &WorkspaceProjection,
    identity: PublicIdentity,
    name: &str,
) -> Result<(), PrincipalError> {
    let anchor = anchor(projection, identity).ok_or(PrincipalError::NotOwnPerson)?;
    set_id(
        connection,
        projection,
        identity,
        derive(projection.workspace_id(), anchor),
        name,
    )
}

pub(crate) fn set_id(
    connection: &mut Connection,
    projection: &WorkspaceProjection,
    author: PublicIdentity,
    id: PrincipalId,
    name: &str,
) -> Result<(), PrincipalError> {
    validate(name)?;
    let (anchor, identity) = projection
        .principal_anchors()
        .into_iter()
        .find_map(|(anchor, key, agent)| {
            let candidate = match agent {
                Some(_) => derive_agent(projection.workspace_id(), anchor),
                None => derive(projection.workspace_id(), anchor),
            };
            (candidate == id
                && match agent {
                    None => key == author,
                    Some((_, steward)) => steward == author,
                })
            .then_some((anchor, key))
        })
        .ok_or(PrincipalError::NotOwnPerson)?;
    let transaction = connection
        .transaction()
        .map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    let written = transaction.execute(
        "INSERT INTO principal_names (workspace_id, principal_id, anchor_operation_id, owner_public_identity, name)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, principal_id) DO UPDATE SET name = excluded.name
         WHERE anchor_operation_id = excluded.anchor_operation_id AND owner_public_identity = excluded.owner_public_identity",
        params![projection.workspace_id().as_bytes().as_slice(), id.0.as_slice(), anchor.as_bytes().as_slice(), identity.as_bytes().as_slice(), name],
    ).map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    if written != 1 {
        return Err(PrincipalError::Storage(StorageError::CorruptWorkspace));
    }
    transaction.execute(
        "INSERT INTO pending_principal_name_edits (workspace_id, principal_id, name) VALUES (?1, ?2, ?3)
         ON CONFLICT(workspace_id, principal_id) DO UPDATE SET name = excluded.name",
        params![projection.workspace_id().as_bytes().as_slice(), id.0.as_slice(), name],
    ).map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    edit::advance_generation(&transaction, projection.workspace_id()).map_err(
        |error| match error {
            edit::EditError::Storage(storage) => PrincipalError::Storage(storage),
            _ => PrincipalError::Storage(StorageError::CorruptWorkspace),
        },
    )?;
    transaction
        .commit()
        .map_err(|_| PrincipalError::Storage(StorageError::UncertainOutcome))
}

pub(crate) fn stage_join(
    connection: &mut Connection,
    workspace: WorkspaceId,
    token: &[u8],
    name: &str,
) -> Result<String, PrincipalError> {
    validate(name)?;
    let transaction = connection
        .transaction()
        .map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    transaction.execute(
        "INSERT OR IGNORE INTO pending_principal_join_names (workspace_id, invitation_token_id, name) VALUES (?1, ?2, ?3)",
        params![workspace.as_bytes().as_slice(), token, name],
    ).map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    let original = transaction.query_row(
        "SELECT name FROM pending_principal_join_names WHERE workspace_id = ?1 AND invitation_token_id = ?2",
        params![workspace.as_bytes().as_slice(), token], |row| row.get(0),
    ).map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    transaction
        .commit()
        .map_err(|_| PrincipalError::Storage(StorageError::UncertainOutcome))?;
    Ok(original)
}

pub(crate) fn finish_join(
    connection: &mut Connection,
    projection: &WorkspaceProjection,
    identity: PublicIdentity,
    token: &[u8],
) -> Result<(), PrincipalError> {
    // Never apply a pending intent to an unrelated admission or a former member.
    let admitted = projection.has_admission_for_token(
        identity,
        token.try_into().map_err(|_| PrincipalError::NotOwnPerson)?,
    );
    if !admitted || anchor(projection, identity).is_none() {
        return Err(PrincipalError::NotOwnPerson);
    }
    let pending: Option<String> = connection.query_row(
        "SELECT name FROM pending_principal_join_names WHERE workspace_id = ?1 AND invitation_token_id = ?2",
        params![projection.workspace_id().as_bytes().as_slice(), token], |row| row.get(0),
    ).optional().map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    if let Some(name) = pending {
        if lookup(connection, projection, identity)?.is_some_and(|person| person.name().is_none()) {
            set(connection, projection, identity, &name)?;
        }
        connection.execute("DELETE FROM pending_principal_join_names WHERE workspace_id = ?1 AND invitation_token_id = ?2",
            params![projection.workspace_id().as_bytes().as_slice(), token])
            .map_err(|_| PrincipalError::Storage(StorageError::Database))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::{MembershipAction, MembershipEnvelope};
    use iroh_base::SecretKey;
    use std::collections::BTreeMap;

    #[test]
    fn person_id_anchors_to_group_create_not_genesis() {
        let secret = SecretKey::from_bytes(&[42; 32]);
        let author = PublicIdentity::from_bytes(*secret.public().as_bytes()).unwrap();
        let workspace = WorkspaceId::from_bytes([5; 16]);
        let genesis =
            MembershipEnvelope::sign(&secret, workspace, 10, MembershipAction::Create, vec![])
                .unwrap();
        let group = MembershipEnvelope::sign(
            &secret,
            workspace,
            10,
            MembershipAction::CreatePerson { creator: author },
            vec![genesis.id],
        )
        .unwrap();
        let attach = MembershipEnvelope::sign(
            &secret,
            workspace,
            10,
            MembershipAction::AttachPerson { group: group.id },
            vec![group.id],
        )
        .unwrap();
        let projection = WorkspaceProjection::for_replication_test(
            workspace,
            author,
            genesis.id,
            BTreeMap::from([
                (genesis.id, genesis.clone()),
                (group.id, group.clone()),
                (attach.id, attach),
            ]),
            vec![author],
        );
        assert_eq!(anchor(&projection, author), Some(group.id));
        assert_ne!(derive(workspace, group.id), derive(workspace, genesis.id));
        assert_ne!(
            derive_agent(workspace, group.id),
            derive(workspace, group.id)
        );
    }
}
