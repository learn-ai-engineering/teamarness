use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use iroh::RelayMode;
use iroh::endpoint::Connection;
use iroh_base::{EndpointAddr, SecretKey};
use tokio::sync::{Notify, OnceCell, OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, timeout, timeout_at};

use super::handler::{IssuerService, OnboardingHandler};
use crate::OnboardingError;
use crate::network::Network;

pub(crate) const ONBOARDING_ALPN: &[u8] = b"teamarness/workspace-join/1";
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
    network: Arc<Network>,
    handler: Arc<OnboardingHandler>,
    lifecycle: Arc<Lifecycle>,
    outbound: Arc<Semaphore>,
    shutdown: OnceCell<Result<(), OnboardingError>>,
    readiness: Readiness,
    require_relay_address: bool,
    #[cfg(test)]
    timeout_next_join: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    invalidate_next_bootstrap: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pause_after_response: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    hold_after_response: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    post_response_held: Notify,
    #[cfg(test)]
    release_post_response: Notify,
    #[cfg(test)]
    hold_after_invitation_readiness: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    invitation_readiness_held: Notify,
    #[cfg(test)]
    release_invitation_readiness: Notify,
    #[cfg(test)]
    post_response_block_ms: std::sync::atomic::AtomicU64,
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
        issuer: IssuerService,
        receiver: Option<Arc<dyn crate::synchronization::SyncReceiver>>,
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
            Some(issuer),
            receiver,
        )
        .await
    }

    async fn bind(
        secret_key: SecretKey,
        options: BindOptions,
        issuer: Option<IssuerService>,
        receiver: Option<Arc<dyn crate::synchronization::SyncReceiver>>,
    ) -> Result<Arc<Self>, OnboardingError> {
        let handler = Arc::new(match issuer {
            Some(issuer) => OnboardingHandler::with_issuer(options.request_timeout, issuer),
            None => OnboardingHandler::new(options.request_timeout),
        });
        let network = Network::bind(
            secret_key,
            options.relay_mode,
            options.loopback_only,
            Arc::clone(&handler),
            receiver,
        )
        .await?;
        Ok(Arc::new(Self {
            network,
            handler,
            lifecycle: Arc::new(Lifecycle::new()),
            outbound: Arc::new(Semaphore::new(MAX_OUTBOUND_JOINS)),
            shutdown: OnceCell::new(),
            readiness: options.readiness,
            require_relay_address: options.require_relay_address,
            #[cfg(test)]
            timeout_next_join: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            invalidate_next_bootstrap: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            pause_after_response: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            hold_after_response: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            post_response_held: Notify::new(),
            #[cfg(test)]
            release_post_response: Notify::new(),
            #[cfg(test)]
            hold_after_invitation_readiness: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            invitation_readiness_held: Notify::new(),
            #[cfg(test)]
            release_invitation_readiness: Notify::new(),
            #[cfg(test)]
            post_response_block_ms: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    pub(crate) fn endpoint_id(&self) -> iroh_base::PublicKey {
        self.network.endpoint_id()
    }

    pub(crate) async fn begin_invitation(&self) -> Result<InvitationOperation, OnboardingError> {
        let operation = self.lifecycle.begin()?;
        match self.readiness {
            Readiness::EndpointOnline(duration) => {
                timeout(duration, self.network.endpoint().online())
                    .await
                    .map_err(|_| OnboardingError::Readiness)?
            }
            #[cfg(test)]
            Readiness::Immediate => {}
        }
        self.lifecycle.ensure_running()?;
        let address = self.network.endpoint().addr();
        if self.require_relay_address
            && !address.addrs.iter().any(iroh_base::TransportAddr::is_relay)
        {
            return Err(OnboardingError::Readiness);
        }
        Ok(InvitationOperation {
            address,
            _operation: operation,
        })
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
        _permit: &OutboundPermit,
    ) -> Result<Connection, OnboardingError> {
        timeout(
            CONNECT_TIMEOUT,
            self.network.endpoint().connect(address, ONBOARDING_ALPN),
        )
        .await
        .map_err(|_| OnboardingError::Timeout)?
        .map_err(|_| OnboardingError::Authentication)
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

    pub(crate) async fn whole_join_until<T>(
        &self,
        future: impl Future<Output = Result<T, OnboardingError>>,
        deadline: std::time::Instant,
    ) -> Result<T, OnboardingError> {
        #[cfg(test)]
        if self.timeout_next_join.swap(false, Ordering::AcqRel) {
            return Err(OnboardingError::Timeout);
        }
        timeout_at(Instant::from_std(deadline), future)
            .await
            .map_err(|_| OnboardingError::Timeout)?
    }

    pub(crate) async fn send_sync_batch(
        &self,
        address: EndpointAddr,
        batch: &crate::synchronization::SyncBatch,
    ) -> Result<usize, OnboardingError> {
        self.network.send_batch(address, batch).await
    }

    pub(crate) async fn send_log_sync(
        &self,
        address: EndpointAddr,
        receiver: Arc<dyn crate::synchronization::SyncReceiver>,
        workspace: crate::WorkspaceId,
        package: String,
        peer: crate::PublicIdentity,
    ) -> Result<usize, OnboardingError> {
        self.network
            .send_log_sync(address, receiver, workspace, package, peer)
            .await
    }

    pub(crate) async fn shutdown(&self) -> Result<(), OnboardingError> {
        self.lifecycle.stop();
        self.handler.stop();
        *self
            .shutdown
            .get_or_init(|| async {
                self.lifecycle.wait_for_idle().await;
                let result = self.network.shutdown().await;
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
            .name("teamarness-transport-shutdown".to_owned())
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
        Self::bind_loopback_inner(secret_key, readiness_timeout, ready_immediately, None, None)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn bind_loopback_with_issuer(
        secret_key: SecretKey,
        readiness_timeout: Duration,
        ready_immediately: bool,
        issuer: IssuerService,
    ) -> Result<Arc<Self>, OnboardingError> {
        Self::bind_loopback_inner(
            secret_key,
            readiness_timeout,
            ready_immediately,
            Some(issuer),
            None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn bind_loopback_with_issuer_and_receiver(
        secret_key: SecretKey,
        readiness_timeout: Duration,
        ready_immediately: bool,
        issuer: IssuerService,
        receiver: Arc<dyn crate::synchronization::SyncReceiver>,
    ) -> Result<Arc<Self>, OnboardingError> {
        Self::bind_loopback_inner(
            secret_key,
            readiness_timeout,
            ready_immediately,
            Some(issuer),
            Some(receiver),
        )
        .await
    }

    #[cfg(test)]
    async fn bind_loopback_inner(
        secret_key: SecretKey,
        readiness_timeout: Duration,
        ready_immediately: bool,
        issuer: Option<IssuerService>,
        receiver: Option<Arc<dyn crate::synchronization::SyncReceiver>>,
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
            issuer,
            receiver,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn endpoint_address(&self) -> EndpointAddr {
        self.network.endpoint().bound_sockets().into_iter().fold(
            EndpointAddr::new(self.network.endpoint().id()),
            EndpointAddr::with_ip_addr,
        )
    }

    #[cfg(test)]
    pub(crate) fn handler(&self) -> &Arc<OnboardingHandler> {
        &self.handler
    }

    #[cfg(test)]
    pub(crate) fn force_next_join_timeout(&self) {
        self.timeout_next_join.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn force_next_invalid_bootstrap(&self) {
        self.invalidate_next_bootstrap
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn force_next_post_response_pause(&self) {
        self.pause_after_response.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) async fn pause_after_response(&self) {
        if self.pause_after_response.swap(false, Ordering::AcqRel) {
            std::future::pending::<()>().await;
        }
        if self.hold_after_response.swap(false, Ordering::AcqRel) {
            self.post_response_held.notify_one();
            self.release_post_response.notified().await;
        }
    }

    #[cfg(test)]
    pub(crate) fn hold_next_join_after_response(&self) {
        self.hold_after_response.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_post_response_hold(&self) {
        self.post_response_held.notified().await;
    }

    #[cfg(test)]
    pub(crate) fn release_post_response_hold(&self) {
        self.release_post_response.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn hold_next_invitation_after_readiness(&self) {
        self.hold_after_invitation_readiness
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) async fn pause_after_invitation_readiness(&self) {
        if self
            .hold_after_invitation_readiness
            .swap(false, Ordering::AcqRel)
        {
            self.invitation_readiness_held.notify_one();
            self.release_invitation_readiness.notified().await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_invitation_readiness_hold(&self) {
        self.invitation_readiness_held.notified().await;
    }

    #[cfg(test)]
    pub(crate) fn release_invitation_readiness_hold(&self) {
        self.release_invitation_readiness.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn force_next_post_response_block(&self, duration: Duration) {
        self.post_response_block_ms.store(
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            Ordering::Release,
        );
    }

    #[cfg(test)]
    pub(crate) fn block_after_response(&self) {
        let milliseconds = self.post_response_block_ms.swap(0, Ordering::AcqRel);
        if milliseconds != 0 {
            std::thread::sleep(Duration::from_millis(milliseconds));
        }
    }

    #[cfg(test)]
    pub(crate) fn alter_received_bootstrap(&self, frame: &mut [u8]) {
        if self.invalidate_next_bootstrap.swap(false, Ordering::AcqRel)
            && let Some(version) = frame.get_mut(4)
        {
            *version = version.wrapping_add(1);
        }
    }

    #[cfg(test)]
    pub(crate) fn lifecycle_state(&self) -> u8 {
        self.lifecycle.state.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn active_operations(&self) -> usize {
        self.lifecycle.active.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn available_outbound_permits(&self) -> usize {
        self.outbound.available_permits()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_idle_waiter(&self) {
        self.lifecycle.idle_waiting.notified().await;
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
    #[cfg(test)]
    idle_waiting: Notify,
}

impl Lifecycle {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(RUNNING),
            active: AtomicUsize::new(0),
            idle: Notify::const_new(),
            #[cfg(test)]
            idle_waiting: Notify::const_new(),
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
        self.wait_for_idle_with_hook(std::future::ready(())).await;
    }

    async fn wait_for_idle_with_hook(&self, hook: impl Future<Output = ()>) {
        let mut hook = Some(hook);
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(hook) = hook.take() {
                hook.await;
            }
            if self.active.load(Ordering::Acquire) == 0 {
                return;
            }
            #[cfg(test)]
            self.idle_waiting.notify_one();
            notified.await;
        }
    }
}

pub(crate) struct ActiveOperation(Arc<Lifecycle>);

impl Drop for ActiveOperation {
    fn drop(&mut self) {
        self.0.finish();
    }
}

pub(crate) struct InvitationOperation {
    address: EndpointAddr,
    _operation: ActiveOperation,
}

impl InvitationOperation {
    pub(crate) fn address(&self) -> EndpointAddr {
        self.address.clone()
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
        let permit = client.begin_outbound().await.unwrap();
        let connection = timeout(
            Duration::from_secs(3),
            client.connect(server.endpoint_address(), &permit),
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
        assert!(matches!(
            transport.begin_invitation().await,
            Err(OnboardingError::Readiness)
        ));
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
        let permit = client.begin_outbound().await.unwrap();
        let connection = client
            .connect(server.endpoint_address(), &permit)
            .await
            .unwrap();
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

    #[tokio::test]
    async fn idle_wait_registers_before_the_final_operation_finishes() {
        let lifecycle = Arc::new(Lifecycle::new());
        let operation = lifecycle.begin().unwrap();
        let count_checked = Arc::new(Notify::new());
        let continue_wait = Arc::new(Notify::new());
        let waiting_lifecycle = Arc::clone(&lifecycle);
        let waiting_count_checked = Arc::clone(&count_checked);
        let waiting_continue = Arc::clone(&continue_wait);
        let waiter = tokio::spawn(async move {
            waiting_lifecycle
                .wait_for_idle_with_hook(async move {
                    waiting_count_checked.notify_one();
                    waiting_continue.notified().await;
                })
                .await;
        });

        count_checked.notified().await;
        drop(operation);
        continue_wait.notify_one();

        timeout(Duration::from_secs(1), waiter)
            .await
            .expect("idle wait lost the final operation notification")
            .unwrap();
    }
}
