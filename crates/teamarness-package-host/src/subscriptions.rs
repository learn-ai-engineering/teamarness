//! Bounded subscriptions to host revisions and to one Package UI event stream.

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::errors::{
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, RecoveryAction, host_error,
};
use crate::values::{HostRevision, PackageValue, UiMountId, UiSubscriptionId};
use crate::{PackageHost, protocol};

/// A closed selector for host state or one already-opened UI event route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageHostSubscriptionSelector {
    HostState {
        after_revision: HostRevision,
    },
    UiEvents {
        mount: UiMountId,
        subscription: UiSubscriptionId,
    },
}

/// A coalesced host-state notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStateNotification {
    pub revision: HostRevision,
    pub revision_gap: bool,
}

/// One ordered Package UI event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiEventNotification {
    pub mount: UiMountId,
    pub subscription: UiSubscriptionId,
    /// Decimal-string sequence for JavaScript-safe desktop transport.
    pub sequence: String,
    pub payload: PackageValue,
}

/// Typed result yielded by a Package Host subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PackageHostNotification {
    HostState(HostStateNotification),
    UiEvent(UiEventNotification),
    UiStreamCompleted(UiStreamCompletedNotification),
}

/// A Package UI stream the Package ended normally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStreamCompletedNotification {
    pub mount: UiMountId,
    pub subscription: UiSubscriptionId,
}

/// A bounded subscription to host revisions or one Package UI stream.
pub struct PackageHostSubscription {
    pub(crate) kind: PackageHostSubscriptionKind,
}

pub(crate) enum PackageHostSubscriptionKind {
    HostState {
        receiver: watch::Receiver<HostRevision>,
        after: HostRevision,
    },
    UiEvents {
        receiver: protocol::UiEventReceiver,
        host: PackageHost,
        mount: UiMountId,
        subscription: UiSubscriptionId,
    },
}

#[allow(clippy::result_large_err)]
impl PackageHostSubscription {
    /// Waits for the next typed notification or a terminal stream result.
    pub async fn changed(&mut self) -> Result<PackageHostNotification, PackageErrorEnvelope> {
        match &mut self.kind {
            PackageHostSubscriptionKind::HostState { receiver, after } => {
                let current = *receiver.borrow();
                let revision = if current > *after {
                    current
                } else {
                    receiver.changed().await.map_err(|_| {
                        host_error(PackageErrorCode::ShuttingDown, PackageErrorStage::Shutdown)
                    })?;
                    *receiver.borrow()
                };
                let notification = HostStateNotification {
                    revision,
                    revision_gap: revision.0 > after.0.saturating_add(1),
                };
                *after = revision;
                Ok(PackageHostNotification::HostState(notification))
            }
            PackageHostSubscriptionKind::UiEvents {
                receiver,
                host,
                mount,
                subscription,
            } => match receiver.changed().await {
                Ok(event) => Ok(PackageHostNotification::UiEvent(UiEventNotification {
                    mount: mount.clone(),
                    subscription: subscription.clone(),
                    sequence: event.sequence.to_string(),
                    payload: event.payload,
                })),
                Err(protocol::StreamEnd::Completed) => {
                    host.release_ui_subscription(mount, subscription).await;
                    Ok(PackageHostNotification::UiStreamCompleted(
                        UiStreamCompletedNotification {
                            mount: mount.clone(),
                            subscription: subscription.clone(),
                        },
                    ))
                }
                Err(protocol::StreamEnd::Package(package_error)) => {
                    host.release_ui_subscription(mount, subscription).await;
                    let mut error =
                        host_error(PackageErrorCode::BackendError, PackageErrorStage::Ui);
                    error.retryable = package_error.retryable;
                    error.recovery = RecoveryAction::None;
                    error.package_error = Some(package_error);
                    Err(error)
                }
                Err(protocol::StreamEnd::Host(code)) => {
                    if matches!(
                        code,
                        PackageErrorCode::UiContractViolation
                            | PackageErrorCode::EventLagged
                            | PackageErrorCode::ProtocolViolation
                    ) {
                        host.fail_mount(mount).await;
                    }
                    Err(host_error(code, PackageErrorStage::Ui))
                }
            },
        }
    }
}
