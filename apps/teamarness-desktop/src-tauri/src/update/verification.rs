use super::{ErrorCategory, Metadata};
use tauri_plugin_updater::{Update, Updater};

pub(super) trait Verifier: Send + Sync {
    fn verify(&self, metadata: &Metadata, bytes: &[u8]) -> Result<Update, ErrorCategory>;
}

pub(super) struct OfficialVerifier(pub Updater);
impl Verifier for OfficialVerifier {
    fn verify(&self, metadata: &Metadata, bytes: &[u8]) -> Result<Update, ErrorCategory> {
        let update = self
            .0
            .restore_update(metadata.raw.clone())
            .map_err(|_| ErrorCategory::Verification)?
            .ok_or(ErrorCategory::Verification)?;
        if update.version != metadata.version.to_string()
            || update.download_url != metadata.url
            || update.signature != metadata.signature
        {
            return Err(ErrorCategory::Verification);
        }
        update
            .verify(bytes)
            .map_err(|_| ErrorCategory::Verification)?;
        Ok(update)
    }
}
