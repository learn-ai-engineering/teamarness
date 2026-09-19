//! The outbound side of package synchronization, and the protocol registration
//! the runtime composition point hands to `network`.
//!
//! `network` owns the Endpoint, the Router and the outbound peer budgets, and
//! names no protocol. Everything that knows what a package-sync session is
//! lives here.

use std::sync::Arc;

use iroh_base::EndpointAddr;
use tokio::time::timeout;

use crate::network::{Network, ProtocolRegistration};
use crate::synchronization::{
    ExchangeReport, LogSessionError, LogSyncHandler, Refusal, SyncReceiver, SyncServices,
};

/// The package-sync registration and the handler behind it.
///
/// The handler is returned so the runtime can observe inbound session counts in
/// tests; the registration is opaque to `network`.
pub(crate) fn package_sync_registration(
    services: SyncServices,
) -> (ProtocolRegistration, Arc<LogSyncHandler>) {
    let limits = services.limits;
    let handler = Arc::new(LogSyncHandler::new(services, limits));
    (
        (
            crate::synchronization::network_handler::LOG_SYNC_ALPN.to_vec(),
            Box::new(Arc::clone(&handler)) as Box<dyn iroh::protocol::DynProtocolHandler>,
        ),
        handler,
    )
}

/// Runs one outbound package synchronization session with a peer, inside the
/// session timeout and the shared outbound peer budget.
pub(crate) async fn send_log_sync(
    network: &Network,
    services: &SyncServices,
    address: EndpointAddr,
    receiver: Arc<dyn SyncReceiver>,
    workspace: crate::WorkspaceId,
    package: String,
    peer: crate::PublicIdentity,
) -> Result<ExchangeReport, LogSessionError> {
    let sessions = Arc::clone(&services.sessions);
    let health = Arc::clone(&services.health);
    let limits = services.limits;
    timeout(
        limits.session_timeout,
        network.run_outbound(
            LogSessionError::Refused(Refusal::Unavailable),
            move |endpoint| async move {
                crate::synchronization::network_handler::outbound_log_session(
                    &endpoint, address, receiver, sessions, health, workspace, package, peer,
                    limits,
                )
                .await
            },
        ),
    )
    .await
    .map_err(|_| LogSessionError::Transport("package sync timed out".to_owned()))?
}
