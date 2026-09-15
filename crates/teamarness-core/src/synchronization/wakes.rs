use std::collections::{BTreeMap, VecDeque};

use crate::{WorkspaceId, synchronization::scope_digest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DurableWake {
    pub(crate) workspace: WorkspaceId,
    pub(crate) generation: i64,
}

/// Coalesces durable wake rows before scheduling finite peer sessions. A wake
/// is only a hint; the caller must always reload durable heights before dialing.
#[derive(Debug, Default)]
pub(crate) struct WakeQueue {
    pending: BTreeMap<(WorkspaceId, String), DurableWake>,
    order: VecDeque<(WorkspaceId, String)>,
}

impl WakeQueue {
    pub(crate) fn publish(&mut self, package: String, wake: DurableWake) {
        let key = (wake.workspace, package);
        let replaced = self.pending.insert(key.clone(), wake).is_some();
        if !replaced {
            self.order.push_back(key);
        }
    }

    pub(crate) fn next(&mut self) -> Option<(String, DurableWake)> {
        let key = self.order.pop_front()?;
        let wake = self.pending.remove(&key)?;
        Some((key.1, wake))
    }

    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }
}

pub(crate) fn durable_wakes(
    conn: &rusqlite::Connection,
) -> Result<Vec<DurableWake>, rusqlite::Error> {
    let mut statement = conn.prepare(
        "SELECT workspace_id, wake_generation FROM durable_wakes JOIN sync_scopes USING(scope_id)",
    )?;
    let rows = statement.query_map([], |row| {
        let workspace: Vec<u8> = row.get(0)?;
        let workspace = workspace
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(DurableWake {
            workspace: WorkspaceId::from_bytes(workspace),
            generation: row.get(1)?,
        })
    })?;
    rows.collect()
}

#[allow(dead_code)]
fn scope_for_wake(package: &str, workspace: WorkspaceId) -> [u8; 32] {
    scope_digest(package, workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_wakes_coalesce_and_preserve_fair_queue_order() {
        let workspace = WorkspaceId::from_bytes([3; 16]);
        let mut queue = WakeQueue::default();
        queue.publish(
            "a".to_owned(),
            DurableWake {
                workspace,
                generation: 1,
            },
        );
        queue.publish(
            "a".to_owned(),
            DurableWake {
                workspace,
                generation: 2,
            },
        );
        queue.publish(
            "b".to_owned(),
            DurableWake {
                workspace,
                generation: 1,
            },
        );
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.next().unwrap().1.generation, 2);
        assert_eq!(queue.next().unwrap().0, "b");
        assert!(queue.next().is_none());
    }
}
