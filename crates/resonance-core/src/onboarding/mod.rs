pub(crate) mod client;
pub(crate) mod handler;
mod transfer;
pub(crate) mod transport;
pub(crate) mod wire;

pub use transfer::WorkspaceInvitation;

#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;

    use super::wire::{
        CodecError, ErrorResponse, JoinRequest, MAX_DEPENDENCIES, MAX_ENVELOPE_BYTES,
        MAX_ERROR_BODY_BYTES, MAX_OPERATIONS, MAX_REQUEST_BODY_BYTES, MAX_RESPONSE_BODY_BYTES,
        MAX_WORKSPACE_NAME_BYTES, SuccessResponse, decode_error, decode_frame, decode_request,
        decode_success, encode_error, encode_frame, encode_request, encode_success,
    };
    use crate::OnboardingError;
    use crate::invitation::{sign_claim, sign_token};
    use crate::membership::{MembershipAction, MembershipEnvelope};
    use crate::{MembershipVersion, WorkspaceId};

    mod admission_recovery;
    mod client;
    mod import;

    const CLAIM: &str = "0100c401020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4171c46882cafe2efbac629bd4f9aef6d7c17b7474f7bbacc3301ec5ed35faf962d39a32e90fef0f27cefc4dda0f3a29dc62b74b19800031e6d4b7985507fa0fb0c011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93caa432e395758fbacba6b468cba11e64b1da4448abf984d30440fb3bfd2d2d308f29a78c358f13b2fc81a384fdeeaba679f3d783e4ab7df904c6fa28817bc9700a";

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn request() -> JoinRequest {
        let token = sign_token(
            &secret(7),
            WorkspaceId::from_bytes([3; 16]),
            1_000,
            2_000,
            [9; 32],
        )
        .unwrap();
        JoinRequest {
            claim: sign_claim(&secret(8), &token).unwrap(),
        }
    }

    fn success(name: String) -> SuccessResponse {
        let workspace_id = WorkspaceId::from_bytes([3; 16]);
        let genesis = MembershipEnvelope::sign(
            &secret(7),
            workspace_id,
            1_000,
            MembershipAction::Create,
            vec![],
        )
        .unwrap();
        SuccessResponse {
            workspace_id,
            creator: genesis.author,
            created_at_unix_ms: 1_000,
            name,
            genesis_id: genesis.id,
            admission_id: genesis.id,
            membership_version: MembershipVersion::from_bytes([5; 32]),
            genesis,
            post_genesis: vec![],
        }
    }

    #[test]
    fn request_golden_vector_round_trips_with_a_big_endian_frame() {
        let frame = encode_request(&request()).unwrap();
        assert_eq!(hex(&frame), format!("0000012b010128{CLAIM}"));
        let decoded = decode_request(&frame).unwrap();
        assert_eq!(decoded.claim.bytes, request().claim.bytes);
    }

    #[test]
    fn success_golden_vector_round_trips_with_a_big_endian_frame() {
        let response = success("A".to_owned());
        let frame = encode_success(&response).unwrap();
        assert_eq!(
            hex(&frame),
            "00000145010003030303030303030303030303030303ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e8000141fd33eaf60d90217f57d550c96689bd91741ac235c0fbd3ef4e14339f7701e5ca000000a20000003e01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e8000000fd33eaf60d90217f57d550c96689bd91741ac235c0fbd3ef4e14339f7701e5ca78b74d324884a90e0a471b7ad82074a9d14ec4d2c696c37dd0dd27ad46adb3cf8adef4536ea3b15ddfd99f2c9517e1943620da28d88823e1c679831ace5256040000fd33eaf60d90217f57d550c96689bd91741ac235c0fbd3ef4e14339f7701e5ca0505050505050505050505050505050505050505050505050505050505050505"
        );
        let decoded = decode_success(&frame).unwrap();
        assert_eq!(decoded.workspace_id, response.workspace_id);
        assert_eq!(
            decoded.genesis.canonical_bytes(),
            response.genesis.canonical_bytes()
        );
        assert_eq!(decoded.membership_version, response.membership_version);
    }

    #[test]
    fn every_stable_remote_error_has_a_golden_code_and_retryability() {
        let cases = [
            (OnboardingError::InvalidInput, 1, false),
            (OnboardingError::Authentication, 2, false),
            (OnboardingError::Expired, 3, false),
            (OnboardingError::AlreadyUsed, 4, false),
            (OnboardingError::Readiness, 5, true),
            (OnboardingError::Timeout, 6, true),
            (OnboardingError::Overload, 7, true),
            (OnboardingError::InvalidBootstrap, 8, false),
            (OnboardingError::LocalConflict, 9, false),
            (OnboardingError::AdmissionNoLongerCurrent, 10, false),
            (OnboardingError::UncertainStorageOutcome, 11, false),
            (OnboardingError::Shutdown, 12, true),
        ];
        for (error, code, retryable) in cases {
            let frame = encode_error(error).unwrap();
            assert_eq!(frame, vec![0, 0, 0, 4, 1, 1, code, u8::from(retryable)]);
            assert_eq!(
                decode_error(&frame).unwrap(),
                ErrorResponse { error, retryable }
            );
        }
    }

    #[test]
    fn every_declared_frame_body_limit_accepts_the_boundary_and_rejects_the_next_byte() {
        for limit in [
            MAX_REQUEST_BODY_BYTES,
            MAX_ERROR_BODY_BYTES,
            MAX_RESPONSE_BODY_BYTES,
        ] {
            let body = vec![0; limit];
            let frame = encode_frame(&body, limit).unwrap();
            assert_eq!(decode_frame(&frame, limit).unwrap().len(), limit);
            assert_eq!(encode_frame(&[0], 0), Err(CodecError::Limit));
            assert_eq!(
                decode_frame(&length_only(limit + 1), limit),
                Err(CodecError::Limit)
            );
        }
    }

    #[test]
    fn frames_reject_unsupported_versions_lengths_trailing_bytes_and_retry_mismatch() {
        let mut request = encode_request(&request()).unwrap();
        request[4] = 2;
        assert!(matches!(
            decode_request(&request),
            Err(CodecError::UnsupportedVersion)
        ));
        request[4] = 1;
        request.push(0);
        assert!(matches!(
            decode_request(&request),
            Err(CodecError::Malformed)
        ));

        let mut success = encode_success(&success("A".to_owned())).unwrap();
        success[4] = 2;
        assert!(matches!(
            decode_success(&success),
            Err(CodecError::UnsupportedVersion)
        ));
        success[4] = 1;
        success.push(0);
        assert!(matches!(
            decode_success(&success),
            Err(CodecError::Malformed)
        ));

        let mut error = encode_error(OnboardingError::Timeout).unwrap();
        error[7] = 0;
        assert_eq!(decode_error(&error), Err(CodecError::InvalidValue));
        error[7] = 2;
        assert_eq!(decode_error(&error), Err(CodecError::Malformed));
        error[4] = 2;
        assert_eq!(decode_error(&error), Err(CodecError::UnsupportedVersion));
        error[4] = 1;
        error[6] = 0xff;
        assert_eq!(decode_error(&error), Err(CodecError::InvalidValue));

        assert!(matches!(
            decode_request(&length_only(MAX_REQUEST_BODY_BYTES + 1)),
            Err(CodecError::Limit)
        ));
        assert_eq!(
            decode_error(&length_only(MAX_ERROR_BODY_BYTES + 1)),
            Err(CodecError::Limit)
        );
        assert!(matches!(
            decode_success(&length_only(MAX_RESPONSE_BODY_BYTES + 1)),
            Err(CodecError::Limit)
        ));
    }

    #[test]
    fn success_enforces_operation_envelope_dependency_and_name_bounds() {
        let mut at_operation_limit = success("A".to_owned());
        at_operation_limit.post_genesis =
            vec![at_operation_limit.genesis.clone(); MAX_OPERATIONS - 1];
        assert!(encode_success(&at_operation_limit).is_ok());
        at_operation_limit
            .post_genesis
            .push(at_operation_limit.genesis.clone());
        assert_eq!(encode_success(&at_operation_limit), Err(CodecError::Limit));

        let max_name = "𐐀".repeat(64);
        assert_eq!(max_name.len(), MAX_WORKSPACE_NAME_BYTES);
        assert!(encode_success(&success(max_name)).is_ok());
        assert_eq!(
            encode_success(&success(format!("{}a", "𐐀".repeat(64)))),
            Err(CodecError::InvalidValue)
        );

        let dependencies = (0..MAX_DEPENDENCIES)
            .map(|index| crate::MembershipOperationId::digest(index.to_be_bytes()))
            .collect();
        assert!(
            MembershipEnvelope::sign(
                &secret(7),
                WorkspaceId::from_bytes([3; 16]),
                1_001,
                MembershipAction::Create,
                dependencies,
            )
            .is_ok()
        );
        let too_many_dependencies = (0..=MAX_DEPENDENCIES)
            .map(|index| crate::MembershipOperationId::digest(index.to_be_bytes()))
            .collect();
        assert!(
            MembershipEnvelope::sign(
                &secret(7),
                WorkspaceId::from_bytes([3; 16]),
                1_001,
                MembershipAction::Create,
                too_many_dependencies,
            )
            .is_err()
        );

        let mut frame = encode_success(&success("A".to_owned())).unwrap();
        let genesis_length_offset = 4 + 2 + 16 + 32 + 8 + 2 + 1 + 32;
        let genesis_length = usize::try_from(u32::from_be_bytes(
            frame[genesis_length_offset..genesis_length_offset + 4]
                .try_into()
                .unwrap(),
        ))
        .unwrap();
        let operation_count_offset = genesis_length_offset + 4 + genesis_length;
        frame[operation_count_offset..operation_count_offset + 2]
            .copy_from_slice(&u16::try_from(MAX_OPERATIONS).unwrap().to_be_bytes());
        assert!(matches!(decode_success(&frame), Err(CodecError::Limit)));

        let mut frame = encode_success(&success("A".to_owned())).unwrap();
        frame[genesis_length_offset..genesis_length_offset + 4]
            .copy_from_slice(&u32::try_from(MAX_ENVELOPE_BYTES + 1).unwrap().to_be_bytes());
        assert!(matches!(decode_success(&frame), Err(CodecError::Limit)));
    }

    fn length_only(length: usize) -> Vec<u8> {
        u32::try_from(length).unwrap().to_be_bytes().to_vec()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
