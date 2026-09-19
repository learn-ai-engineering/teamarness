use std::str::FromStr;

use crate::CoreRuntime;
use crate::invitation::{InvitationClaim, InvitationToken};

const TOKEN: &str = "01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4173a2a0c0b6820a6ed6d42a020e06b98bb0552457070a469b02ae1479f24913dbbf3f4417accb9e9276549657229fb434f0c06ffab49a80b059d9ed74913ee6d03";
const CLAIM: &str = "0100c401020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4173a2a0c0b6820a6ed6d42a020e06b98bb0552457070a469b02ae1479f24913dbbf3f4417accb9e9276549657229fb434f0c06ffab49a80b059d9ed74913ee6d03011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca511c5aeb0f1f776e59dea0e74c30ee2d1eae9a0eb75c2167d6f7e37683103e2635e528ef81502b64ad857773766eef6de1282266792da0f519f26320f6461c06";
const ADMIT: &str = "000001aa01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c000000000000044c01011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca0101280100c401020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4173a2a0c0b6820a6ed6d42a020e06b98bb0552457070a469b02ae1479f24913dbbf3f4417accb9e9276549657229fb434f0c06ffab49a80b059d9ed74913ee6d03011398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca511c5aeb0f1f776e59dea0e74c30ee2d1eae9a0eb75c2167d6f7e37683103e2635e528ef81502b64ad857773766eef6de1282266792da0f519f26320f6461c060001df840a304e772ef4cdfb23293d8adadde8d43567bde4b0190c3d954f696f0b0ae879c13df286a4dd389eebd703768c011b1c5d4d6b981d20625b5f77c37d0af5a06a47e50ca87f9ca88d69c978f7b0f435f47b7aa7b269762760d7573e17e9b2bfccfceba78d5cbae78c188addaa3531edfd60f3a8c37bedf4f1db9f42ffea03";

#[test]
fn golden_token_claim_and_admit_authenticate_without_a_workspace() {
    assert_eq!(InvitationToken::from_str(TOKEN).unwrap().encoded(), TOKEN);
    assert_eq!(InvitationClaim::from_str(CLAIM).unwrap().encoded(), CLAIM);
    assert_eq!(
        CoreRuntime::authenticate_membership_operation(&decode_hex(ADMIT))
            .unwrap()
            .to_string(),
        "e879c13df286a4dd389eebd703768c011b1c5d4d6b981d20625b5f77c37d0af5"
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
    crate::encoding::hex_decode(value).expect("hex fixture")
}
