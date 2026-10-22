use super::*;
use iroh_base::{SecretKey, Signature};
use rusqlite::TransactionBehavior;

use crate::encoding::ByteReader;

const VERSION: u8 = 2;
const SIGNING_CONTEXT: &str = "teamarness.principal-name-edit/v2";
const ID_DOMAIN: &[u8] = b"teamarness.principal-name-edit-id/v2";
const EDIT_NAME_OFFSET: usize = 1 + 16 + 32 + 32 + 8 + 2;
pub(crate) const MAX_EDIT_BYTES: usize = EDIT_NAME_OFFSET + 256 + 32 + 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditError {
    InvalidEdit,
    Unauthorized,
    Storage(StorageError),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEdit => formatter.write_str("invalid Person name edit"),
            Self::Unauthorized => formatter.write_str("unauthorized Person name edit"),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

/// Canonical, independently signed semantic edit; the record transport is not its authority.
pub(crate) struct PrincipalNameEdit {
    workspace: WorkspaceId,
    person: PrincipalId,
    author: PublicIdentity,
    counter: u64,
    name: String,
    id: [u8; 32],
    bytes: Vec<u8>,
}

impl PrincipalNameEdit {
    pub(crate) fn sign(
        secret: &SecretKey,
        workspace: WorkspaceId,
        person: PrincipalId,
        counter: u64,
        name: &str,
    ) -> Result<Self, EditError> {
        validate(name).map_err(|_| EditError::InvalidEdit)?;
        if counter == 0 || counter >= i64::MAX as u64 {
            return Err(EditError::InvalidEdit);
        }
        let mut unsigned = Vec::with_capacity(EDIT_NAME_OFFSET + name.len());
        unsigned.push(VERSION);
        unsigned.extend_from_slice(workspace.as_bytes());
        unsigned.extend_from_slice(person.as_bytes());
        unsigned.extend_from_slice(secret.public().as_bytes());
        unsigned.extend_from_slice(&counter.to_be_bytes());
        unsigned.extend_from_slice(&(name.len() as u16).to_be_bytes());
        unsigned.extend_from_slice(name.as_bytes());
        let id = edit_id(&unsigned);
        let signature = crate::identity::sign(secret, SIGNING_CONTEXT, &unsigned)
            .map_err(|_| EditError::InvalidEdit)?;
        unsigned.extend_from_slice(&id);
        unsigned.extend_from_slice(&signature.to_bytes());
        Self::decode(&unsigned)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, EditError> {
        if !(EDIT_NAME_OFFSET + 1 + 32 + 64..=MAX_EDIT_BYTES).contains(&bytes.len()) {
            return Err(EditError::InvalidEdit);
        }
        let mut reader = ByteReader::new(bytes);
        if reader.take_u8() != Some(VERSION) {
            return Err(EditError::InvalidEdit);
        }
        let workspace = WorkspaceId::from_bytes(reader.take_array().ok_or(EditError::InvalidEdit)?);
        let person = PrincipalId(reader.take_array().ok_or(EditError::InvalidEdit)?);
        let author = PublicIdentity::from_bytes(reader.take_array().ok_or(EditError::InvalidEdit)?)
            .map_err(|_| EditError::InvalidEdit)?;
        let counter = u64::from_be_bytes(reader.take_array().ok_or(EditError::InvalidEdit)?);
        if counter == 0 || counter >= i64::MAX as u64 {
            return Err(EditError::InvalidEdit);
        }
        let name_len =
            u16::from_be_bytes(reader.take_array().ok_or(EditError::InvalidEdit)?) as usize;
        if name_len > 256 || reader.remaining() != name_len + 32 + 64 {
            return Err(EditError::InvalidEdit);
        }
        let name = std::str::from_utf8(reader.take(name_len).ok_or(EditError::InvalidEdit)?)
            .map_err(|_| EditError::InvalidEdit)?;
        validate(name).map_err(|_| EditError::InvalidEdit)?;
        let unsigned_len = EDIT_NAME_OFFSET + name_len;
        let id = reader.take_array().ok_or(EditError::InvalidEdit)?;
        let signature = Signature::from_bytes(&reader.take_array().ok_or(EditError::InvalidEdit)?);
        reader.finish().ok_or(EditError::InvalidEdit)?;
        if id != edit_id(&bytes[..unsigned_len]) {
            return Err(EditError::InvalidEdit);
        }
        crate::identity::verify(
            author.as_public_key(),
            SIGNING_CONTEXT,
            &bytes[..unsigned_len],
            &signature,
        )
        .map_err(|_| EditError::InvalidEdit)?;
        Ok(Self {
            workspace,
            person,
            author,
            counter,
            name: name.to_owned(),
            id,
            bytes: bytes.to_vec(),
        })
    }

    pub(crate) fn author(&self) -> PublicIdentity {
        self.author
    }
    pub(crate) fn target(&self) -> PrincipalId {
        self.person
    }
    pub(crate) fn workspace(&self) -> WorkspaceId {
        self.workspace
    }
    #[cfg(test)]
    pub(crate) fn id(&self) -> [u8; 32] {
        self.id
    }
    pub(crate) fn canonical_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn authorized_author(
    projection: &WorkspaceProjection,
    id: PrincipalId,
    author: PublicIdentity,
) -> bool {
    let rows = registry(projection);
    rows.iter().any(|(candidate, key, _, steward, _)| {
        *candidate == id
            && if let Some(steward) = steward {
                rows.iter().any(|(principal_id, person_key, _, _, _)| {
                    principal_id == steward && *person_key == author
                })
            } else {
                *key == author
            }
    })
}

/// Only an authenticated current Person's edit for an as-yet-unclaimed target
/// can wait for membership. Historical and currently known targets are terminal.
pub(crate) fn is_early_target(projection: &WorkspaceProjection, edit: &PrincipalNameEdit) -> bool {
    edit.workspace == projection.workspace_id()
        && !projection.principal_history().contains(&edit.person)
        && registry(projection)
            .iter()
            .any(|(_, key, kind, _, _)| *key == edit.author && *kind == PrincipalKind::Person)
}

fn edit_id(unsigned: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(ID_DOMAIN);
    hash.update(unsigned);
    hash.finalize().into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingNameIntent {
    pub(crate) name: String,
    pub(crate) revision: i64,
}

pub(crate) fn pending_intent(
    connection: &Connection,
    workspace: WorkspaceId,
    person: PrincipalId,
) -> Result<Option<PendingNameIntent>, EditError> {
    connection.query_row(
        "SELECT name, intent_revision FROM pending_principal_name_edits WHERE workspace_id = ?1 AND principal_id = ?2",
        params![workspace.as_bytes().as_slice(), person.as_bytes().as_slice()],
        |row| Ok(PendingNameIntent { name: row.get(0)?, revision: row.get(1)? }),
    ).optional().map_err(|_| EditError::Storage(StorageError::Database))
}

fn stored_winner(
    connection: &Connection,
    workspace: WorkspaceId,
    person: PrincipalId,
) -> Result<Option<PrincipalNameEdit>, EditError> {
    let row: Option<(i64, Vec<u8>, Vec<u8>)> = connection.query_row(
        "SELECT counter, edit_id, canonical_edit FROM principal_name_winners WHERE workspace_id = ?1 AND principal_id = ?2",
        params![workspace.as_bytes().as_slice(), person.as_bytes().as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(|_| EditError::Storage(StorageError::Database))?;
    row.map(|(counter, id, bytes)| {
        let winner = PrincipalNameEdit::decode(&bytes)
            .map_err(|_| EditError::Storage(StorageError::CorruptWorkspace))?;
        if winner.workspace != workspace
            || winner.person != person
            || counter < 1
            || winner.counter != counter as u64
            || id.as_slice() != winner.id
        {
            return Err(EditError::Storage(StorageError::CorruptWorkspace));
        }
        Ok(winner)
    })
    .transpose()
}

/// Returns true only when this authenticated edit changes the materialized winner.
/// Dominated edits remain in the immutable record log, not in this bounded projection.
pub(crate) fn accept(
    connection: &mut Connection,
    projection: &WorkspaceProjection,
    bytes: &[u8],
) -> Result<bool, EditError> {
    let edit = PrincipalNameEdit::decode(bytes)?;
    if edit.workspace != projection.workspace_id()
        || !authorized_author(projection, edit.person, edit.author)
    {
        return Err(EditError::Unauthorized);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| EditError::Storage(StorageError::Database))?;
    let old = stored_winner(&transaction, edit.workspace, edit.person)?;
    if old
        .as_ref()
        .is_some_and(|winner| (winner.counter, winner.id) >= (edit.counter, edit.id))
    {
        return Ok(false);
    }
    transaction.execute(
        "INSERT INTO principal_name_winners (workspace_id, principal_id, counter, edit_id, canonical_edit) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, principal_id) DO UPDATE SET counter = excluded.counter, edit_id = excluded.edit_id, canonical_edit = excluded.canonical_edit",
        params![edit.workspace.as_bytes().as_slice(), edit.person.as_bytes().as_slice(), edit.counter as i64, edit.id.as_slice(), bytes],
    ).map_err(|_| EditError::Storage(StorageError::Database))?;
    // A locally selected name remains visible while its pending edit awaits durable append.
    let pending: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM pending_principal_name_edits WHERE workspace_id = ?1 AND principal_id = ?2)",
        params![edit.workspace.as_bytes().as_slice(), edit.person.as_bytes().as_slice()],
        |row| row.get(0),
    ).map_err(|_| EditError::Storage(StorageError::Database))?;
    if !pending {
        let (_, owner, _, _, operation) = registry(projection)
            .into_iter()
            .find(|(id, _, _, _, _)| *id == edit.person)
            .ok_or(EditError::Unauthorized)?;
        let changed = transaction.execute(
            "INSERT INTO principal_names (workspace_id, principal_id, anchor_operation_id, owner_public_identity, name) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(workspace_id, principal_id) DO UPDATE SET name = excluded.name
             WHERE anchor_operation_id = excluded.anchor_operation_id AND owner_public_identity = excluded.owner_public_identity",
            params![edit.workspace.as_bytes().as_slice(), edit.person.as_bytes().as_slice(), operation.as_bytes().as_slice(), owner.as_bytes().as_slice(), edit.name],
        ).map_err(|_| EditError::Storage(StorageError::Database))?;
        if changed != 1 {
            return Err(EditError::Storage(StorageError::CorruptWorkspace));
        }
        advance_generation(&transaction, edit.workspace)?;
    }
    transaction
        .commit()
        .map_err(|_| EditError::Storage(StorageError::UncertainOutcome))?;
    Ok(true)
}

/// Integration calls only after `edit` is durably appended to the private record log.
/// A newer local intent is never cleared by an older append completion.
pub(crate) fn complete_pending(
    connection: &mut Connection,
    projection: &WorkspaceProjection,
    edit: &PrincipalNameEdit,
    intent_revision: i64,
) -> Result<bool, EditError> {
    if edit.workspace != projection.workspace_id()
        || !authorized_author(projection, edit.person, edit.author)
    {
        return Err(EditError::Unauthorized);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| EditError::Storage(StorageError::Database))?;
    let pending = pending_intent(&transaction, edit.workspace, edit.person)?;
    // Check metadata even for a stale completion; corrupt projection state must fail closed.
    let winner = stored_winner(&transaction, edit.workspace, edit.person)?;
    if pending
        .as_ref()
        .is_none_or(|intent| intent.revision != intent_revision || intent.name != edit.name)
    {
        return Ok(false);
    }
    let Some(winner) = winner else {
        return Err(EditError::Storage(StorageError::CorruptWorkspace));
    };
    if (winner.counter, winner.id) != (edit.counter, edit.id) {
        // The intent lost an ordering race. Keep it for re-signing at next_counter.
        return Ok(false);
    }
    transaction
        .execute(
            "DELETE FROM pending_principal_name_edits WHERE workspace_id = ?1 AND principal_id = ?2",
            params![
                edit.workspace.as_bytes().as_slice(),
                edit.person.as_bytes().as_slice()
            ],
        )
        .map_err(|_| EditError::Storage(StorageError::Database))?;
    let (_, owner, _, _, operation) = registry(projection)
        .into_iter()
        .find(|(id, _, _, _, _)| *id == winner.person)
        .ok_or(EditError::Unauthorized)?;
    if !authorized_author(projection, winner.person, winner.author) {
        return Err(EditError::Unauthorized);
    }
    let changed = transaction.execute(
        "UPDATE principal_names SET name = ?1 WHERE workspace_id = ?2 AND principal_id = ?3 AND anchor_operation_id = ?4 AND owner_public_identity = ?5",
        params![winner.name, edit.workspace.as_bytes().as_slice(), edit.person.as_bytes().as_slice(), operation.as_bytes().as_slice(), owner.as_bytes().as_slice()],
    ).map_err(|_| EditError::Storage(StorageError::Database))?;
    if changed != 1 {
        return Err(EditError::Storage(StorageError::CorruptWorkspace));
    }
    advance_generation(&transaction, edit.workspace)?;
    transaction
        .commit()
        .map_err(|_| EditError::Storage(StorageError::UncertainOutcome))?;
    Ok(true)
}

pub(crate) fn next_counter(
    connection: &Connection,
    workspace: WorkspaceId,
    person: PrincipalId,
) -> Result<u64, EditError> {
    match stored_winner(connection, workspace, person)? {
        Some(winner) if winner.counter < (i64::MAX - 1) as u64 => Ok(winner.counter + 1),
        None => Ok(1),
        _ => Err(EditError::InvalidEdit),
    }
}

pub(crate) fn profile_generation(
    connection: &Connection,
    workspace: WorkspaceId,
) -> Result<u64, EditError> {
    let generation: Option<i64> = connection
        .query_row(
            "SELECT generation FROM principal_name_generations WHERE workspace_id = ?1",
            [workspace.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| EditError::Storage(StorageError::Database))?;
    match generation {
        None => Ok(0),
        Some(value) if value >= 0 => Ok(value as u64),
        _ => Err(EditError::Storage(StorageError::CorruptWorkspace)),
    }
}

pub(crate) fn advance_generation(
    connection: &Connection,
    workspace: WorkspaceId,
) -> Result<(), EditError> {
    let written = connection
        .execute(
            "INSERT INTO principal_name_generations (workspace_id, generation) VALUES (?1, 1)
         ON CONFLICT(workspace_id) DO UPDATE SET generation = generation + 1
         WHERE generation < 9223372036854775807",
            [workspace.as_bytes().as_slice()],
        )
        .map_err(|_| EditError::Storage(StorageError::Database))?;
    if written != 1 {
        return Err(EditError::Storage(StorageError::CorruptWorkspace));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::{MembershipAction, MembershipEnvelope};
    use iroh_base::SecretKey;
    use std::collections::BTreeMap;

    fn fixture() -> (Connection, WorkspaceProjection, SecretKey, PrincipalId) {
        let db = crate::storage::memory_schema_for_test();
        let key = SecretKey::from_bytes(&[31; 32]);
        let workspace = WorkspaceId::from_bytes([51; 16]);
        let genesis =
            MembershipEnvelope::sign(&key, workspace, 1, MembershipAction::Create, vec![]).unwrap();
        let author = PublicIdentity::from_bytes(*key.public().as_bytes()).unwrap();
        let group = MembershipEnvelope::sign(
            &key,
            workspace,
            1,
            MembershipAction::CreatePerson { creator: author },
            vec![genesis.id],
        )
        .unwrap();
        let attach = MembershipEnvelope::sign(
            &key,
            workspace,
            1,
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
        let person = derive(workspace, group.id);
        (db, projection, key, person)
    }

    #[test]
    fn early_target_requires_current_person_and_unknown_membership_anchor() {
        let (_, projection, steward, person) = fixture();
        let workspace = projection.workspace_id();
        let unknown = derive_agent(workspace, MembershipOperationId::digest(b"future-agent"));
        let valid = PrincipalNameEdit::sign(&steward, workspace, unknown, 1, "Waiting").unwrap();
        assert!(is_early_target(&projection, &valid));
        let outsider = SecretKey::from_bytes(&[43; 32]);
        let unauthorized =
            PrincipalNameEdit::sign(&outsider, workspace, unknown, 1, "Wrong").unwrap();
        assert!(!is_early_target(&projection, &unauthorized));
        let known = PrincipalNameEdit::sign(&steward, workspace, person, 1, "Known").unwrap();
        assert!(!is_early_target(&projection, &known));
        let other = PrincipalNameEdit::sign(
            &steward,
            WorkspaceId::from_bytes([52; 16]),
            unknown,
            1,
            "Other",
        )
        .unwrap();
        assert!(!is_early_target(&projection, &other));
    }

    #[test]
    fn agent_name_edit_requires_steward_signature_not_agent_or_manager() {
        let (mut db, base, steward, _) = fixture();
        let workspace = base.workspace_id();
        let creator = base.creator();
        let group = base.person_group(creator).unwrap();
        let agent_secret = SecretKey::from_bytes(&[41; 32]);
        let agent = PublicIdentity::from_bytes(*agent_secret.public().as_bytes()).unwrap();
        let proof =
            crate::membership::AgentKeyProof::sign(&agent_secret, workspace, group, creator)
                .unwrap();
        let add = MembershipEnvelope::sign(
            &steward,
            workspace,
            2,
            MembershipAction::AddAgentKey {
                group,
                agent,
                proof,
            },
            base.heads(),
        )
        .unwrap();
        let id = derive_agent(workspace, add.id);
        let genesis =
            MembershipEnvelope::sign(&steward, workspace, 1, MembershipAction::Create, vec![])
                .unwrap();
        let created = MembershipEnvelope::sign(
            &steward,
            workspace,
            1,
            MembershipAction::CreatePerson { creator },
            vec![genesis.id],
        )
        .unwrap();
        let attached = MembershipEnvelope::sign(
            &steward,
            workspace,
            1,
            MembershipAction::AttachPerson { group },
            vec![group],
        )
        .unwrap();
        let operations = BTreeMap::from([
            (genesis.id, genesis),
            (created.id, created),
            (attached.id, attached),
            (add.id, add),
        ]);
        let projection = WorkspaceProjection::for_replication_test(
            workspace,
            creator,
            base.genesis_id(),
            operations,
            vec![creator, agent],
        );
        let owner = PrincipalNameEdit::sign(&steward, workspace, id, 1, "Agent").unwrap();
        let agent_edit =
            PrincipalNameEdit::sign(&agent_secret, workspace, id, 2, "Forged").unwrap();
        let manager = SecretKey::from_bytes(&[42; 32]);
        let manager_edit = PrincipalNameEdit::sign(&manager, workspace, id, 2, "Forged").unwrap();
        assert_eq!(
            accept(&mut db, &projection, agent_edit.canonical_bytes()),
            Err(EditError::Unauthorized)
        );
        assert_eq!(
            accept(&mut db, &projection, manager_edit.canonical_bytes()),
            Err(EditError::Unauthorized)
        );
        assert!(accept(&mut db, &projection, owner.canonical_bytes()).unwrap());
        assert_eq!(
            lookup_id(&db, &projection, id).unwrap().unwrap().name(),
            Some("Agent")
        );
    }

    #[test]
    fn signed_edits_converge_in_any_order_and_persist_high_water_mark() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        let before = projection.snapshot();
        let a = PrincipalNameEdit::sign(&key, workspace, person, 1, "A").unwrap();
        let b = PrincipalNameEdit::sign(&key, workspace, person, 1, "B").unwrap();
        let later = PrincipalNameEdit::sign(&key, workspace, person, 2, "Later").unwrap();
        let (low, high) = if a.id() < b.id() { (a, b) } else { (b, a) };
        assert!(accept(&mut db, &projection, high.canonical_bytes()).unwrap());
        assert!(!accept(&mut db, &projection, low.canonical_bytes()).unwrap());
        assert!(!accept(&mut db, &projection, high.canonical_bytes()).unwrap());
        assert!(accept(&mut db, &projection, later.canonical_bytes()).unwrap());
        assert_eq!(next_counter(&db, workspace, person).unwrap(), 3);
        assert_eq!(
            lookup(&db, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("Later")
        );
        assert_eq!(projection.snapshot(), before);
        assert_eq!(profile_generation(&db, workspace).unwrap(), 2);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM principal_name_winners", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let (mut reordered, projection, _, _) = fixture();
        assert!(accept(&mut reordered, &projection, low.canonical_bytes()).unwrap());
        assert!(accept(&mut reordered, &projection, high.canonical_bytes()).unwrap());
        assert!(accept(&mut reordered, &projection, later.canonical_bytes()).unwrap());
        assert_eq!(
            lookup(&reordered, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("Later")
        );
    }

    #[test]
    fn materialized_winner_and_snapshot_cursor_survive_reopen() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        let edit = PrincipalNameEdit::sign(&key, workspace, person, 1, "Visible").unwrap();
        accept(&mut db, &projection, edit.canonical_bytes()).unwrap();
        let path = std::env::temp_dir().join(format!(
            "person-edit-{}-{:?}.sqlite3",
            std::process::id(),
            std::thread::current().id()
        ));
        db.execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
            .unwrap();
        drop(db);
        let mut db = Connection::open(&path).unwrap();
        assert_eq!(profile_generation(&db, workspace).unwrap(), 1);
        assert_eq!(next_counter(&db, workspace, person).unwrap(), 2);
        assert_eq!(
            lookup(&db, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("Visible")
        );
        assert!(!accept(&mut db, &projection, edit.canonical_bytes()).unwrap());
        let later = PrincipalNameEdit::sign(&key, workspace, person, 2, "New").unwrap();
        assert!(accept(&mut db, &projection, later.canonical_bytes()).unwrap());
        assert_eq!(profile_generation(&db, workspace).unwrap(), 2);
        assert_eq!(
            lookup(&db, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("New")
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn local_pending_name_survives_remote_projection_until_published() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        set(&mut db, &projection, projection.creator(), "Local pending").unwrap();
        assert_eq!(profile_generation(&db, workspace).unwrap(), 1);
        let remote = PrincipalNameEdit::sign(&key, workspace, person, 1, "Older").unwrap();
        assert!(accept(&mut db, &projection, remote.canonical_bytes()).unwrap());
        assert_eq!(
            lookup(&db, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("Local pending")
        );
        assert_eq!(profile_generation(&db, workspace).unwrap(), 1);
        assert_eq!(next_counter(&db, workspace, person).unwrap(), 2);
    }

    #[test]
    fn completing_durable_pending_edit_restores_winner_and_generation() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        set(&mut db, &projection, projection.creator(), "Pending").unwrap();
        let edit = PrincipalNameEdit::sign(&key, workspace, person, 1, "Pending").unwrap();
        let intent = pending_intent(&db, workspace, person).unwrap().unwrap();
        accept(&mut db, &projection, edit.canonical_bytes()).unwrap();
        assert!(complete_pending(&mut db, &projection, &edit, intent.revision).unwrap());
        assert_eq!(profile_generation(&db, workspace).unwrap(), 2);
        assert!(!complete_pending(&mut db, &projection, &edit, intent.revision).unwrap());
        assert_eq!(
            lookup(&db, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("Pending")
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM pending_principal_name_edits",
                [],
                |r| { r.get::<_, i64>(0) }
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn losing_durable_append_retains_pending_name_for_higher_counter_retry() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        set(&mut db, &projection, projection.creator(), "Chosen").unwrap();
        let intent = pending_intent(&db, workspace, person).unwrap().unwrap();
        let local = PrincipalNameEdit::sign(
            &key,
            workspace,
            person,
            next_counter(&db, workspace, person).unwrap(),
            &intent.name,
        )
        .unwrap();
        let remote =
            PrincipalNameEdit::sign(&key, workspace, person, local.counter + 1, "Remote").unwrap();
        assert!(accept(&mut db, &projection, remote.canonical_bytes()).unwrap());
        assert!(!accept(&mut db, &projection, local.canonical_bytes()).unwrap());
        assert!(!complete_pending(&mut db, &projection, &local, intent.revision).unwrap());
        assert_eq!(
            pending_intent(&db, workspace, person).unwrap().unwrap(),
            intent
        );
        assert_eq!(
            lookup(&db, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("Chosen")
        );
        let retry = PrincipalNameEdit::sign(
            &key,
            workspace,
            person,
            next_counter(&db, workspace, person).unwrap(),
            &intent.name,
        )
        .unwrap();
        assert!(retry.counter > remote.counter);
        assert!(accept(&mut db, &projection, retry.canonical_bytes()).unwrap());
        assert!(complete_pending(&mut db, &projection, &retry, intent.revision).unwrap());
        assert!(pending_intent(&db, workspace, person).unwrap().is_none());
    }

    #[test]
    fn old_a_completion_cannot_clear_new_a_intent() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        set(&mut db, &projection, projection.creator(), "A").unwrap();
        let first = pending_intent(&db, workspace, person).unwrap().unwrap();
        let old = PrincipalNameEdit::sign(&key, workspace, person, 1, &first.name).unwrap();
        set(&mut db, &projection, projection.creator(), "B").unwrap();
        set(&mut db, &projection, projection.creator(), "A").unwrap();
        let latest = pending_intent(&db, workspace, person).unwrap().unwrap();
        assert_ne!(first.revision, latest.revision);
        assert_eq!(latest.name, first.name);
        let remote = PrincipalNameEdit::sign(&key, workspace, person, 2, "Remote").unwrap();
        accept(&mut db, &projection, remote.canonical_bytes()).unwrap();
        assert!(!accept(&mut db, &projection, old.canonical_bytes()).unwrap());
        assert!(!complete_pending(&mut db, &projection, &old, first.revision).unwrap());
        assert_eq!(
            pending_intent(&db, workspace, person).unwrap().unwrap(),
            latest
        );
        let retry = PrincipalNameEdit::sign(
            &key,
            workspace,
            person,
            next_counter(&db, workspace, person).unwrap(),
            &latest.name,
        )
        .unwrap();
        accept(&mut db, &projection, retry.canonical_bytes()).unwrap();
        assert!(complete_pending(&mut db, &projection, &retry, latest.revision).unwrap());
        set(&mut db, &projection, projection.creator(), "A").unwrap();
        assert!(
            pending_intent(&db, workspace, person)
                .unwrap()
                .unwrap()
                .revision
                > latest.revision
        );
    }

    #[test]
    fn corrupt_winner_metadata_fails_closed_before_comparison_or_completion() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        set(&mut db, &projection, projection.creator(), "Pending").unwrap();
        let intent = pending_intent(&db, workspace, person).unwrap().unwrap();
        let old = PrincipalNameEdit::sign(&key, workspace, person, 1, "Pending").unwrap();
        accept(&mut db, &projection, old.canonical_bytes()).unwrap();
        db.execute(
            "UPDATE principal_name_winners SET counter = 999 WHERE workspace_id = ?1",
            [workspace.as_bytes().as_slice()],
        )
        .unwrap();
        let new = PrincipalNameEdit::sign(&key, workspace, person, 2, "New").unwrap();
        let corrupt = Err(EditError::Storage(StorageError::CorruptWorkspace));
        assert_eq!(accept(&mut db, &projection, new.canonical_bytes()), corrupt);
        assert_eq!(
            complete_pending(&mut db, &projection, &old, intent.revision),
            corrupt
        );
        assert_eq!(next_counter(&db, workspace, person), corrupt.map(|_| 0));
        assert_eq!(
            pending_intent(&db, workspace, person).unwrap().unwrap(),
            intent
        );
        db.execute(
            "UPDATE principal_name_winners SET counter = 1, edit_id = zeroblob(32)",
            [],
        )
        .unwrap();
        assert_eq!(accept(&mut db, &projection, new.canonical_bytes()), corrupt);
        let other_workspace = PrincipalNameEdit::sign(
            &key,
            WorkspaceId::from_bytes([52; 16]),
            person,
            1,
            "Pending",
        )
        .unwrap();
        db.execute(
            "UPDATE principal_name_winners SET edit_id = ?1, canonical_edit = ?2",
            params![old.id.as_slice(), other_workspace.canonical_bytes()],
        )
        .unwrap();
        assert_eq!(next_counter(&db, workspace, person), corrupt.map(|_| 0));
        assert_eq!(
            pending_intent(&db, workspace, person).unwrap().unwrap(),
            intent
        );
    }

    #[test]
    fn concurrent_accepts_serialize_on_one_durable_winner() {
        let (db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        let path = std::env::temp_dir().join(format!(
            "person-concurrent-{}-{:?}.sqlite3",
            std::process::id(),
            std::thread::current().id()
        ));
        db.execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
            .unwrap();
        drop(db);
        let low = PrincipalNameEdit::sign(&key, workspace, person, 1, "Low").unwrap();
        let high = PrincipalNameEdit::sign(&key, workspace, person, 2, "High").unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                let mut db = Connection::open(&path).unwrap();
                db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
                barrier.wait();
                accept(&mut db, &projection, low.canonical_bytes()).unwrap()
            });
            let second = scope.spawn(|| {
                let mut db = Connection::open(&path).unwrap();
                db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
                barrier.wait();
                accept(&mut db, &projection, high.canonical_bytes()).unwrap()
            });
            first.join().unwrap();
            second.join().unwrap();
        });
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            lookup(&db, &projection, projection.creator())
                .unwrap()
                .unwrap()
                .name(),
            Some("High")
        );
        assert_eq!(next_counter(&db, workspace, person).unwrap(), 3);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn uncertain_append_does_not_discard_durable_pending_intent() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        set(&mut db, &projection, projection.creator(), "Chosen").unwrap();
        let intent = pending_intent(&db, workspace, person).unwrap().unwrap();
        let signed = PrincipalNameEdit::sign(
            &key,
            workspace,
            person,
            next_counter(&db, workspace, person).unwrap(),
            &intent.name,
        )
        .unwrap();
        // No append acknowledgement: do not complete, even if signing succeeded.
        let path = std::env::temp_dir().join(format!(
            "person-intent-{}-{:?}.sqlite3",
            std::process::id(),
            std::thread::current().id()
        ));
        db.execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
            .unwrap();
        drop(db);
        let mut db = Connection::open(&path).unwrap();
        assert_eq!(
            pending_intent(&db, workspace, person).unwrap().unwrap(),
            intent
        );
        assert_eq!(
            next_counter(&db, workspace, person).unwrap(),
            signed.counter
        );
        accept(&mut db, &projection, signed.canonical_bytes()).unwrap();
        assert!(complete_pending(&mut db, &projection, &signed, intent.revision).unwrap());
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_or_unmapped_edits_do_not_poison_valid_edits() {
        let (mut db, projection, key, person) = fixture();
        let workspace = projection.workspace_id();
        let good = PrincipalNameEdit::sign(&key, workspace, person, 1, "🧑‍💻").unwrap();
        let other = SecretKey::from_bytes(&[32; 32]);
        let wrong_author = PrincipalNameEdit::sign(&other, workspace, person, 2, "Wrong").unwrap();
        let wrong_person = PrincipalNameEdit::sign(
            &key,
            workspace,
            derive(WorkspaceId::from_bytes([52; 16]), projection.genesis_id()),
            2,
            "Wrong",
        )
        .unwrap();
        let wrong_workspace =
            PrincipalNameEdit::sign(&key, WorkspaceId::from_bytes([52; 16]), person, 2, "Wrong")
                .unwrap();
        for bytes in [
            wrong_author.canonical_bytes(),
            wrong_person.canonical_bytes(),
            wrong_workspace.canonical_bytes(),
        ] {
            assert!(accept(&mut db, &projection, bytes).is_err());
        }
        let mut forged = good.canonical_bytes().to_vec();
        *forged.last_mut().unwrap() ^= 1;
        assert!(accept(&mut db, &projection, &forged).is_err());
        assert!(accept(&mut db, &projection, &vec![0; MAX_EDIT_BYTES + 1]).is_err());
        let mut invalid_utf8 = good.canonical_bytes().to_vec();
        invalid_utf8[EDIT_NAME_OFFSET] = 0xff;
        assert!(accept(&mut db, &projection, &invalid_utf8).is_err());
        assert!(PrincipalNameEdit::sign(&key, workspace, person, 2, "x\n").is_err());
        assert!(PrincipalNameEdit::sign(&key, workspace, person, 2, "x\u{200b}").is_err());
        assert_eq!(profile_generation(&db, workspace).unwrap(), 0);
        let revoked = WorkspaceProjection::for_replication_test(
            workspace,
            projection.creator(),
            projection.genesis_id(),
            BTreeMap::new(),
            vec![],
        );
        assert_eq!(
            accept(&mut db, &revoked, good.canonical_bytes()),
            Err(EditError::Unauthorized)
        );
        assert!(accept(&mut db, &projection, good.canonical_bytes()).unwrap());
    }
}
