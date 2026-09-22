//! Multi-profile system tests.
//!
//! These drive a real Core Runtime, a real Package Host and a real
//! Conversations Backend process together. They live outside every Package
//! crate so that a Package depends only on the SDK and the protocol, which is
//! what a Package an author writes can depend on (ARC-10).
//!
//! The Backend they drive is the bin target this crate declares over the
//! Package's own `src/main.rs`, so cargo rebuilds it from current sources for
//! every run. The library itself is empty: everything is in `tests/`.
