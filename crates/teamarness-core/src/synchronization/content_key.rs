//! The per-scope content key, and the only shape it takes inside Core.
//!
//! The key is stored unencrypted in the private data directory (ADR 0017), so
//! the guarantee this type carries is narrow and deliberate: the bytes live in
//! a zeroizing buffer, they never reach a serializer, and they never reach a
//! formatter. Zeroization is best effort — a move, SQLite's page cache, or the
//! operating system's file cache can still leave a copy behind.

use p2panda_core::Hash;
use zeroize::Zeroizing;

use super::SynchronizationError;
use crate::error::StorageError;

/// One scope's XChaCha20-Poly1305 content key.
#[derive(Clone)]
pub(crate) struct ContentKey(Zeroizing<[u8; 32]>);

impl ContentKey {
    /// Mints a scope's first content key.
    pub(crate) fn generate() -> Result<Self, StorageError> {
        let mut key = Zeroizing::new([0_u8; 32]);
        getrandom::fill(key.as_mut_slice()).map_err(|_| StorageError::Database)?;
        Ok(Self(key))
    }

    /// Takes ownership of a key read from `sync_scopes.content_key`.
    ///
    /// The row's `Vec` is moved into a zeroizing buffer before it is measured,
    /// so a wrong-width value is cleared as well as refused.
    pub(crate) fn from_column(value: Vec<u8>) -> Result<Self, SynchronizationError> {
        let owned = Zeroizing::new(value);
        let mut key = Zeroizing::new([0_u8; 32]);
        if owned.len() != 32 {
            return Err(SynchronizationError::CorruptRecord);
        }
        key.copy_from_slice(owned.as_slice());
        Ok(Self(key))
    }

    /// Adopts a key recovered from a protection control.
    pub(crate) fn from_opened(key: Zeroizing<[u8; 32]>) -> Self {
        Self(key)
    }

    /// `BLAKE3(key)`, the public identifier carried in every sealed envelope.
    pub(crate) fn id(&self) -> [u8; 32] {
        *Hash::digest(self.0.as_slice()).as_bytes()
    }

    /// The raw key, for the one cryptographic call that needs it.
    pub(crate) fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for ContentKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ContentKey(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_key_debug_does_not_print_bytes() {
        // A recognisable value: were the buffer printed at all, "173" would be
        // in the output whichever way the bytes were formatted.
        let key = ContentKey::from_column(vec![173; 32]).unwrap();

        assert_eq!(format!("{key:?}"), "ContentKey(..)");
        assert!(!format!("{key:#?}").contains("173"));
        // A derived Debug on a struct that holds one leaks nothing either.
        #[derive(Debug)]
        struct Holder {
            #[expect(dead_code, reason = "read only by the derived Debug")]
            content_key: ContentKey,
        }
        assert!(!format!("{:?}", Holder { content_key: key }).contains("173"));
    }

    #[test]
    fn a_column_of_the_wrong_width_is_refused() {
        assert!(ContentKey::from_column(vec![0; 31]).is_err());
        assert!(ContentKey::from_column(vec![0; 33]).is_err());
        assert!(ContentKey::from_column(vec![0; 32]).is_ok());
    }

    #[test]
    fn the_identifier_is_the_digest_of_the_key() {
        let key = ContentKey::from_column(vec![7; 32]).unwrap();

        assert_eq!(key.id(), *Hash::digest([7_u8; 32]).as_bytes());
        assert_eq!(key.id(), key.clone().id());
    }
}
