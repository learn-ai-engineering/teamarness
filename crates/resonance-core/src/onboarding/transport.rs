use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use iroh::endpoint::{Connection, presets};
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode};
use iroh_base::{EndpointAddr, SecretKey};
use tokio::sync::{Notify, OnceCell, OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use super::handler::OnboardingHandler;
use crate::OnboardingError;

pub(crate) const ONBOARDING_ALPN: &[u8] = b"resonance/workspace-join/1";
pub(crate) const INVITATION_READINESS_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);
#[allow(dead_code)]
pub(crate) const RESPONSE_IO_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const WHOLE_JOIN_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const MAX_OUTBOUND_JOINS: usize = 4;

const RUNNING: u8 = 0;
const STOPPING: u8 = 1;
const STOPPED: u8 = 2;

pub(crate) struct Transport {
    endpoint: Endpoint,
    router: Router,
    handler: Arc<OnboardingHandler>,
    lifecycle: Arc<Lifecycle>,
    outbound: Arc<Semaphore>,
    shutdown: OnceCell<Result<(), OnboardingError>>,
    readiness: Readiness,
    require_relay_address: bool,
}

#[derive(Clone, Copy)]
enum Readiness {
    EndpointOnline(Duration),
    #[cfg(test)]
    Immediate,
}

impl Transport {
    pub(crate) async fn bind_production(
        secret_key: SecretKey,
    ) -> Result<Arc<Self>, OnboardingError> {
        Self::bind(
            secret_key,
            BindOptions {
                relay_mode: RelayMode::Default,
                loopback_only: false,
                readiness: Readiness::EndpointOnline(INVITATION_READINESS_TIMEOUT),
                require_relay_address: true,
                request_timeout: REQUEST_READ_TIMEOUT,
            },
        )
        .await
    }

    async fn bind(
        secret_key: SecretKey,
        options: BindOptions,
    ) -> Result<Arc<Self>, OnboardingError> {
        let mut builder = Endpoint::builder(presets::N0)
            .secret_key(secret_key)
            .alpns(vec![ONBOARDING_ALPN.to_vec()])
            .relay_mode(options.relay_mode);
        if options.loopback_only {
            builder = builder
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .map_err(|_| OnboardingError::Readiness)?;
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|_| OnboardingError::Readiness)?;
        let handler = Arc::new(OnboardingHandler::new(options.request_timeout));
        let router = Router::builder(endpoint.clone())
            .accept(ONBOARDING_ALPN, Arc::clone(&handler))
            .spawn();
        Ok(Arc::new(Self {
            endpoint,
            router,
            handler,
            lifecycle: Arc::new(Lifecycle::new()),
            outbound: Arc::new(Semaphore::new(MAX_OUTBOUND_JOINS)),
            shutdown: OnceCell::new(),
            readiness: options.readiness,
            require_relay_address: options.require_relay_address,
        }))
    }

    pub(crate) fn endpoint_id(&self) -> iroh_base::PublicKey {
        self.endpoint.id()
    }

    pub(crate) async fn invitation_address(&self) -> Result<EndpointAddr, OnboardingError> {
        let _operation = self.lifecycle.begin()?;
        match self.readiness {
            Readiness::EndpointOnline(duration) => timeout(duration, self.endpoint.online())
                .await
                .map_err(|_| OnboardingError::Readiness)?,
            #[cfg(test)]
            Readiness::Immediate => {}
        }
        self.lifecycle.ensure_running()?;
        let address = self.endpoint.addr();
        if self.require_relay_address
            && !address.addrs.iter().any(iroh_base::TransportAddr::is_relay)
        {
            return Err(OnboardingError::Readiness);
        }
        Ok(address)
    }

    #[allow(dead_code)]
    pub(crate) async fn begin_outbound(&self) -> Result<OutboundPermit, OnboardingError> {
        let operation = self.lifecycle.begin()?;
        let permit = Arc::clone(&self.outbound)
            .try_acquire_owned()
            .map_err(|_| OnboardingError::Overload)?;
        Ok(OutboundPermit {
            _operation: operation,
            _permit: permit,
        })
    }

