//! The membership subscription handle and the runtime's subscriber registry.

use super::*;

/// A gap-free stream of durable then live membership transitions.
pub struct MembershipSubscription {
    pub(super) state: crate::subscription::SubscriptionState,
    pub(super) runtime_state: Weak<Mutex<RuntimeState>>,
    pub(super) workspace_id: WorkspaceId,
    pub(super) subscriber_id: u64,
}

impl fmt::Debug for MembershipSubscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MembershipSubscription")
            .finish_non_exhaustive()
    }
}

impl Drop for MembershipSubscription {
    fn drop(&mut self) {
        let Some(runtime_state) = self.runtime_state.upgrade() else {
            return;
        };
        let Ok(mut state) = runtime_state.lock() else {
            return;
        };
        if let Some(listeners) = state.subscribers.get_mut(&self.workspace_id) {
            listeners.retain(|listener| listener.id != self.subscriber_id);
            if listeners.is_empty() {
                state.subscribers.remove(&self.workspace_id);
            }
        }
    }
}

impl MembershipSubscription {
    /// Waits for the next transition, returning `None` after orderly shutdown.
    pub fn next_transition(&mut self) -> Result<Option<MembershipTransition>, SubscriptionError> {
        Ok(self.state.next())
    }

    /// Waits up to `timeout` for the next transition.
    ///
    /// `None` means no transition arrived before the timeout or the runtime shut down.
    /// Reopening after the last delivered epoch preserves durable catch-up.
    pub fn next_transition_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Option<MembershipTransition>, SubscriptionError> {
        Ok(self.state.next_timeout(timeout))
    }
}

/// How many membership subscriptions have been opened since the process began.
///
/// A membership stream that waits on the epoch watch opens one per transition.
#[cfg(feature = "test-hooks")]
pub(super) static MEMBERSHIP_SUBSCRIPTIONS_OPENED: Mutex<BTreeMap<WorkspaceId, usize>> =
    Mutex::new(BTreeMap::new());

#[cfg(feature = "test-hooks")]
pub(crate) fn membership_subscriptions_opened(workspace: WorkspaceId) -> usize {
    MEMBERSHIP_SUBSCRIPTIONS_OPENED
        .lock()
        .ok()
        .and_then(|counts| counts.get(&workspace).copied())
        .unwrap_or(0)
}

pub(super) struct MembershipSubscriber {
    pub(super) id: u64,
    pub(super) sender: crate::subscription::TransitionSender,
}
