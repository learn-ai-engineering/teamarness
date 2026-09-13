use crate::error::OnboardingError;
use crate::invitation::{VerifiedInvitationClaim, decode_claim};
use crate::membership::MembershipEnvelope;
use p2panda_core::Hash;

use crate::{MembershipOperationId, MembershipVersion, PublicIdentity, WorkspaceId};

const VERSION: u8 = 1;
const SUCCESS_TAG: u8 = 0;
const ERROR_TAG: u8 = 1;
pub(super) const MAX_REQUEST_BODY_BYTES: usize = 1024;
pub(super) const MAX_ERROR_BODY_BYTES: usize = 1024;
pub(crate) const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_OPERATIONS: usize = 1024;
pub(super) const MAX_ENVELOPE_BYTES: usize = 65_636;
pub(super) const MAX_DEPENDENCIES: usize = 64;
pub(super) const MAX_WORKSPACE_NAME_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodecError {
    UnsupportedVersion,
    Malformed,
    Limit,
    InvalidValue,
}

#[derive(Clone)]
pub(super) struct JoinRequest {
    pub(super) claim: VerifiedInvitationClaim,
}

#[derive(Clone)]
pub(crate) struct SuccessResponse {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) creator: PublicIdentity,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) name: String,
    pub(crate) genesis_id: MembershipOperationId,
    pub(crate) genesis: MembershipEnvelope,
    pub(crate) post_genesis: Vec<MembershipEnvelope>,
    pub(crate) admission_id: MembershipOperationId,
    pub(crate) membership_version: MembershipVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ErrorResponse {
    pub(super) error: OnboardingError,
    pub(super) retryable: bool,
}

pub(super) fn encode_request(request: &JoinRequest) -> Result<Vec<u8>, CodecError> {
    let claim_len = u16::try_from(request.claim.bytes.len()).map_err(|_| CodecError::Limit)?;
    let mut body = Vec::with_capacity(3 + request.claim.bytes.len());
    body.push(VERSION);
    body.extend_from_slice(&claim_len.to_be_bytes());
    body.extend_from_slice(&request.claim.bytes);
    encode_frame(&body, MAX_REQUEST_BODY_BYTES)
}

pub(super) fn decode_request(frame: &[u8]) -> Result<JoinRequest, CodecError> {
    let body = decode_frame(frame, MAX_REQUEST_BODY_BYTES)?;
    let mut cursor = Cursor::new(body);
    require_version(&mut cursor)?;
    let claim_len = usize::from(cursor.read_u16()?);
    let claim = decode_claim(cursor.read(claim_len)?).map_err(|_| CodecError::InvalidValue)?;
    cursor.finish()?;
    Ok(JoinRequest { claim })
}

pub(crate) fn encode_success(response: &SuccessResponse) -> Result<Vec<u8>, CodecError> {
    encode_success_with_limits(response, MAX_RESPONSE_BODY_BYTES, MAX_OPERATIONS)
}

pub(crate) fn encode_success_with_limits(
    response: &SuccessResponse,
    max_body_bytes: usize,
    max_operations: usize,
) -> Result<Vec<u8>, CodecError> {
    validate_success(response, max_operations)?;
    let name_len = u16::try_from(response.name.len()).map_err(|_| CodecError::Limit)?;
    let post_count = u16::try_from(response.post_genesis.len()).map_err(|_| CodecError::Limit)?;
    let mut body = Vec::new();
    body.extend_from_slice(&[VERSION, SUCCESS_TAG]);
    body.extend_from_slice(response.workspace_id.as_bytes());
    body.extend_from_slice(response.creator.as_bytes());
    body.extend_from_slice(&response.created_at_unix_ms.to_be_bytes());
    body.extend_from_slice(&name_len.to_be_bytes());
    body.extend_from_slice(response.name.as_bytes());
    body.extend_from_slice(response.genesis_id.as_bytes());
    write_envelope(&mut body, &response.genesis)?;
    body.extend_from_slice(&post_count.to_be_bytes());
    for envelope in &response.post_genesis {
        write_envelope(&mut body, envelope)?;
    }
    body.extend_from_slice(response.admission_id.as_bytes());
    body.extend_from_slice(response.membership_version.as_bytes());
    encode_frame(&body, max_body_bytes)
}

