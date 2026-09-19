//! One redacted diagnostic line per internal failure, written where a cause
//! becomes a public error code.
//!
//! The public error enums are fieldless and `Copy`, so a cause cannot travel
//! with them. Rather than lose it, the boundary that erases it writes it once
//! to stderr. There is no logging dependency and no log sink: this is the whole
//! observability surface of the crate until `context/02-system/08-observability`
//! is authored and a facade is chosen.

/// Records an internal failure cause once, at the boundary where it becomes a
/// public code.
///
/// Only `Display` of the source is written; callers must not pass values
/// containing secrets (key bytes, grant plaintext, invitation secrets).
/// `rusqlite`, iroh and `std::io` errors qualify, and
/// `a_storage_failure_does_not_leak_bound_values` pins that assumption for the
/// one of them that formats SQL.
pub(crate) fn record(site: &'static str, source: &dyn std::fmt::Display) {
    eprintln!("teamarness-core fault site={site} cause={source}");
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

#[cfg(test)]
mod tests {

    /// `record` writes `Display` of the source. rusqlite's `Display` carries the
    /// statement text but never a bound value; if that ever changes, `record`
    /// must switch to writing the error's variant name only.
    #[test]
    fn a_storage_failure_does_not_leak_bound_values() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE secrets (value TEXT NOT NULL UNIQUE)")
            .unwrap();
        connection
            .execute(
                "INSERT INTO secrets (value) VALUES (?1)",
                ["SECRET-SENTINEL"],
            )
            .unwrap();
        let error = connection
            .execute(
                "INSERT INTO secrets (value) VALUES (?1)",
                ["SECRET-SENTINEL"],
            )
            .expect_err("the unique constraint rejects the second row");
        let displayed = error.to_string();
        assert!(
            !displayed.contains("SECRET-SENTINEL"),
            "rusqlite Display leaked a bound value: {displayed}"
        );
    }

    #[test]
    fn a_recorded_fault_keeps_the_code_it_maps_to() {
        let mapped: Result<(), &str> =
            Err(std::io::Error::other("cause")).map_err(fault!("storage failed"));
        assert_eq!(mapped, Err("storage failed"));
    }
}