    #[allow(dead_code)]
    pub(crate) async fn connect(
        &self,
        address: EndpointAddr,
    ) -> Result<(Connection, OutboundPermit), OnboardingError> {
        let permit = self.begin_outbound().await?;
        let connection = timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(address, ONBOARDING_ALPN),
        )
        .await
        .map_err(|_| OnboardingError::Timeout)?
        .map_err(|_| OnboardingError::Authentication)?;
        Ok((connection, permit))
    }

    #[allow(dead_code)]
    pub(crate) async fn response_io<T>(
        &self,
        future: impl Future<Output = Result<T, OnboardingError>>,
    ) -> Result<T, OnboardingError> {
        timeout(RESPONSE_IO_TIMEOUT, future)
            .await
            .map_err(|_| OnboardingError::Timeout)?
    }

    #[allow(dead_code)]
    pub(crate) async fn whole_join<T>(
        &self,
        future: impl Future<Output = Result<T, OnboardingError>>,
    ) -> Result<T, OnboardingError> {
        timeout(WHOLE_JOIN_TIMEOUT, future)
            .await
            .map_err(|_| OnboardingError::Timeout)?
    }

    pub(crate) async fn shutdown(&self) -> Result<(), OnboardingError> {
        self.lifecycle.stop();
        self.handler.stop();
        *self
            .shutdown
            .get_or_init(|| async {
                self.lifecycle.wait_for_idle().await;
                let result = self
                    .router
                    .shutdown()
                    .await
                    .map_err(|_| OnboardingError::Shutdown);
                self.lifecycle.stopped();
                result
            })
            .await
    }

    pub(crate) fn start_abnormal_shutdown(self: &Arc<Self>) {
        if !self.lifecycle.stop() {
            return;
        }
        self.handler.stop();
        let transport = Arc::clone(self);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = transport.shutdown().await;
            });
            return;
        }
        let _ = std::thread::Builder::new()
            .name("resonance-transport-shutdown".to_owned())
            .spawn(move || {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    let _ = runtime.block_on(transport.shutdown());
                }
            });
    }

    #[cfg(test)]
    pub(crate) async fn bind_loopback(
        secret_key: SecretKey,
        readiness_timeout: Duration,
        ready_immediately: bool,
    ) -> Result<Arc<Self>, OnboardingError> {
        Self::bind(
            secret_key,
            BindOptions {
                relay_mode: RelayMode::Disabled,
                loopback_only: true,
                readiness: if ready_immediately {
                    Readiness::Immediate
                } else {
                    Readiness::EndpointOnline(readiness_timeout)
                },
                require_relay_address: false,
                request_timeout: REQUEST_READ_TIMEOUT,
            },
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn endpoint_address(&self) -> EndpointAddr {
        self.endpoint.bound_sockets().into_iter().fold(
            EndpointAddr::new(self.endpoint.id()),
            EndpointAddr::with_ip_addr,
        )
    }

    #[cfg(test)]
    pub(crate) fn handler(&self) -> &Arc<OnboardingHandler> {
        &self.handler
    }

    #[cfg(test)]
    pub(crate) fn lifecycle_state(&self) -> u8 {
        self.lifecycle.state.load(Ordering::Acquire)
    }
}

struct BindOptions {
    relay_mode: RelayMode,
    loopback_only: bool,
    readiness: Readiness,
    require_relay_address: bool,
    request_timeout: Duration,
}

struct Lifecycle {
    state: AtomicU8,
    active: AtomicUsize,
    idle: Notify,
}

impl Lifecycle {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(RUNNING),
            active: AtomicUsize::new(0),
            idle: Notify::const_new(),
        }
    }

    fn begin(self: &Arc<Self>) -> Result<ActiveOperation, OnboardingError> {
        self.ensure_running()?;
        self.active.fetch_add(1, Ordering::AcqRel);
        if self.state.load(Ordering::Acquire) != RUNNING {
            self.finish();
            return Err(OnboardingError::Shutdown);
        }
        Ok(ActiveOperation(Arc::clone(self)))
    }

    fn ensure_running(&self) -> Result<(), OnboardingError> {
        if self.state.load(Ordering::Acquire) == RUNNING {
            Ok(())
        } else {
            Err(OnboardingError::Shutdown)
        }
    }

    fn stop(&self) -> bool {
        self.state
            .compare_exchange(RUNNING, STOPPING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn stopped(&self) {
        self.state.store(STOPPED, Ordering::Release);
        self.idle.notify_waiters();
    }

    fn finish(&self) {
        if self.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.idle.notify_waiters();
        }
    }

    async fn wait_for_idle(&self) {
        while self.active.load(Ordering::Acquire) != 0 {
            self.idle.notified().await;
        }
    }
}

pub(crate) struct ActiveOperation(Arc<Lifecycle>);

impl Drop for ActiveOperation {
    fn drop(&mut self) {
        self.0.finish();
    }
}

pub(crate) struct OutboundPermit {
    _operation: ActiveOperation,
    _permit: OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use iroh_base::SecretKey;

