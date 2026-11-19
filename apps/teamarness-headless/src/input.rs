//! Bounded secret input. No input bytes or operating-system errors reach diagnostics.
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::{ControlError, ErrorCode};

pub const MAX_INPUT_BYTES: usize = teamarness_package_host::MAX_UI_PAYLOAD_BYTES;
pub const MAX_INVITATION_BYTES: usize = 8 * 1024;

pub fn read_private(path: &Path, limit: usize) -> Result<Vec<u8>, ControlError> {
    if path == Path::new("-") {
        return read_bounded(std::io::stdin().lock(), limit);
    }
    // O_NONBLOCK avoids waiting on a raced FIFO before fstat can refuse it.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| invalid())?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.len() > limit as u64
    {
        return Err(invalid());
    }
    read_bounded(file, limit)
}

pub fn read_bounded(reader: impl Read, limit: usize) -> Result<Vec<u8>, ControlError> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > limit {
        return Err(invalid());
    }
    Ok(bytes)
}

pub fn json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ControlError> {
    serde_json::from_slice(bytes).map_err(|_| invalid())
}

fn invalid() -> ControlError {
    ControlError::new(ErrorCode::InvalidInput)
}