pub(crate) fn decode_success(frame: &[u8]) -> Result<SuccessResponse, CodecError> {
    let body = decode_frame(frame, MAX_RESPONSE_BODY_BYTES)?;
    let mut cursor = Cursor::new(body);
    require_version(&mut cursor)?;
    if cursor.read_u8()? != SUCCESS_TAG {
        return Err(CodecError::Malformed);
    }
    let workspace_id = WorkspaceId::from_bytes(cursor.read_array()?);
    let creator =
        PublicIdentity::from_bytes(cursor.read_array()?).map_err(|_| CodecError::InvalidValue)?;
    let created_at_unix_ms = i64::from_be_bytes(cursor.read_array()?);
    let name_len = usize::from(cursor.read_u16()?);
    if name_len > MAX_WORKSPACE_NAME_BYTES {
        return Err(CodecError::Limit);
    }
    let name = std::str::from_utf8(cursor.read(name_len)?)
        .map_err(|_| CodecError::InvalidValue)?
        .to_owned();
    validate_workspace_name(&name)?;
    let genesis_id = MembershipOperationId::from_hash(Hash::from_bytes(cursor.read_array()?));
    let genesis = read_envelope(&mut cursor)?;
    let post_count = usize::from(cursor.read_u16()?);
    if post_count.checked_add(1).ok_or(CodecError::Limit)? > MAX_OPERATIONS {
        return Err(CodecError::Limit);
    }
    let mut post_genesis = Vec::with_capacity(post_count);
    for _ in 0..post_count {
        post_genesis.push(read_envelope(&mut cursor)?);
    }
    let admission_id = MembershipOperationId::from_hash(Hash::from_bytes(cursor.read_array()?));
    let membership_version = MembershipVersion::from_bytes(cursor.read_array()?);
    cursor.finish()?;

    let response = SuccessResponse {
        workspace_id,
        creator,
        created_at_unix_ms,
        name,
        genesis_id,
        genesis,
        post_genesis,
        admission_id,
        membership_version,
    };
    validate_success(&response, MAX_OPERATIONS)?;
    Ok(response)
}

pub(super) fn encode_error(error: OnboardingError) -> Result<Vec<u8>, CodecError> {
    let code = RemoteErrorCode::from_error(error);
    encode_frame(
        &[
            VERSION,
            ERROR_TAG,
            code as u8,
            u8::from(error.is_retryable()),
        ],
        MAX_ERROR_BODY_BYTES,
    )
}

pub(super) fn decode_error(frame: &[u8]) -> Result<ErrorResponse, CodecError> {
    let body = decode_frame(frame, MAX_ERROR_BODY_BYTES)?;
    if body.len() != 4 {
        return Err(CodecError::Malformed);
    }
    if body[0] != VERSION {
        return Err(CodecError::UnsupportedVersion);
    }
    if body[1] != ERROR_TAG {
        return Err(CodecError::Malformed);
    }
    let code = RemoteErrorCode::try_from(body[2])?;
    let retryable = match body[3] {
        0 => false,
        1 => true,
        _ => return Err(CodecError::Malformed),
    };
    let error = code.error();
    if retryable != error.is_retryable() {
        return Err(CodecError::InvalidValue);
    }
    Ok(ErrorResponse { error, retryable })
}

fn validate_success(response: &SuccessResponse, max_operations: usize) -> Result<(), CodecError> {
    validate_workspace_name(&response.name)?;
    if response.name.len() > MAX_WORKSPACE_NAME_BYTES
        || response
            .post_genesis
            .len()
            .checked_add(1)
            .ok_or(CodecError::Limit)?
            > max_operations
    {
        return Err(CodecError::Limit);
    }
    if response.genesis.canonical_bytes().len() > MAX_ENVELOPE_BYTES
        || response.genesis.dependencies.len() > MAX_DEPENDENCIES
        || response.post_genesis.iter().any(|envelope| {
            envelope.canonical_bytes().len() > MAX_ENVELOPE_BYTES
                || envelope.dependencies.len() > MAX_DEPENDENCIES
        })
    {
        return Err(CodecError::Limit);
    }
    Ok(())
}

fn validate_workspace_name(name: &str) -> Result<(), CodecError> {
    crate::workspace::validate_name(name).map_err(|_| CodecError::InvalidValue)
}

fn write_envelope(body: &mut Vec<u8>, envelope: &MembershipEnvelope) -> Result<(), CodecError> {
    let bytes = envelope.canonical_bytes();
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(CodecError::Limit);
    }
    let length = u32::try_from(bytes.len()).map_err(|_| CodecError::Limit)?;
    body.extend_from_slice(&length.to_be_bytes());
    body.extend_from_slice(bytes);
    Ok(())
}

fn read_envelope(cursor: &mut Cursor<'_>) -> Result<MembershipEnvelope, CodecError> {
    let length = usize::try_from(cursor.read_u32()?).map_err(|_| CodecError::Limit)?;
    if length > MAX_ENVELOPE_BYTES {
        return Err(CodecError::Limit);
    }
    let envelope =
        MembershipEnvelope::decode(cursor.read(length)?).map_err(|_| CodecError::InvalidValue)?;
    if envelope.dependencies.len() > MAX_DEPENDENCIES {
        return Err(CodecError::Limit);
    }
    Ok(envelope)
}

