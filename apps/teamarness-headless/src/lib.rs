//! Foreground Core/Host composition and owner-only local operator transport.
#![forbid(unsafe_code)]

pub mod application;
pub mod cli;
pub mod control;
mod socket;

use serde::{Deserialize, Serialize};

/// Bounded, payload-free local failure. An uncertain outcome must not be retried automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlError {
    pub code: ErrorCode,
    pub retryable: bool,
    pub outcome_uncertain: bool,
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
}

impl ControlError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            retryable: matches!(code, ErrorCode::Busy | ErrorCode::Unavailable),
            outcome_uncertain: false,
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
