//! Installation-local Agent signing-key custody and Package binding.
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

pub(crate) fn bind(
    connection: &Connection,
    workspace: WorkspaceId,
    id: PrincipalId,
    package: &str,
) -> Result<(), StorageError> {
    connection.execute("INSERT INTO agent_package_bindings (workspace_id,agent_principal_id,package_identity) VALUES (?1,?2,?3) ON CONFLICT(workspace_id,agent_principal_id) DO UPDATE SET package_identity=excluded.package_identity",
        params![workspace.as_bytes().as_slice(), id.as_bytes().as_slice(), package])
        .map_err(|_| StorageError::Database)?;
    Ok(())
}
