//! Multi-profile system tests.
//!
//! These drive a real Core Runtime, a real Package Host and real Backend processes together.
//! They live outside every Package crate so that a Package depends only on the SDK and the
//! protocol, which is what a Package an author writes can depend on (ARC-10).
//!
//! The Backends they drive are bin targets this crate declares over each Package's own
//! `src/main.rs`, so cargo rebuilds them from current sources for every run. The library holds
//! only the Exhibit fixtures, which the test-only Exhibits Consumer and the tests share;
//! everything else is in `tests/`.
#![forbid(unsafe_code)]

pub mod exhibit_fixtures;
