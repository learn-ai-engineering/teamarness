use iroh_base::{PublicKey, SecretKey, Signature};

use crate::error::IdentityError;

pub(crate) const DEFAULT_DISPLAY_NAME: &str = "This device";
pub(crate) const MAX_DISPLAY_NAME_CHARS: usize = 64;
pub(crate) const MAX_SIGNING_CONTEXT_BYTES: usize = 64;
const SIGNING_ENVELOPE_DOMAIN: &[u8] = b"teamarness.signature/v1";

pub(crate) fn validate_display_name(display_name: &str) -> Result<(), IdentityError> {
    let character_count = display_name.chars().count();
    let has_valid_characters = display_name.chars().all(|character| {
        character.is_alphanumeric() || matches!(character, ' ' | '-' | '_' | '\'' | '.')
    });

    if character_count == 0
        || character_count > MAX_DISPLAY_NAME_CHARS
        || display_name.trim() != display_name
        || !has_valid_characters
    {
        return Err(IdentityError::InvalidDisplayName);
    }

    Ok(())
}

pub(crate) fn validate_signing_context(context: &str) -> Result<(), IdentityError> {
    let valid = !context.is_empty()
        && context.len() <= MAX_SIGNING_CONTEXT_BYTES
        && context.is_ascii()
        && context.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._/-".contains(&byte)
        })
        && context.as_bytes()[0].is_ascii_lowercase();

    if !valid {
        return Err(IdentityError::InvalidSigningContext);
    }

    Ok(())
}

fn signing_message(context: &str, payload: &[u8]) -> Result<Vec<u8>, IdentityError> {
    let context_length =
        u16::try_from(context.len()).map_err(|_| IdentityError::PayloadTooLarge)?;
    let payload_length =
        u64::try_from(payload.len()).map_err(|_| IdentityError::PayloadTooLarge)?;
    let mut message =
        Vec::with_capacity(SIGNING_ENVELOPE_DOMAIN.len() + 2 + context.len() + 8 + payload.len());
    message.extend_from_slice(SIGNING_ENVELOPE_DOMAIN);
    message.extend_from_slice(&context_length.to_be_bytes());
    message.extend_from_slice(context.as_bytes());
    message.extend_from_slice(&payload_length.to_be_bytes());
    message.extend_from_slice(payload);
    Ok(message)
}

pub(crate) fn sign(
    secret_key: &SecretKey,
    context: &str,
    payload: &[u8],
) -> Result<Signature, IdentityError> {
    let message = signing_message(context, payload)?;
    Ok(secret_key.sign(&message))
}

pub(crate) fn verify(
    public_key: PublicKey,
    context: &str,
    payload: &[u8],
    signature: &Signature,
) -> Result<(), IdentityError> {
    let message = signing_message(context, payload)?;
    public_key
        .verify(&message, signature)
        .map_err(|_| IdentityError::InvalidSignature)
}
