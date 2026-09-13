use std::str::FromStr;

use teamarness_core::{MembershipOperationId, PublicIdentity, PublicSignature, SigningContext};

const CANONICAL_UNSIGNED: &str = "01020505050505050505050505050505050501ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c000000000000002a00000210e5cf3d3c8a4f9f3468c8cc58eea84892a22fdadbc1acb22410190044c1d55317762fddd969a453925d65717ac3eea21320b66b54342fde15128d6caf21215f";

#[test]
fn membership_operation_id_is_the_blake3_digest_of_canonical_bytes() {
    let canonical = decode_hex(CANONICAL_UNSIGNED);
    let id = MembershipOperationId::digest(&canonical);
    assert_eq!(
        id.to_string(),
        "3aa8a2aad82029a0ed923a103cfe2512a694d3fb2596bfdd0b2653dca120bc4e"
    );
}

#[test]
fn golden_operation_signature_is_bound_to_the_membership_domain_and_bytes() {
    let author = PublicIdentity::from_str(
        "ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c",
    )
    .unwrap();
    let signature = PublicSignature::from_str(
        "b427746d74a3f2543710173899733a358082ba89c2366a4ccb6d52382782a0e4e521c20399c77505f95b7b5df70614f4ea1e3c6a7b87ef3a357c5398e79f0b00",
    )
    .unwrap();
    let membership = SigningContext::new("teamarness.membership-operation/v1").unwrap();
    let invitation = SigningContext::new("teamarness.invitation/v1").unwrap();
    let canonical = decode_hex(CANONICAL_UNSIGNED);

    assert!(author.verify(&membership, &canonical, &signature).is_ok());
    assert!(author.verify(&invitation, &canonical, &signature).is_err());
    let mut tampered = canonical;
    tampered[0] ^= 1;
    assert!(author.verify(&membership, &tampered, &signature).is_err());
}

fn decode_hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect()
}
