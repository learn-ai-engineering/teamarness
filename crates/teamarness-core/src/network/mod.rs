//! Shared Core networking primitives.
//!
//! This module is the sole owner of the installation Endpoint and Router. The
//! onboarding and synchronization protocols are registered together here so a
//! runtime never accidentally creates a second endpoint for replication.

use std::sync::Arc;
use std::time::Duration;

use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode};
use iroh_base::{EndpointAddr, PublicKey, SecretKey};
use tokio::sync::Semaphore;
use tokio::time::timeout;

use crate::OnboardingError;
use crate::onboarding::handler::OnboardingHandler;
use crate::synchronization::{
    FatalSyncError, LogSessionError, LogSyncHandler, OperationOutcome, PeerSessions, Refusal,
    SessionGrant, SyncReceiver, SyncServices,
};

pub(crate) const PACKAGE_SYNC_ALPN: &[u8] = crate::synchronization::network_handler::LOG_SYNC_ALPN;

/// One extra Core protocol the runtime composition point registers with the
/// shared Router. Keeping it opaque here is what lets this module own the
/// Endpoint and Router while naming no protocol or synchronization type.
pub(crate) type ProtocolRegistration = (Vec<u8>, Box<dyn iroh::protocol::DynProtocolHandler>);
pub(crate) const MAX_OUTBOUND_PEERS: usize = 4;
pub(crate) const SYNC_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The receiver a Network bound without Core services uses. It authorizes
/// nothing, so an unattached endpoint serves no Workspace.
#[derive(Debug)]
struct RejectingSyncReceiver;

impl SyncReceiver for RejectingSyncReceiver {
    fn authorize_session(
        &self,
        _peer: crate::PublicIdentity,
        _workspace: crate::WorkspaceId,
        _package: &str,
    ) -> Result<SessionGrant, Refusal> {
        Err(Refusal::NotAuthorized)
    }

    fn protection_bundle_for(
        &self,
        _workspace: crate::WorkspaceId,
        _package: &str,
        _recipient: crate::PublicIdentity,
    ) -> Result<Option<crate::synchronization::ProtectionBundle>, ()> {
        Ok(None)
    }

    fn import_protection_bundle(
        &self,
        _peer: crate::PublicIdentity,
        _workspace: crate::WorkspaceId,
        _package: &str,
        _bundle: crate::synchronization::ProtectionBundle,
    ) -> Result<usize, ()> {
        Err(())
    }

    fn authors(
        &self,
        _peer: crate::PublicIdentity,
        _workspace: crate::WorkspaceId,
    ) -> Result<Vec<crate::PublicIdentity>, ()> {
        Err(())
    }

    fn operations(
        &self,
        _peer: crate::PublicIdentity,
        _workspace: crate::WorkspaceId,
        _package: &str,
    ) -> Result<Vec<([u8; 32], p2panda_core::Operation<()>)>, ()> {
        Err(())
    }

    fn receive_operation(
        &self,
        _peer: crate::PublicIdentity,
        _workspace: crate::WorkspaceId,
        _package: &str,
        _operation: p2panda_core::Operation<()>,
    ) -> Result<OperationOutcome, FatalSyncError> {
        Err(FatalSyncError::PeerRevoked)
    }
}

pub(crate) struct Network {
    endpoint: Endpoint,
    router: Router,
    outbound: Arc<Semaphore>,
    /// A second peer budget, for the Core protocols registered beside package
    /// sync. A session stalled on one protocol cannot starve the other.
    outbound_extra: Arc<Semaphore>,
    services: SyncServices,
    /// Loopback runtimes have no relay and no discovery, so tests register peer
    /// addresses here instead. Production binds with `loopback_only == false`.
    address_book: Option<iroh::address_lookup::memory::MemoryLookup>,
    #[cfg(test)]
    alpns: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Network {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Network")
            .field("endpoint_id", &self.endpoint.id())
            .finish_non_exhaustive()
    }
}

impl Network {
    pub(crate) async fn bind(
        secret_key: SecretKey,
        relay_mode: RelayMode,
        loopback_only: bool,
        onboarding: Arc<OnboardingHandler>,
        services: Option<SyncServices>,
        protocols: Vec<ProtocolRegistration>,
    ) -> Result<Arc<Self>, OnboardingError> {
        let mut configured_alpns = vec![crate::onboarding::transport::ONBOARDING_ALPN.to_vec()];
        configured_alpns.extend(protocols.iter().map(|(alpn, _)| alpn.clone()));
        configured_alpns.push(PACKAGE_SYNC_ALPN.to_vec());
        let mut builder = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .alpns(configured_alpns.clone())
            .relay_mode(relay_mode);
        let address_book = if loopback_only {
            let lookup = iroh::address_lookup::memory::MemoryLookup::new();
            builder = builder
                .address_lookup(lookup.clone())
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .map_err(|_| OnboardingError::Readiness)?;
            Some(lookup)
        } else {
            None
        };
        let endpoint = builder
            .bind()
            .await
            .map_err(|_| OnboardingError::Readiness)?;
        let services = services.unwrap_or_else(|| SyncServices {
            receiver: Arc::new(RejectingSyncReceiver),
            sessions: PeerSessions::new(),
        });
        let log_sync = Arc::new(LogSyncHandler::new(services.clone()));
        let mut builder = Router::builder(endpoint.clone())
            .accept(crate::onboarding::transport::ONBOARDING_ALPN, onboarding);
        for (alpn, handler) in protocols {
            builder = builder.accept(alpn, handler);
        }
        let router = builder.accept(PACKAGE_SYNC_ALPN, log_sync).spawn();
        Ok(Arc::new(Self {
            endpoint,
            router,
            outbound: Arc::new(Semaphore::new(MAX_OUTBOUND_PEERS)),
            outbound_extra: Arc::new(Semaphore::new(MAX_OUTBOUND_PEERS)),
            services,
            address_book,
            #[cfg(test)]
            alpns: configured_alpns,
        }))
    }