    use super::*;
    use crate::WorkspaceId;
    use crate::invitation::{sign_claim, sign_token};
    use crate::onboarding::handler::{MAX_INBOUND_HANDLERS, OnboardingHandler};
    use crate::onboarding::wire::{JoinRequest, encode_request};

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    #[tokio::test]
    async fn relay_disabled_endpoints_route_the_fixed_alpn_and_handle_one_stream() {
        let server = Transport::bind_loopback(secret(1), Duration::from_millis(20), true)
            .await
            .unwrap();
        let client = Transport::bind_loopback(secret(2), Duration::from_millis(20), true)
            .await
            .unwrap();
        let (connection, permit) = timeout(
            Duration::from_secs(3),
            client.connect(server.endpoint_address()),
        )
        .await
        .expect("loopback connect timed out")
        .unwrap();
        assert_eq!(connection.remote_id(), server.endpoint_id());

        let token =
            sign_token(&secret(1), WorkspaceId::from_bytes([3; 16]), 1, 2, [4; 32]).unwrap();
        let request = encode_request(&JoinRequest {
            claim: sign_claim(&secret(2), &token).unwrap(),
        })
        .unwrap();
        let (mut send, _receive) = connection.open_bi().await.unwrap();
        send.write_all(&request).await.unwrap();
        send.finish().unwrap();
        timeout(Duration::from_secs(3), server.handler().wait_for_handled(1))
            .await
            .expect("handler did not consume the request");
        drop(connection);
        drop(permit);

        server.shutdown().await.unwrap();
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn relay_disabled_readiness_times_out_without_starting_work() {
        let transport = Transport::bind_loopback(secret(3), Duration::from_millis(5), false)
            .await
            .unwrap();
        assert_eq!(
            transport.invitation_address().await.unwrap_err(),
            OnboardingError::Readiness
        );
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn inbound_and_outbound_limits_are_exact_and_peer_scoped() {
        let handler = OnboardingHandler::new(Duration::from_secs(1));
        let peer = secret(9).public();
        let first = handler.acquire(peer).unwrap();
        assert!(handler.acquire(peer).is_none());
        let mut permits = vec![first];
        for byte in 10..10 + MAX_INBOUND_HANDLERS - 1 {
            permits.push(handler.acquire(secret(byte as u8).public()).unwrap());
        }
        assert_eq!(handler.available_inbound_permits(), 0);
        assert!(handler.acquire(secret(99).public()).is_none());
        drop(permits);

        let transport = Transport::bind_loopback(secret(4), Duration::from_millis(20), true)
            .await
            .unwrap();
        let mut outbound = Vec::new();
        for _ in 0..MAX_OUTBOUND_JOINS {
            outbound.push(transport.begin_outbound().await.unwrap());
        }
        assert!(matches!(
            transport.begin_outbound().await,
            Err(OnboardingError::Overload)
        ));
        drop(outbound);
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn time_budgets_are_fixed_and_elapsed_work_is_classified_as_timeout() {
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(15));
        assert_eq!(REQUEST_READ_TIMEOUT, Duration::from_secs(10));
        assert_eq!(RESPONSE_IO_TIMEOUT, Duration::from_secs(30));
        assert_eq!(WHOLE_JOIN_TIMEOUT, Duration::from_secs(60));
        let classified = timeout(Duration::from_millis(1), std::future::pending::<()>())
            .await
            .map_err(|_| OnboardingError::Timeout);
        assert_eq!(classified.unwrap_err(), OnboardingError::Timeout);
    }

    #[tokio::test]
    async fn active_handler_is_cancelled_before_shutdown_completes() {
        let server = Transport::bind_loopback(secret(5), Duration::from_millis(20), true)
            .await
            .unwrap();
        let client = Transport::bind_loopback(secret(6), Duration::from_millis(20), true)
            .await
            .unwrap();
        let (connection, permit) = client.connect(server.endpoint_address()).await.unwrap();
        let (mut send, receive) = connection.open_bi().await.unwrap();
        send.write_all(&[0]).await.unwrap();

        timeout(Duration::from_secs(2), server.shutdown())
            .await
            .expect("active handler outlived shutdown")
            .unwrap();
        drop(receive);
        drop(send);
        drop(connection);
        drop(permit);
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_shutdown_observes_one_completed_close_and_rejects_new_work() {
        let transport = Transport::bind_loopback(secret(6), Duration::from_millis(20), true)
            .await
            .unwrap();
        let active = transport.begin_outbound().await.unwrap();
        let first = Arc::clone(&transport);
        let second = Arc::clone(&transport);
        let first_shutdown = tokio::spawn(async move { first.shutdown().await });
        while transport.lifecycle_state() == RUNNING {
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            transport.begin_outbound().await,
            Err(OnboardingError::Shutdown)
        ));
        let second_shutdown = tokio::spawn(async move { second.shutdown().await });
        drop(active);
        first_shutdown.await.unwrap().unwrap();
        second_shutdown.await.unwrap().unwrap();
        assert_eq!(transport.lifecycle_state(), STOPPED);
    }
}
