use std::str::FromStr;

use crate::CoreRuntime;
use crate::invitation::{InvitationClaim, InvitationToken};

const TOKEN: &str = "01020303030303030303030303030303030301ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c00000000000003e800000000000007d0090909090909090909090909090909090909090909090909090909090909090901390172f8e526626476553d760aff5765f0982aad65c66ce82004fca24383d4173a2a0c0b6820a6ed6d42a020e06b98bb0552457070a469b02ae1479f24913dbbf3f4417accb9e9276549657229fb434f0c06ffab49a80b059d9ed74913ee6d03";
fn signed_claim() -> crate::invitation::VerifiedInvitationClaim {
    let token = crate::invitation::sign_token(
        &iroh_base::SecretKey::from_bytes(&[7; 32]),
        crate::WorkspaceId::from_bytes([3; 16]),
        1_000,
        2_000,
        [9; 32],
    )
    .unwrap();
    crate::invitation::sign_claim(
        &iroh_base::SecretKey::from_bytes(&[8; 32]),
        &token,
        crate::MembershipOperationId::digest(b"genesis"),
    )
    .unwrap()
}

#[test]
fn golden_token_claim_and_admit_authenticate_without_a_workspace() {
    assert_eq!(InvitationToken::from_str(TOKEN).unwrap().encoded(), TOKEN);
    let claim = signed_claim();
    let encoded = InvitationClaim::from_verified(&claim);
    assert_eq!(
        InvitationClaim::from_str(encoded.encoded())
            .unwrap()
            .verified()
            .unwrap()
            .create_person
            .id,
        claim.create_person.id
    );
    let admission = crate::membership::MembershipEnvelope::sign(
        &iroh_base::SecretKey::from_bytes(&[7; 32]),
        claim.token.workspace_id,
        1_100,
        crate::membership::MembershipAction::Admit {
            target: claim.invitee,
            proof: Box::new(crate::membership::AdmissionProof {
                claim: claim.clone(),
            }),
        },
        vec![claim.create_person.id],
    )
    .unwrap();
    assert_eq!(
        CoreRuntime::authenticate_membership_operation(admission.canonical_bytes()).unwrap(),
        admission.id,
    );
}

#[test]
fn malformed_tampered_and_changed_target_values_are_rejected() {
    assert!(InvitationToken::from_str("00").is_err());
    assert!(InvitationClaim::from_str("00").is_err());

    let mut token = TOKEN.to_owned();
    token.replace_range(140..142, "08");
    assert!(InvitationToken::from_str(&token).is_err());

    let mut claim = signed_claim().bytes;
    // The claim signature cannot authenticate a changed invitee.
    claim[3 + 196 + 1] ^= 1;
    assert!(crate::invitation::decode_claim(&claim).is_err());

    let mut oversized = TOKEN.to_owned();
    oversized.push_str("00");
    assert!(InvitationToken::from_str(&oversized).is_err());

    let signed = signed_claim();
    let mut admit = crate::membership::MembershipEnvelope::sign(
        &iroh_base::SecretKey::from_bytes(&[7; 32]),
        signed.token.workspace_id,
        1_100,
        crate::membership::MembershipAction::Admit {
            target: signed.invitee,
            proof: Box::new(crate::membership::AdmissionProof {
                claim: signed.clone(),
            }),
        },
        vec![signed.create_person.id],
    )
    .unwrap()
    .canonical_bytes()
    .to_vec();
    admit[100] ^= 1;
    assert!(CoreRuntime::authenticate_membership_operation(&admit).is_err());
}