    pub(crate) fn endpoint_id(&self) -> PublicKey {
        self.endpoint.id()
    }

    pub(crate) fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    #[cfg(test)]
    pub(crate) fn alpns(&self) -> &[Vec<u8>] {
        &self.alpns
    }

    /// Teaches a loopback endpoint how to reach a peer it has never dialled.
    #[cfg(test)]
    pub(crate) fn remember_peer_address(&self, address: EndpointAddr) {
        if let Some(book) = &self.address_book {
            book.add_endpoint_info(address);
        }
    }

    pub(crate) async fn send_log_sync(
        &self,
        address: EndpointAddr,
        receiver: Arc<dyn SyncReceiver>,
        workspace: crate::WorkspaceId,
        package: String,
        peer: crate::PublicIdentity,
    ) -> Result<usize, LogSessionError> {
        self.send_log_sync_with_timeout(
            address,
            receiver,
            workspace,
            package,
            peer,
            crate::synchronization::network_handler::SYNC_SESSION_TIMEOUT,
        )
        .await
    }

    async fn send_log_sync_with_timeout(
        &self,
        address: EndpointAddr,
        receiver: Arc<dyn SyncReceiver>,
        workspace: crate::WorkspaceId,
        package: String,
        peer: crate::PublicIdentity,
        session_timeout: Duration,
    ) -> Result<usize, LogSessionError> {
        timeout(session_timeout, async {
            let _permit = self
                .outbound
                .clone()
                .try_acquire_owned()
                .map_err(|_| LogSessionError::Refused(Refusal::Unavailable))?;
            crate::synchronization::network_handler::outbound_log_session(
                &self.endpoint,
                address,
                receiver,
                Arc::clone(&self.services.sessions),
                workspace,
                package,
                peer,
            )
            .await
        })
        .await
        .map_err(|_| LogSessionError::Transport("package sync timed out".to_owned()))?
    }

    /// Runs one outbound session on the shared endpoint within the outbound
    /// peer budget. The session itself is supplied by the caller, so this
    /// module never names the protocol it carries.
    pub(crate) async fn run_outbound<T, E, Fut>(
        &self,
        overloaded: E,
        session: impl FnOnce(Endpoint) -> Fut,
    ) -> Result<T, E>
    where
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        let Ok(_permit) = self.outbound_extra.clone().try_acquire_owned() else {
            return Err(overloaded);
        };
        session(self.endpoint.clone()).await
    }

    pub(crate) async fn shutdown(&self) -> Result<(), OnboardingError> {
        self.router
            .shutdown()
            .await
            .map_err(|_| OnboardingError::Shutdown)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh_base::SecretKey;

    use super::*;

    fn secret(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }

    /// A protocol this module knows nothing about, which is the point: the
    /// composition point supplies the handler and this module only routes it.
    #[derive(Debug)]
    struct OpaqueProtocol;

    impl iroh::protocol::ProtocolHandler for OpaqueProtocol {
        async fn accept(
            &self,
            connection: iroh::endpoint::Connection,
        ) -> Result<(), iroh::protocol::AcceptError> {
            connection.close(1_u8.into(), b"unavailable");
            Ok(())
        }
    }

    #[tokio::test]
    async fn one_router_registers_onboarding_composed_protocols_and_package_sync() {
        let handler = Arc::new(OnboardingHandler::new(Duration::from_secs(1)));
        let network = Network::bind(
            secret(1),
            RelayMode::Disabled,
            true,
            handler,
            None,
            vec![(
                b"teamarness/opaque-protocol/1".to_vec(),
                Box::new(OpaqueProtocol) as Box<dyn iroh::protocol::DynProtocolHandler>,
            )],
        )
        .await
        .unwrap();
        assert_eq!(network.endpoint_id(), secret(1).public());
        assert_eq!(
            network.alpns(),
            vec![
                crate::onboarding::transport::ONBOARDING_ALPN.to_vec(),
                b"teamarness/opaque-protocol/1".to_vec(),
                b"teamarness/package-sync/2".to_vec(),
            ]
        );
        network.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_unattached_network_authorizes_no_session() {
        let handler = Arc::new(OnboardingHandler::new(Duration::from_secs(1)));
        let server = Network::bind(
            secret(3),
            RelayMode::Disabled,
            true,
            handler,
            None,
            Vec::new(),
        )
        .await
        .unwrap();
        let client = Network::bind(
            secret(4),
            RelayMode::Disabled,
            true,
            Arc::new(OnboardingHandler::new(Duration::from_secs(1))),
            None,
            Vec::new(),
        )
        .await
        .unwrap();
        let address = server.endpoint().bound_sockets().into_iter().fold(
            EndpointAddr::new(server.endpoint_id()),
            EndpointAddr::with_ip_addr,
        );
        let result = client
            .send_log_sync(
                address,
                Arc::new(RejectingSyncReceiver),
                crate::WorkspaceId::from_bytes([8; 16]),
                "teamarness.test".to_owned(),
                crate::PublicIdentity::from_bytes(*server.endpoint_id().as_bytes()).unwrap(),
            )
            .await;
        assert_eq!(
            result.unwrap_err(),
            LogSessionError::Refused(Refusal::NotAuthorized)
        );
        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }
}
