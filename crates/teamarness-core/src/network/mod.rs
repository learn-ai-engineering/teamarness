//! Shared Core networking primitives.
//!
//! This module is the sole owner of the installation Endpoint and Router, and
//! it is a leaf: it names no protocol, no ALPN and no Core domain module. Every
//! protocol is handed to [`Network::bind`] as an opaque registration by the
//! runtime composition point, so a runtime never accidentally creates a second
//! endpoint for replication and this module never depends on what it carries.

mod error;

use std::sync::Arc;

use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode};
#[cfg(any(test, feature = "test-hooks"))]
use iroh_base::EndpointAddr;
use iroh_base::{PublicKey, SecretKey};
use tokio::sync::Semaphore;

use crate::diag::fault;
pub(crate) use error::NetworkError;

/// One extra Core protocol the runtime composition point registers with the
/// shared Router. Keeping it opaque here is what lets this module own the
/// Endpoint and Router while naming no protocol or synchronization type.
pub(crate) type ProtocolRegistration = (Vec<u8>, Box<dyn iroh::protocol::DynProtocolHandler>);
pub(crate) const MAX_OUTBOUND_PEERS: usize = 4;

pub(crate) struct Network {
    endpoint: Endpoint,
    router: Router,
    outbound: Arc<Semaphore>,
    /// A second peer budget, for the Core protocols registered beside package
    /// sync. A session stalled on one protocol cannot starve the other.
    outbound_extra: Arc<Semaphore>,
    /// Loopback runtimes have no relay and no discovery, so tests register peer
    /// addresses here instead. Production binds with `loopback_only == false`,
    /// and the endpoint holds its own clone of the lookup, so only
    /// `remember_peer_address` reads this handle.
    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]
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

/// The relay mode a bind really uses.
///
/// `loopback_only` and `relay_mode` were two independent parameters, so a
/// caller could ask for loopback and still get `RelayMode::Default` — a
/// "loopback" endpoint that connects to a public n0 relay. The suite was
/// hermetic only because the one loopback caller happened to pass `Disabled`
/// next to it, which is a convention in a sibling module rather than an
/// invariant of this one. Loopback now means relay-free by construction,
/// whatever the caller asked for, and
/// `loopback_is_relay_free_whatever_the_caller_asks_for` reads the relay set
/// this produces rather than the argument a call site passed.
fn effective_relay_mode(relay_mode: RelayMode, loopback_only: bool) -> RelayMode {
    if loopback_only {
        RelayMode::Disabled
    } else {
        relay_mode
    }
}

