//! Installation-local Agent signing-key custody and Agent Bindings: the
//! Package-keyed local configuration that makes a stewarded Agent a signer on
//! this Installation (ADR 0030).
use iroh_base::SecretKey;
use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use super::PrincipalId;
use crate::{PublicIdentity, StorageError, WorkspaceId};

pub(crate) fn store(
    connection: &Connection,
    workspace: WorkspaceId,
    id: PrincipalId,
    secret: &SecretKey,
) -> Result<(), StorageError> {
    let bytes = Zeroizing::new(secret.to_bytes());
    connection.execute("INSERT INTO agent_keys (workspace_id, agent_principal_id, public_identity, secret_key) VALUES (?1,?2,?3,?4)",
        params![workspace.as_bytes().as_slice(), id.as_bytes().as_slice(), secret.public().as_bytes().as_slice(), bytes.as_slice()])
        .map_err(|_| StorageError::Database)?;
    Ok(())
}

// Private to Core. An Agent removed by a peer can retain a local custody row,
// but membership must be checked before this key is ever used to sign again.
#[allow(dead_code)]
pub(crate) fn load(
    connection: &Connection,
    workspace: WorkspaceId,
    id: PrincipalId,
    identity: PublicIdentity,
) -> Result<Option<SecretKey>, StorageError> {
    let bytes: Option<Vec<u8>> = connection.query_row(
        "SELECT secret_key FROM agent_keys WHERE workspace_id=?1 AND agent_principal_id=?2 AND public_identity=?3",
        params![workspace.as_bytes().as_slice(), id.as_bytes().as_slice(), identity.as_bytes().as_slice()],
        |row| row.get(0),
    ).optional().map_err(|_| StorageError::Database)?;
    bytes
        .map(|bytes| {
            let bytes = Zeroizing::new(bytes);
            let mut secret = Zeroizing::new([0u8; 32]);
            if bytes.len() != 32 {
                return Err(StorageError::CorruptWorkspace);
            }
            secret.copy_from_slice(&bytes);
            let key = SecretKey::from_bytes(&secret);
            if key.public().as_bytes() != identity.as_bytes() {
                return Err(StorageError::CorruptWorkspace);
            }
            Ok(key)
        })
        .transpose()
}

/// Binds one Package Instance to an Agent; a later bind replaces the
/// Package's previous Agent, never another Package's binding (ADR 0030).
pub(crate) fn bind(
    connection: &Connection,
    workspace: WorkspaceId,
    id: PrincipalId,
    package: &str,
) -> Result<(), StorageError> {
    connection.execute("INSERT INTO agent_package_bindings (workspace_id,package_identity,agent_principal_id) VALUES (?1,?2,?3) ON CONFLICT(workspace_id,package_identity) DO UPDATE SET agent_principal_id=excluded.agent_principal_id",
        params![workspace.as_bytes().as_slice(), package, id.as_bytes().as_slice()])
        .map_err(|_| StorageError::Database)?;
    Ok(())
}

pub(crate) fn unbind(
    connection: &Connection,
    workspace: WorkspaceId,
    package: &str,
) -> Result<(), StorageError> {
    connection
        .execute(
            "DELETE FROM agent_package_bindings WHERE workspace_id=?1 AND package_identity=?2",
            params![workspace.as_bytes().as_slice(), package],
        )
        .map_err(|_| StorageError::Database)?;
    Ok(())
}

/// The stored binding row. Callers filter it through the current registry:
/// a row may outlive its Agent when a peer removed the Agent's Steward.
pub(crate) fn bound_agent(
    connection: &Connection,
    workspace: WorkspaceId,
    package: &str,
) -> Result<Option<PrincipalId>, StorageError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT agent_principal_id FROM agent_package_bindings WHERE workspace_id=?1 AND package_identity=?2",
            params![workspace.as_bytes().as_slice(), package],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StorageError::Database)?;
    bytes
        .map(|bytes| {
            <[u8; 32]>::try_from(bytes.as_slice())
                .map(PrincipalId)
                .map_err(|_| StorageError::CorruptWorkspace)
        })
        .transpose()
}

/// Whether any Package Instance on this Installation is bound to the Agent.
pub(crate) fn has_binding(
    connection: &Connection,
    workspace: WorkspaceId,
    id: PrincipalId,
) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_package_bindings WHERE workspace_id=?1 AND agent_principal_id=?2)",
            params![workspace.as_bytes().as_slice(), id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| StorageError::Database)
}

/// Clears every binding to the Agent whose key is `identity`. Only the
/// Steward's Installation holds custody and binding rows, so an accepted
/// removal from anywhere clears exactly this Installation's rows.
pub(crate) fn clear_bindings_for_key(
    connection: &Connection,
    workspace: WorkspaceId,
    identity: PublicIdentity,
) -> Result<(), StorageError> {
    connection
        .execute(
            "DELETE FROM agent_package_bindings WHERE workspace_id=?1 AND agent_principal_id IN (SELECT agent_principal_id FROM agent_keys WHERE workspace_id=?1 AND public_identity=?2)",
            params![workspace.as_bytes().as_slice(), identity.as_bytes().as_slice()],
        )
        .map_err(|_| StorageError::Database)?;
    Ok(())
}
