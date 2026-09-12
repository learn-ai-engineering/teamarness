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

/// An installation identity or signing error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityError {
    /// A signing context was empty or contained non-ASCII characters.
    InvalidSigningContext,
    /// The operation is part of the public contract but is not available in this scaffold.
    RuntimeUnavailable,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSigningContext => formatter.write_str("invalid signing context"),
            Self::RuntimeUnavailable => {
                formatter.write_str("the core runtime is not available in the repository scaffold")
            }
        }
    }
}

impl Error for IdentityError {}

error_family!(WorkspaceError, "A workspace command or query error.");
error_family!(MembershipError, "A canonical membership error.");
error_family!(
    InvitationError,
    "An invitation issue, claim, or redemption error."
);
error_family!(StorageError, "A durable core storage or recovery error.");
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

impl Error for CoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Identity(error) => Some(error),
            Self::Storage(error) => Some(error),
        }
    }
}
