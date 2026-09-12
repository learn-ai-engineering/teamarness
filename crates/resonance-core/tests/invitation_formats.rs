use std::str::FromStr;

use resonance_core::{CoreRuntime, InvitationClaim, InvitationToken};

const TOKEN: &str = "01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4171c46882cafe2efbac629bd4f9aef6d7c17b7474f7bbacc3301ec5ed35faf962d39a32e90fef0f27cefc4dda0f3a29dc62b74b19800031e6d4b7985507fa0fb0c";
const CLAIM: &str = "0100c401020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4171c46882cafe2efbac629bd4f9aef6d7c17b7474f7bbacc3301ec5ed35faf962d39a32e90fef0f27cefc4dda0f3a29dc62b74b19800031e6d4b7985507fa0fb0c011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93caa432e395758fbacba6b468cba11e64b1da4448abf984d30440fb3bfd2d2d308f29a78c358f13b2fc81a384fdeeaba679f3d783e4ab7df904c6fa28817bc9700a";
const ADMIT: &str = "000001aa01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c000000000000044c01011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca0101280100c401020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4171c46882cafe2efbac629bd4f9aef6d7c17b7474f7bbacc3301ec5ed35faf962d39a32e90fef0f27cefc4dda0f3a29dc62b74b19800031e6d4b7985507fa0fb0c011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93caa432e395758fbacba6b468cba11e64b1da4448abf984d30440fb3bfd2d2d308f29a78c358f13b2fc81a384fdeeaba679f3d783e4ab7df904c6fa28817bc9700a0001df840a304e772ef4cdfb23293d8adadde8d43567bde4b0190c3d954f696f0b0a58a301421e90dfa44ea1198b33b65c2cc35fa73a37d50f849dd1f29a489b7f3a4237d498f81392a3a4293e53a817506d9825fe4477576bc68f9340588c932a6d6e839d398042160a86779ae09b6c6ca1027e7647b081da58af8fe5b10af1ab04";

#[test]
fn golden_token_claim_and_admit_authenticate_without_a_workspace() {
    assert_eq!(InvitationToken::from_str(TOKEN).unwrap().encoded(), TOKEN);
    assert_eq!(InvitationClaim::from_str(CLAIM).unwrap().encoded(), CLAIM);
    assert_eq!(
        CoreRuntime::authenticate_membership_operation(&decode_hex(ADMIT))
            .unwrap()
            .to_string(),
        "58a301421e90dfa44ea1198b33b65c2cc35fa73a37d50f849dd1f29a489b7f3a"
    );
}

#[test]
fn malformed_tampered_and_changed_target_values_are_rejected() {
    assert!(InvitationToken::from_str("00").is_err());
    assert!(InvitationClaim::from_str("00").is_err());

    let mut token = TOKEN.to_owned();
    token.replace_range(140..142, "08");
    assert!(InvitationToken::from_str(&token).is_err());

    let mut claim = CLAIM.to_owned();
    // First byte of the invitee identity in the complete claim.
    claim.replace_range(400..402, "12");
    assert!(InvitationClaim::from_str(&claim).is_err());

    let mut oversized = TOKEN.to_owned();
    oversized.push_str("00");
    assert!(InvitationToken::from_str(&oversized).is_err());

    let mut admit = decode_hex(ADMIT);
    admit[100] ^= 1;
    assert!(CoreRuntime::authenticate_membership_operation(&admit).is_err());
}

fn decode_hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect()
}
