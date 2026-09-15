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
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use crate::OnboardingError;
use crate::onboarding::handler::OnboardingHandler;
use crate::synchronization::{
    LogSyncHandler, PackageSyncHandler, SyncBatch, SyncReceiver, encode_batch,
};

pub(crate) const PACKAGE_SYNC_ALPN: &[u8] = b"teamarness/package-sync/1";
const PACKAGE_BATCH_ALPN: &[u8] = b"teamarness/package-sync-batch/1";
pub(crate) const MAX_OUTBOUND_PEERS: usize = 4;
pub(crate) const SYNC_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
struct RejectingSyncReceiver;

impl SyncReceiver for RejectingSyncReceiver {
    fn receive(&self, _peer: crate::PublicIdentity, _batch: SyncBatch) -> Result<usize, ()> {
        Err(())
    }
}

pub(crate) struct Network {
    endpoint: Endpoint,
    router: Router,
    outbound: Arc<Semaphore>,
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
        receiver: Option<Arc<dyn SyncReceiver>>,
    ) -> Result<Arc<Self>, OnboardingError> {
        let configured_alpns = vec![
            crate::onboarding::transport::ONBOARDING_ALPN.to_vec(),
            PACKAGE_SYNC_ALPN.to_vec(),
            PACKAGE_BATCH_ALPN.to_vec(),
        ];
        let mut builder = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .alpns(configured_alpns.clone())
            .relay_mode(relay_mode);
        if loopback_only {
            builder = builder
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .map_err(|_| OnboardingError::Readiness)?;
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|_| OnboardingError::Readiness)?;
        let receiver = receiver.unwrap_or_else(|| Arc::new(RejectingSyncReceiver));
        let synchronization = Arc::new(PackageSyncHandler::new(Arc::clone(&receiver)));
        let log_sync = Arc::new(LogSyncHandler::new(receiver));
        let router = Router::builder(endpoint.clone())
            .accept(crate::onboarding::transport::ONBOARDING_ALPN, onboarding)
            .accept(PACKAGE_BATCH_ALPN, synchronization)
            .accept(PACKAGE_SYNC_ALPN, log_sync)
            .spawn();
        Ok(Arc::new(Self {
            endpoint,
            router,
            outbound: Arc::new(Semaphore::new(MAX_OUTBOUND_PEERS)),
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

    pub(crate) async fn send_batch(
        &self,
        address: EndpointAddr,
        batch: &SyncBatch,
    ) -> Result<usize, OnboardingError> {
        let permit = self
            .outbound
            .clone()
            .try_acquire_owned()
            .map_err(|_| OnboardingError::Overload)?;
        self.send_batch_with_permit(address, batch, permit).await
    }

    async fn send_batch_with_permit(
        &self,
        address: EndpointAddr,
        batch: &SyncBatch,
        _permit: OwnedSemaphorePermit,
    ) -> Result<usize, OnboardingError> {
        let connection = timeout(
            SYNC_CONNECT_TIMEOUT,
            self.endpoint.connect(address, PACKAGE_BATCH_ALPN),
        )
        .await
        .map_err(|_| OnboardingError::Timeout)?
        .map_err(|_| OnboardingError::Authentication)?;
        let (mut send, mut receive) = timeout(Duration::from_secs(30), connection.open_bi())
            .await
            .map_err(|_| OnboardingError::Timeout)?
            .map_err(|_| OnboardingError::Authentication)?;
        let frame = encode_batch(batch).map_err(|_| OnboardingError::Overload)?;
        send.write_all(&frame)
            .await
            .map_err(|_| OnboardingError::Authentication)?;
        send.finish().map_err(|_| OnboardingError::Authentication)?;
        let ack = receive
            .read_to_end(16)
            .await
            .map_err(|_| OnboardingError::Authentication)?;
        if ack.len() != 8 || ack[4] != 1 || ack[5] != 2 {
            return Err(OnboardingError::InvalidInput);
        }
        Ok(usize::from(u16::from_be_bytes([ack[6], ack[7]])))
    }

    pub(crate) async fn send_log_sync(
        &self,
        address: EndpointAddr,
        receiver: Arc<dyn SyncReceiver>,
        workspace: crate::WorkspaceId,
        package: String,
        peer: crate::PublicIdentity,
    ) -> Result<usize, OnboardingError> {
        timeout(
            crate::synchronization::network_handler::SYNC_SESSION_TIMEOUT,
            crate::synchronization::network_handler::outbound_log_session(
                &self.endpoint,
                address,
                receiver,
                workspace,
                package,
                peer,
            ),
        )
        .await
        .map_err(|_| OnboardingError::Timeout)?
        .map_err(|_| OnboardingError::Authentication)
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
    use std::sync::Mutex;
    use std::time::Duration;

    use iroh_base::{EndpointAddr, SecretKey};

    use super::*;

    #[derive(Debug, Default)]
    struct RecordingReceiver(Mutex<Vec<SyncBatch>>);

    impl SyncReceiver for RecordingReceiver {
        fn receive(&self, _peer: crate::PublicIdentity, batch: SyncBatch) -> Result<usize, ()> {
            let count = batch.records.len();
            self.0.lock().unwrap().push(batch);
            Ok(count)
        }
    }

    fn secret(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }

    #[tokio::test]
    async fn one_router_registers_onboarding_and_package_sync_alpns() {
        let handler = Arc::new(OnboardingHandler::new(Duration::from_secs(1)));
        let network = Network::bind(secret(1), RelayMode::Disabled, true, handler, None)
            .await
            .unwrap();
        assert_eq!(network.endpoint_id(), secret(1).public());
        assert_eq!(
            network.alpns(),
            vec![
                crate::onboarding::transport::ONBOARDING_ALPN.to_vec(),
                PACKAGE_SYNC_ALPN.to_vec(),
                PACKAGE_BATCH_ALPN.to_vec(),
            ]
        );
        network.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn bounded_sync_batch_is_delivered_through_the_shared_network() {
        let receiver = Arc::new(RecordingReceiver::default());
        let server = Network::bind(
            secret(3),
            RelayMode::Disabled,
            true,
            Arc::new(OnboardingHandler::new(Duration::from_secs(1))),
            Some(receiver.clone()),
        )
        .await
        .unwrap();
        let client = Network::bind(
            secret(4),
            RelayMode::Disabled,
            true,
            Arc::new(OnboardingHandler::new(Duration::from_secs(1))),
            None,
        )
        .await
        .unwrap();
        let address = server.endpoint().bound_sockets().into_iter().fold(
            EndpointAddr::new(server.endpoint_id()),
            EndpointAddr::with_ip_addr,
        );
        let batch = SyncBatch {
            workspace: [8; 16],
            membership: vec![],
            package: "teamarness.test".to_owned(),
            records: vec![vec![1, 2, 3], vec![4]],
        };
        assert_eq!(client.send_batch(address, &batch).await.unwrap(), 2);
        assert_eq!(receiver.0.lock().unwrap().as_slice(), &[batch]);
        client.shutdown().await.unwrap();
        server.shutdown().await.unwrap();
    }
}
