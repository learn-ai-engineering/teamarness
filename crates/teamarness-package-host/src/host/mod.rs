//! The `PackageHost` command handlers, grouped by concern.
//!
//! [`crate::PackageHost::execute`] is a dispatcher over these: every arm is one
//! named handler. The three concerns are the Instance lifecycle, UI mounts and
//! UI requests.

pub(crate) mod lifecycle;
pub(crate) mod personal;
pub(crate) mod ui_mounts;
pub(crate) mod ui_requests;
