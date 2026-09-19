//! Multi-Installation runtime tests.
//!
//! Every test here is hermetic and exercises real components: real
//! [`CoreRuntime`](super::CoreRuntime) instances on temporary directories, real
//! Iroh endpoints bound to loopback with an in-memory address book, and real
//! SQLite. Fakes appear only to inject a fault or to drive a raw probe.

mod async_boundary;
mod cluster;
mod membership_replication;
mod order_keys;
mod record_delivery;
mod recovery;
mod sync_authorization;
mod sync_bounds;
