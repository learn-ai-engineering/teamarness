use crate::error::MembershipError;

use super::MembershipEnvelope;

/// Performs only version-1 structural and cryptographic checks. Workspace
/// anchoring, dependency lookup, timestamps, authorization, and CRDT processing
/// intentionally belong to later phases.
pub(crate) fn authenticate(bytes: &[u8]) -> Result<MembershipEnvelope, MembershipError> {
    MembershipEnvelope::decode(bytes)
}