pub(super) fn encode_frame(body: &[u8], limit: usize) -> Result<Vec<u8>, CodecError> {
    if body.len() > limit {
        return Err(CodecError::Limit);
    }
    let length = u32::try_from(body.len()).map_err(|_| CodecError::Limit)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(body);
    Ok(frame)
}

pub(super) fn decode_frame(frame: &[u8], limit: usize) -> Result<&[u8], CodecError> {
    let length_bytes = frame.get(..4).ok_or(CodecError::Malformed)?;
    let length = usize::try_from(u32::from_be_bytes(copy_array(length_bytes)))
        .map_err(|_| CodecError::Limit)?;
    if length > limit {
        return Err(CodecError::Limit);
    }
    let body = frame.get(4..).ok_or(CodecError::Malformed)?;
    if body.len() != length {
        return Err(CodecError::Malformed);
    }
    Ok(body)
}

fn require_version(cursor: &mut Cursor<'_>) -> Result<(), CodecError> {
    if cursor.read_u8()? != VERSION {
        return Err(CodecError::UnsupportedVersion);
    }
    Ok(())
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteErrorCode {
    InvalidInput = 1,
    Authentication = 2,
    Expired = 3,
    AlreadyUsed = 4,
    Readiness = 5,
    Timeout = 6,
    Overload = 7,
    InvalidBootstrap = 8,
    LocalConflict = 9,
    AdmissionNoLongerCurrent = 10,
    UncertainStorageOutcome = 11,
    Shutdown = 12,
}

impl RemoteErrorCode {
    const ALL: [Self; 12] = [
        Self::InvalidInput,
        Self::Authentication,
        Self::Expired,
        Self::AlreadyUsed,
        Self::Readiness,
        Self::Timeout,
        Self::Overload,
        Self::InvalidBootstrap,
        Self::LocalConflict,
        Self::AdmissionNoLongerCurrent,
        Self::UncertainStorageOutcome,
        Self::Shutdown,
    ];

    const fn from_error(error: OnboardingError) -> Self {
        match error {
            OnboardingError::InvalidInput => Self::InvalidInput,
            OnboardingError::Authentication => Self::Authentication,
            OnboardingError::Expired => Self::Expired,
            OnboardingError::AlreadyUsed => Self::AlreadyUsed,
            OnboardingError::Readiness => Self::Readiness,
            OnboardingError::Timeout => Self::Timeout,
            OnboardingError::Overload => Self::Overload,
            OnboardingError::InvalidBootstrap => Self::InvalidBootstrap,
            OnboardingError::LocalConflict => Self::LocalConflict,
            OnboardingError::AdmissionNoLongerCurrent => Self::AdmissionNoLongerCurrent,
            OnboardingError::UncertainStorageOutcome => Self::UncertainStorageOutcome,
            OnboardingError::Shutdown => Self::Shutdown,
        }
    }

    const fn error(self) -> OnboardingError {
        match self {
            Self::InvalidInput => OnboardingError::InvalidInput,
            Self::Authentication => OnboardingError::Authentication,
            Self::Expired => OnboardingError::Expired,
            Self::AlreadyUsed => OnboardingError::AlreadyUsed,
            Self::Readiness => OnboardingError::Readiness,
            Self::Timeout => OnboardingError::Timeout,
            Self::Overload => OnboardingError::Overload,
            Self::InvalidBootstrap => OnboardingError::InvalidBootstrap,
            Self::LocalConflict => OnboardingError::LocalConflict,
            Self::AdmissionNoLongerCurrent => OnboardingError::AdmissionNoLongerCurrent,
            Self::UncertainStorageOutcome => OnboardingError::UncertainStorageOutcome,
            Self::Shutdown => OnboardingError::Shutdown,
        }
    }
}

impl TryFrom<u8> for RemoteErrorCode {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::ALL
            .into_iter()
            .find(|code| *code as u8 == value)
            .ok_or(CodecError::InvalidValue)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read(&mut self, length: usize) -> Result<&'a [u8], CodecError> {
        let end = self.offset.checked_add(length).ok_or(CodecError::Limit)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(CodecError::Malformed)?;
        self.offset = end;
        Ok(bytes)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        Ok(copy_array(self.read(N)?))
    }

    fn read_u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.read(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_be_bytes(self.read_array()?))
    }

    fn read_u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_be_bytes(self.read_array()?))
    }

    fn finish(self) -> Result<(), CodecError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(CodecError::Malformed)
        }
    }
}

fn copy_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    bytes.try_into().expect("validated wire slice width")
}
