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

/// Verifies: TEAM.SYS.MEM-R02, ID-R10. An `add agent key` proof binds the
/// Agent key to one Workspace, one Person group and one Steward. A proof made
/// for any other binding, or by another key, is an invalid proof; tampered
/// canonical bytes fail hash or signature authentication before any authority
/// is considered.
#[test]
fn agent_key_proof_forgery_is_rejected_before_authorization() {
    use teamarness_core::test_authoring::{
        TestAction, identity_of, operation_id, replay_for_tests, sign_admission,
        sign_agent_key_proof, sign_operation, try_sign_operation,
    };
    use teamarness_core::{MembershipError, WorkspaceId};

    const CREATOR: [u8; 32] = [1; 32];
    const MEMBER: [u8; 32] = [2; 32];
    const AGENT: [u8; 32] = [3; 32];
    const OTHER: [u8; 32] = [4; 32];

    let workspace = WorkspaceId::from_str("07070707070707070707070707070707").unwrap();
    let genesis = sign_operation(&CREATOR, workspace, 10, TestAction::Create, &[]);
    let genesis_id = operation_id(&genesis);
    let creator_group = sign_operation(
        &CREATOR,
        workspace,
        10,
        TestAction::CreatePerson,
        &[genesis_id],
    );
    let attach = sign_operation(
        &CREATOR,
        workspace,
        10,
        TestAction::AttachPerson {
            group: operation_id(&creator_group),
        },
        &[operation_id(&creator_group)],
    );
    let (member_group, admit) = sign_admission(
        &CREATOR,
        &MEMBER,
        workspace,
        genesis_id,
        20,
        1_000,
        [1; 32],
        30,
        &[operation_id(&attach)],
    );
    let group = operation_id(&member_group);
    let head = operation_id(&admit);
    let base = vec![genesis, creator_group, attach, member_group, admit.clone()];
    let replay = |extra: Vec<u8>| {
        let mut all = base.clone();
        all.push(extra);
        replay_for_tests(workspace, identity_of(&CREATOR), &all).map(|_| ())
    };
    let steward = identity_of(&MEMBER);
    let agent = identity_of(&AGENT);

    // The genuine proof is accepted.
    let genuine = sign_operation(
        &MEMBER,
        workspace,
        40,
        TestAction::AddAgentKey {
            group,
            agent_secret: AGENT,
        },
        &[head],
    );
    assert_eq!(replay(genuine.clone()), Ok(()));

    let forged = [
        (
            "signed by the wrong key",
            sign_agent_key_proof(&OTHER, workspace, group, steward),
        ),
        (
            "over the wrong workspace",
            sign_agent_key_proof(
                &AGENT,
                WorkspaceId::from_str("08080808080808080808080808080808").unwrap(),
                group,
                steward,
            ),
        ),
        (
            "over the wrong group",
            sign_agent_key_proof(&AGENT, workspace, genesis_id, steward),
        ),
        (
            "over the wrong steward",
            sign_agent_key_proof(&AGENT, workspace, group, identity_of(&CREATOR)),
        ),
    ];
    for (case, proof) in forged {
        // The envelope codec authenticates the proof while decoding, so a forged
        // proof never produces an envelope at all, let alone reaches authority.
        let outcome = try_sign_operation(
            &MEMBER,
            workspace,
            40,
            TestAction::AddAgentKeyWithProof {
                group,
                agent,
                proof,
            },
            &[head],
        );
        assert_eq!(
            outcome.map(|_| ()),
            Err(MembershipError::InvalidAdmissionProof),
            "a proof {case} is an invalid admission proof"
        );
    }

    // Tampered canonical bytes: the content hash and signature are checked
    // before any authority rule can run.
    let mut hash_tampered = genuine.clone();
    // Length prefix, header (version, workspace, author, timestamp), action tag
    // and group tag, then the first byte of the group identifier.
    let group_offset = 4 + (1 + 1 + 16 + 1 + 32 + 8) + 1 + 1;
    hash_tampered[group_offset] ^= 0x01;
    assert_eq!(
        replay(hash_tampered),
        Err(MembershipError::ContentHashMismatch)
    );
    let mut signature_tampered = genuine.clone();
    let last = signature_tampered.len() - 1;
    signature_tampered[last] ^= 0x01;
    assert_eq!(
        replay(signature_tampered),
        Err(MembershipError::InvalidSignature)
    );
}
