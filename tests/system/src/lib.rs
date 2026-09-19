//! Multi-profile system tests.
//!
//! These drive a real Core Runtime, a real Package Host and the staged
//! Conversations Backend together. They live outside every Package crate so
//! that a Package depends only on the SDK and the protocol, which is what a
//! Package an author writes can depend on (ARC-10).
//!
//! The crate itself is empty: everything is in `tests/`.
