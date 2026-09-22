//! Installation identity: the display name and the signing seam.

use super::*;

impl CoreRuntime {
    /// Returns this installation's validated public identity.
    #[must_use]
    pub const fn public_identity(&self) -> PublicIdentity {
        self.public_identity
    }

    /// Returns a snapshot of this installation's validated display name.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .display_name
            .clone()
    }

    /// Updates this installation's display name.
    ///
    /// Names contain 1–64 Unicode alphanumeric characters, spaces, hyphens,
    /// underscores, apostrophes, or periods, with no surrounding whitespace.
    pub fn set_display_name(&self, display_name: &str) -> Result<(), IdentityError> {
        crate::identity::validate_display_name(display_name)?;
        let mut state = self.lock_identity_state()?;
        match crate::storage::update_display_name(&mut state.connection, display_name) {
            Ok(()) => {
                state.display_name = display_name.to_owned();
                Ok(())
            }
            Err(error) => {
                if error == StorageError::UncertainOutcome {
                    state.poisoned = true;
                }
                Err(IdentityError::Storage(error))
            }
        }
    }

    /// Signs bytes under a validated, versioned, length-delimited domain context.
    pub fn sign(
        &self,
        context: &SigningContext,
        payload: &[u8],
    ) -> Result<PublicSignature, IdentityError> {
        let state = self.lock_identity_state()?;
        crate::identity::sign(&state.secret_key, context.as_str(), payload)
            .map(|signature| PublicSignature(signature.to_bytes()))
    }

    fn lock_identity_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RuntimeState>, IdentityError> {
        let state = self
            .state
            .lock()
            .map_err(|_| IdentityError::Storage(StorageError::RuntimePoisoned))?;
        if state.poisoned {
            return Err(IdentityError::Storage(StorageError::RuntimePoisoned));
        }
        Ok(state)
    }
}