impl Network {
    /// Binds the one Endpoint and Router this Installation uses.
    ///
    /// `protocols` is registered in the order given, and the ALPN list the
    /// endpoint advertises is exactly those ALPNs in that order.
    pub(crate) async fn bind(
        secret_key: SecretKey,
        relay_mode: RelayMode,
        loopback_only: bool,
        protocols: Vec<ProtocolRegistration>,
    ) -> Result<Arc<Self>, NetworkError> {
        let configured_alpns: Vec<Vec<u8>> =
            protocols.iter().map(|(alpn, _)| alpn.clone()).collect();
        let mut builder = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .alpns(configured_alpns.clone())
            .relay_mode(effective_relay_mode(relay_mode, loopback_only));
        let address_book = if loopback_only {
            let lookup = iroh::address_lookup::memory::MemoryLookup::new();
            builder = builder
                // The preset installs three address-lookup services that reach
                // the public internet: a pkarr publisher and resolver against
                // n0's relay, and n0's DNS. `address_lookup` pushes onto that
                // list rather than replacing it, and `clear_ip_transports`
                // clears IP transports, not lookup services — so a loopback
                // Installation still published its identity and addresses to a
                // third party over HTTPS. Clearing the list first is what makes
                // "loopback" mean nothing off this machine is contacted.
                .clear_address_lookup()
                .address_lookup(lookup.clone())
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .map_err(fault!(NetworkError::Readiness))?;
            Some(lookup)
        } else {
            None
        };
        let endpoint = builder
            .bind()
            .await
            .map_err(fault!(NetworkError::Readiness))?;
        let mut builder = Router::builder(endpoint.clone());
        for (alpn, handler) in protocols {
            builder = builder.accept(alpn, handler);
        }
        let router = builder.spawn();
        Ok(Arc::new(Self {
            endpoint,
            router,
            outbound: Arc::new(Semaphore::new(MAX_OUTBOUND_PEERS)),
            outbound_extra: Arc::new(Semaphore::new(MAX_OUTBOUND_PEERS)),
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
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn remember_peer_address(&self, address: EndpointAddr) {
        if let Some(book) = &self.address_book {
            book.add_endpoint_info(address);
        }
    }

    /// Runs one outbound session on the shared endpoint within the primary
    /// outbound peer budget. The session itself is supplied by the caller, so
    /// this module never names the protocol it carries.
    pub(crate) async fn run_outbound<T, E, Fut>(
        &self,
        overloaded: E,
        session: impl FnOnce(Endpoint) -> Fut,
    ) -> Result<T, E>
    where
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        let Ok(_permit) = self.outbound.clone().try_acquire_owned() else {
            return Err(overloaded);
        };
        session(self.endpoint.clone()).await
    }

    /// Runs one outbound session within the second peer budget, which the Core
    /// protocols registered beside the primary one share. A session stalled on
    /// one budget cannot starve the other.
    pub(crate) async fn run_extra_outbound<T, E, Fut>(
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

    pub(crate) async fn shutdown(&self) -> Result<(), NetworkError> {
        self.router
            .shutdown()
            .await
            .map_err(fault!(NetworkError::Shutdown))
    }
}

#[cfg(test)]
mod tests {
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

    fn opaque(alpn: &[u8]) -> ProtocolRegistration {
        (
            alpn.to_vec(),
            Box::new(OpaqueProtocol) as Box<dyn iroh::protocol::DynProtocolHandler>,
        )
    }

    #[tokio::test]
    async fn one_router_advertises_the_composed_protocols_in_order() {
        let network = Network::bind(
            secret(1),
            RelayMode::Disabled,
            true,
            vec![
                opaque(b"teamarness/first-protocol/1"),
                opaque(b"teamarness/second-protocol/1"),
            ],
        )
        .await
        .unwrap();
        assert_eq!(network.endpoint_id(), secret(1).public());
        assert_eq!(
            network.alpns(),
            vec![
                b"teamarness/first-protocol/1".to_vec(),
                b"teamarness/second-protocol/1".to_vec(),
            ]
        );
        network.shutdown().await.unwrap();
    }

    /// A loopback Installation contacts nothing off this machine.
    ///
    /// The preset's pkarr publisher, pkarr resolver and n0 DNS lookup each
    /// reach the public internet, and `address_lookup` adds to that list rather
    /// than replacing it. Counting the services is the cheap guard: one is the
    /// in-memory address book and nothing else.
    #[tokio::test]
    async fn a_loopback_endpoint_carries_only_its_in_memory_address_book() {
        let network = Network::bind(secret(3), RelayMode::Disabled, true, Vec::new())
            .await
            .unwrap();
        assert_eq!(
            network.endpoint.address_lookup().unwrap().len(),
            1,
            "a loopback endpoint keeps only the in-memory address book"
        );
        network.shutdown().await.unwrap();
    }

    /// Hermeticity is a property of `bind`, not of what its call sites pass.
    ///
    /// The address-lookup count alone passed unchanged while a "loopback"
    /// endpoint was `Connected` to `usw1-1.relay.n0.iroh.link`: a relay is not
    /// an address-lookup service, and nothing forced the two arguments to
    /// agree. This asks for the relay set a loopback bind really uses, with the
    /// caller asking for production relays on purpose.
    #[test]
    fn loopback_is_relay_free_whatever_the_caller_asks_for() {
        assert!(
            effective_relay_mode(RelayMode::Default, true)
                .relay_map()
                .is_empty(),
            "a loopback bind reaches no relay, however it was called"
        );
        assert!(
            effective_relay_mode(RelayMode::Staging, true)
                .relay_map()
                .is_empty()
        );
        assert!(
            !effective_relay_mode(RelayMode::Default, false)
                .relay_map()
                .is_empty(),
            "a production bind still reaches the relays it asked for"
        );
    }

    #[tokio::test]
    async fn each_outbound_budget_refuses_on_its_own() {
        let network = Network::bind(secret(2), RelayMode::Disabled, true, Vec::new())
            .await
            .unwrap();
        let mut held = Vec::new();
        for _ in 0..MAX_OUTBOUND_PEERS {
            held.push(network.outbound.clone().try_acquire_owned().unwrap());
        }
        assert_eq!(
            network
                .run_outbound("overloaded", |_| async { Ok::<(), _>(()) })
                .await,
            Err("overloaded")
        );
        // The second budget is untouched by the first being exhausted.
        assert_eq!(
            network
                .run_extra_outbound("overloaded", |_| async { Ok::<(), &str>(()) })
                .await,
            Ok(())
        );
        network.shutdown().await.unwrap();
    }
}
