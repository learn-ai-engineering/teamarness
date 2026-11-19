//! Foreground Core/Host composition and owner-only local operator transport.
#![forbid(unsafe_code)]

mod administration;
pub mod application;
pub mod cli;
pub mod control;
pub mod input;
mod socket;

use serde::{Deserialize, Serialize};

/// Bounded local failure. Details belong to explicit observer output, not routine diagnostics.
/// An uncertain outcome must not be retried automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlError {
    pub code: ErrorCode,
    pub retryable: bool,
    pub outcome_uncertain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    UnsupportedVersion,
    FrameTooLarge,
    Timeout,
    Unavailable,
    UnsafePath,
    UnauthorizedPeer,
    Busy,
    ShuttingDown,
    Internal,
    ResponseTooLarge,
    CoreFailure,
    HostFailure,
    ReconciliationFailed,
}

/// Upstream failures are returned to the explicit observer, never copied to routine logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorDetails {
    Core {
        family: String,
        code: String,
    },
    Host {
        envelope: Box<teamarness_package_host::PackageErrorEnvelope>,
    },
    Joined {
        #[serde(rename = "membershipCommitted")]
        membership_committed: bool,
        workspace: serde_json::Value,
        reconciliation: Box<teamarness_package_host::PackageErrorEnvelope>,
    },
}

impl ControlError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            retryable: matches!(code, ErrorCode::Busy | ErrorCode::Unavailable),
            outcome_uncertain: false,
            details: None,
        }
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_string(self) {
            Ok(json) => f.write_str(&json),
            Err(_) => f.write_str("control_error"),
        }
    }
}
impl std::error::Error for ControlError {}
