//! UI request, cancellation and event-subscription commands.

use tokio::sync::watch;

use crate::errors::{
    PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, host_error, wire_error,
};
use crate::values::UiMountId;
use crate::{
    AcknowledgeUiEvents, CancelUiRequest, CloseUiSubscription, OpenUiSubscription, PackageHost,
    PackageHostOutcome, UiRequest, protocol,
};

#[allow(clippy::result_large_err)]
impl PackageHost {
    /// Bounded, read-only operation metadata from this Instance's admitted contract.
    /// This neither activates a Backend nor opens a mount or reads source files.
    pub async fn package_operations(
        &self,
        instance: &crate::PackageInstanceKey,
    ) -> Result<serde_json::Value, PackageErrorEnvelope> {
        self.actor(instance).await?;
        let ui = self
            .admitted_ui(&instance.package_identity)
            .ok_or_else(|| host_error(PackageErrorCode::UiLoadFailed, PackageErrorStage::Ui))?;
        let operations: Vec<_> = ui
            .contract
            .operations()
            .into_iter()
            .map(|(name, kind)| serde_json::json!({"name":name,"kind":kind}))
            .collect();
        Ok(serde_json::json!({"contractDigest":ui.cache.contract_digest,"operations":operations}))
    }

    /// One admitted operation, including both kinds if declared, and complete local `$defs`.
    /// The schema and digest belong to the same immutable admission as request validation.
    pub async fn package_schema(
        &self,
        instance: &crate::PackageInstanceKey,
        operation: &str,
    ) -> Result<serde_json::Value, PackageErrorEnvelope> {
        self.actor(instance).await?;
        let ui = self
            .admitted_ui(&instance.package_identity)
            .ok_or_else(|| host_error(PackageErrorCode::UiLoadFailed, PackageErrorStage::Ui))?;
        let schema = ui.contract.operation_schema(operation).ok_or_else(|| {
            host_error(PackageErrorCode::UiContractViolation, PackageErrorStage::Ui)
        })?;
        Ok(serde_json::json!({"contractDigest":ui.cache.contract_digest,"schema":schema}))
    }

    /// Checks against the exact live lease's existing admitted validator, without dispatch.
    /// Non-rendering adapters can distinguish proven input refusals from response failures.
    pub async fn validate_ui_request(&self, input: &UiRequest) -> Result<(), PackageErrorEnvelope> {
        let (lease, _) = self.live_mount(&input.mount).await?;
        lease
            .ui
            .contract
            .validate_request(&input.operation, &input.payload)
            .map_err(|_| host_error(PackageErrorCode::UiContractViolation, PackageErrorStage::Ui))
    }

    pub(crate) async fn ui_request(
        &self,
        input: UiRequest,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let (lease, actor) = self.live_mount(&input.mount).await?;
        lease
            .ui
            .contract
            .validate_request(&input.operation, &input.payload)
            .map_err(|_| {
                host_error(PackageErrorCode::UiContractViolation, PackageErrorStage::Ui)
            })?;
        let operation = input.operation;
        let payload = actor
            .request(
                &input.mount,
                input.request,
                operation.clone(),
                input.payload,
            )
            .await
            .map_err(|failure| wire_error(&failure, PackageErrorStage::Process))?;
        if lease
            .ui
            .contract
            .validate_response(&operation, &payload)
            .is_err()
        {
            self.fail_mount(&input.mount).await;
            return Err(host_error(
                PackageErrorCode::UiContractViolation,
                PackageErrorStage::Ui,
            ));
        }
        Ok(PackageHostOutcome::UiResponse { payload })
    }

    pub(crate) async fn cancel_ui_request(
        &self,
        input: CancelUiRequest,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let (_, actor) = self.live_mount(&input.mount).await?;
        actor
            .cancel(&input.mount, &input.request)
            .await
            .map_err(|failure| wire_error(&failure, PackageErrorStage::Protocol))?;
        Ok(PackageHostOutcome::UiRequestCancelled)
    }

    pub(crate) async fn open_ui_subscription(
        &self,
        input: OpenUiSubscription,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let (lease, actor) = self.live_mount(&input.mount).await?;
        lease
            .ui
            .contract
            .validate_subscription(&input.operation, &input.payload)
            .map_err(|_| {
                host_error(PackageErrorCode::UiContractViolation, PackageErrorStage::Ui)
            })?;
        let receiver = actor
            .stream_open(
                &input.mount,
                input.subscription.clone(),
                input.operation,
                input.payload,
                lease.ui.clone(),
            )
            .await
            .map_err(|failure| wire_error(&failure, PackageErrorStage::Protocol))?;
        let terminal = receiver.terminal();
        self.inner
            .data
            .lock()
            .await
            .ui_routes
            .insert((input.mount.clone(), input.subscription), Some(receiver));
        self.monitor_ui_route(input.mount, terminal);
        Ok(PackageHostOutcome::UiSubscriptionOpened)
    }

    pub(crate) async fn acknowledge_ui_events(
        &self,
        input: AcknowledgeUiEvents,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let (_, actor) = self.live_mount(&input.mount).await?;
        actor
            .acknowledge(&input.mount, &input.subscription, input.through_sequence)
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Protocol))?;
        Ok(PackageHostOutcome::UiEventsAcknowledged)
    }

    pub(crate) async fn close_ui_subscription(
        &self,
        input: CloseUiSubscription,
    ) -> Result<PackageHostOutcome, PackageErrorEnvelope> {
        let (_, actor) = self.live_mount(&input.mount).await?;
        actor
            .close_subscription(&input.mount, &input.subscription)
            .await
            .map_err(|failure| host_error(failure.code, PackageErrorStage::Protocol))?;
        self.inner
            .data
            .lock()
            .await
            .ui_routes
            .remove(&(input.mount, input.subscription));
        Ok(PackageHostOutcome::UiSubscriptionClosed)
    }

    pub(crate) fn monitor_ui_route(
        &self,
        mount: UiMountId,
        mut terminal: watch::Receiver<Option<protocol::StreamEnd>>,
    ) {
        let host = self.clone();
        tokio::spawn(async move {
            loop {
                let current = terminal.borrow().clone();
                if let Some(end) = current {
                    // Only a host-owned failure faults the mount; a Package-owned end is the
                    // Package's own business and reaches its subscriber.
                    if let protocol::StreamEnd::Host(code) = end
                        && matches!(
                            code,
                            PackageErrorCode::UiContractViolation
                                | PackageErrorCode::EventLagged
                                | PackageErrorCode::ProtocolViolation
                        )
                    {
                        host.fail_mount(&mount).await;
                    }
                    return;
                }
                if terminal.changed().await.is_err() {
                    return;
                }
            }
        });
    }
}
