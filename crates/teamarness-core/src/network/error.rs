//! What the shared Endpoint and Router can fail at.
//!
//! `network` is a leaf: it names no protocol and no domain error. Each caller
//! maps these at its own boundary — onboarding to `OnboardingError`,
//! synchronization to a session error.

/// A failure of the shared Endpoint or Router itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkError {
    /// The endpoint could not be bound.
    Readiness,
    /// The router did not shut down cleanly.
    Shutdown,
}
