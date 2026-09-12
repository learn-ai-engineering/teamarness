use std::error::Error;
use std::fmt;

/// An error returned while parsing a public domain value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueParseError {
    value_name: &'static str,
}

impl ValueParseError {
    pub(crate) const fn new(value_name: &'static str) -> Self {
        Self { value_name }
    }
}

impl fmt::Display for ValueParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}", self.value_name)
    }
}

impl Error for ValueParseError {}

macro_rules! error_family {
    ($name:ident, $summary:literal) => {
        #[doc = $summary]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[non_exhaustive]
        pub enum $name {
            /// The operation is part of the public contract but is not available in this scaffold.
            RuntimeUnavailable,
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("the core runtime is not available in the repository scaffold")
            }
        }

        impl Error for $name {}
    };
}

error_family!(ConfigurationError, "A runtime configuration error.");

/// A durable core storage or recovery error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageError {
    /// The private data directory is not a directory.
    InvalidDataDirectory,
    /// A filesystem operation failed.
    Io,
    /// Restrictive filesystem permissions could not be applied.
    Permission,
    /// SQLite could not open or operate on the core database.
    Database,
    /// An ordered migration could not be applied.
    Migration,
    /// The database schema is newer than this runtime supports.
    UnsupportedSchema,
    /// Durable installation identity records failed authentication.
    CorruptIdentity,
    /// SQLite may have committed, so the active runtime cannot safely continue.
    UncertainOutcome,
    /// The active runtime is poisoned and must be reopened.
    RuntimePoisoned,
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidDataDirectory => "the private data path is not a directory",
            Self::Io => "private storage filesystem operation failed",
            Self::Permission => "private storage permissions could not be restricted",
            Self::Database => "core database operation failed",
            Self::Migration => "core database migration failed",
            Self::UnsupportedSchema => "core database schema is newer than this runtime",
            Self::CorruptIdentity => "durable installation identity is corrupt",
            Self::UncertainOutcome => "database commit outcome is uncertain",
            Self::RuntimePoisoned => "the runtime must be reopened before another command",
        };
        formatter.write_str(message)
    }
}

impl Error for StorageError {}

/// An installation identity or signing error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityError {
    /// A display name violated the documented length or character rules.
    InvalidDisplayName,
    /// A signing context violated the documented domain-tag rules.
    InvalidSigningContext,
    /// Signature verification failed.
    InvalidSignature,
    /// A payload cannot be represented by the versioned signing envelope.
    PayloadTooLarge,
    /// Durable storage failed while changing identity metadata.
    Storage(StorageError),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDisplayName => formatter.write_str("invalid installation display name"),
            Self::InvalidSigningContext => formatter.write_str("invalid signing context"),
            Self::InvalidSignature => formatter.write_str("invalid signature"),
            Self::PayloadTooLarge => formatter.write_str("signing payload is too large"),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl Error for IdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

error_family!(WorkspaceError, "A workspace command or query error.");
error_family!(MembershipError, "A canonical membership error.");
error_family!(
    InvitationError,
    "An invitation issue, claim, or redemption error."
);
error_family!(
    SubscriptionError,
    "A membership transition subscription error."
);

/// An error returned while opening the core runtime.
#[derive(Debug)]
#[non_exhaustive]
pub enum CoreError {
    /// Runtime configuration could not be accepted.
    Configuration(ConfigurationError),
    /// Installation identity initialization failed.
    Identity(IdentityError),
    /// Durable storage initialization or recovery failed.
    Storage(StorageError),
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(formatter),
            Self::Identity(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl From<StorageError> for CoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Identity(error) => Some(error),
            Self::Storage(error) => Some(error),
        }
    }
}
