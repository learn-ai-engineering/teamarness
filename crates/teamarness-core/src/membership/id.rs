use std::fmt;
use std::str::FromStr;

use p2panda_core::Hash;

use crate::error::ValueParseError;

/// The content-derived identifier of a canonical membership operation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MembershipOperationId(pub(crate) Hash);

impl MembershipOperationId {
    /// Derives an operation identifier from exact canonical unsigned bytes.
    #[must_use]
    pub fn digest(canonical_unsigned_bytes: impl AsRef<[u8]>) -> Self {
        Self(Hash::digest(canonical_unsigned_bytes))
    }

    pub(crate) const fn from_hash(hash: Hash) -> Self {
        Self(hash)
    }

    pub(crate) const fn as_hash(self) -> Hash {
        self.0
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for MembershipOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MembershipOperationId")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for MembershipOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for MembershipOperationId {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value
            .parse::<Hash>()
            .map(Self)
            .map_err(|_| ValueParseError::new("membership operation ID"))
    }
}
