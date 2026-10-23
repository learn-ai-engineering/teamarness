use std::fmt;
use std::str::FromStr;

use iroh_base::EndpointAddr;
use iroh_tickets::endpoint::EndpointTicket;

use crate::MembershipOperationId;
use crate::error::ValueParseError;
use crate::invitation::{InvitationToken, VerifiedInvitationToken};

const PREFIX: &str = "teamarness-workspace-v1.";
/// Payload version 2 adds the Workspace genesis identifier after the token, so
/// the joiner can anchor its own `create person` operation before it connects.
const VERSION: u8 = 2;
const MAX_TEXT_BYTES: usize = 8 * 1024;
const MAX_PAYLOAD_BYTES: usize = 4 * 1024;
const MAX_TICKET_BYTES: usize = 3 * 1024;
const MAX_ADDRESSES: usize = 8;

/// An opaque, bounded Workspace onboarding value suitable for copy and paste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInvitation(String);

#[derive(Clone)]
pub(crate) struct VerifiedWorkspaceInvitation {
    pub(crate) token: VerifiedInvitationToken,
    /// Unauthenticated until the bootstrap arrives; a wrong value only makes
    /// the issuer refuse the claim's group create.
    pub(crate) genesis_id: MembershipOperationId,
    pub(crate) endpoint_addr: EndpointAddr,
}

impl WorkspaceInvitation {
    /// Returns the canonical transfer text.
    #[must_use]
    pub fn encoded(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_parts(
        token: &VerifiedInvitationToken,
        genesis_id: MembershipOperationId,
        endpoint_addr: EndpointAddr,
    ) -> Result<Self, ValueParseError> {
        let ticket = EndpointTicket::new(endpoint_addr).to_string();
        let payload = encode_payload(token, genesis_id, &ticket)?;
        let value = format!("{PREFIX}{}", encode_hex(&payload));
        Self::from_str(&value)
    }

    pub(crate) fn verified(&self) -> Result<VerifiedWorkspaceInvitation, ValueParseError> {
        decode_transfer(&self.0)
    }
}

impl fmt::Display for WorkspaceInvitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for WorkspaceInvitation {
    type Err = ValueParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        decode_transfer(value)?;
        Ok(Self(value.to_owned()))
    }
}

fn encode_payload(
    token: &VerifiedInvitationToken,
    genesis_id: MembershipOperationId,
    ticket: &str,
) -> Result<Vec<u8>, ValueParseError> {
    let token_len = u16::try_from(token.bytes.len()).map_err(|_| invalid())?;
    let ticket_len = u16::try_from(ticket.len()).map_err(|_| invalid())?;
    let mut payload = Vec::with_capacity(1 + 2 + token.bytes.len() + 32 + 2 + ticket.len());
    payload.push(VERSION);
    payload.extend_from_slice(&token_len.to_be_bytes());
    payload.extend_from_slice(&token.bytes);
    payload.extend_from_slice(genesis_id.as_bytes());
    payload.extend_from_slice(&ticket_len.to_be_bytes());
    payload.extend_from_slice(ticket.as_bytes());
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(invalid());
    }
    Ok(payload)
}

fn decode_transfer(value: &str) -> Result<VerifiedWorkspaceInvitation, ValueParseError> {
    if value.len() > MAX_TEXT_BYTES || !value.is_ascii() {
        return Err(invalid());
    }
    let encoded = value.strip_prefix(PREFIX).ok_or_else(invalid)?;
    if encoded.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(invalid());
    }
    let payload = decode_hex(encoded)?;
    if payload.len() > MAX_PAYLOAD_BYTES || payload.first() != Some(&VERSION) {
        return Err(invalid());
    }

    let mut cursor = Cursor::new(&payload[1..]);
    let token_len = usize::from(cursor.read_u16()?);
    let token_bytes = cursor.read(token_len)?;
    let token = InvitationToken::from_str(&encode_hex(token_bytes))
        .map_err(|_| invalid())?
        .verified()
        .map_err(|_| invalid())?;
    let genesis_id = MembershipOperationId::from_hash(p2panda_core::Hash::from_bytes(copy_array(
        cursor.read(32)?,
    )));

    let ticket_len = usize::from(cursor.read_u16()?);
    if ticket_len > MAX_TICKET_BYTES {
        return Err(invalid());
    }
    let ticket_text = std::str::from_utf8(cursor.read(ticket_len)?).map_err(|_| invalid())?;
    if !cursor.is_empty() {
        return Err(invalid());
    }
    let ticket = EndpointTicket::from_str(ticket_text).map_err(|_| invalid())?;
    if ticket.to_string() != ticket_text {
        return Err(invalid());
    }
    let endpoint_addr = ticket.endpoint_addr();
    if endpoint_addr.id != token.inviter.as_public_key()
        || endpoint_addr.addrs.len() > MAX_ADDRESSES
        || endpoint_addr
            .addrs
            .iter()
            .any(|address| !address.is_ip() && !address.is_relay())
    {
        return Err(invalid());
    }

    Ok(VerifiedWorkspaceInvitation {
        token,
        genesis_id,
        endpoint_addr: endpoint_addr.clone(),
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read(&mut self, length: usize) -> Result<&'a [u8], ValueParseError> {
        let end = self.offset.checked_add(length).ok_or_else(invalid)?;
        let bytes = self.bytes.get(self.offset..end).ok_or_else(invalid)?;
        self.offset = end;
        Ok(bytes)
    }

    fn read_u16(&mut self) -> Result<u16, ValueParseError> {
        Ok(u16::from_be_bytes(copy_array(self.read(2)?)))
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn invalid() -> ValueParseError {
    ValueParseError::new("workspace invitation")
}

fn copy_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    bytes.try_into().expect("validated transfer slice width")
}

fn encode_hex(bytes: &[u8]) -> String {
    crate::encoding::hex_encode(bytes)
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ValueParseError> {
    crate::encoding::hex_decode(value).ok_or_else(invalid)
}
