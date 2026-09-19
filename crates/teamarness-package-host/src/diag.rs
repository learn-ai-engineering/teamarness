//! One redacted diagnostic line per internal failure, written where a cause
//! becomes a stable `PackageErrorCode`.
//!
//! The Host keeps its own copy rather than depending on Core internals; the two
//! crates share the shape, not the code.

/// Records an internal failure cause once, at the boundary where it becomes a
/// public code.
///
/// Only `Display` of the source is written; callers must not pass values
/// containing secrets. `rusqlite` and `std::io` errors qualify.
pub(crate) fn record(site: &'static str, source: &dyn std::fmt::Display) {
    eprintln!("teamarness-package-host fault site={site} cause={source}");
}

/// Builds a `map_err` closure that records the erased cause and returns `$code`.
macro_rules! fault {
    ($code:expr) => {
        |source| {
            $crate::diag::record(concat!(file!(), ":", line!()), &source);
            $code
        }
    };
}

pub(crate) use fault;
