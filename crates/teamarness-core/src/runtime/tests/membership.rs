use super::*;

#[tokio::test]
async fn uncertain_invitation_commit_poisons_runtime_until_authenticated_reopen() {
    let issuer_directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-invitation-uncertain-{}",
        std::process::id()
    ));
    let invitee_directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-invitation-claimant-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&issuer_directory);
    let _ = fs::remove_dir_all(&invitee_directory);
    let config = CoreRuntimeConfig::new(&issuer_directory);
    let issuer = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let invitee = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&invitee_directory))
        .await
        .unwrap();
    let workspace = issuer.create_workspace("Uncertain invitation").unwrap();
    let token = issuer
        .issue_invitation(
            workspace.id(),
            issuer.clock.now_unix_ms().saturating_add(60_000),
        )
        .unwrap();
    let claim = invitee
        .create_invitation_claim(&token, issuer.genesis_id_for_test(workspace.id()))
        .unwrap();

    assert_eq!(
        issuer
            .redeem_invitation_with_test_fault(
                &claim,
                crate::membership::projection::RedemptionBoundary::AfterCommit,
            )
            .unwrap_err(),
        InvitationError::Storage(StorageError::UncertainOutcome)
    );
    assert_eq!(
        issuer
            .issue_invitation(
                workspace.id(),
                issuer.clock.now_unix_ms().saturating_add(60_000),
            )
            .unwrap_err(),
        InvitationError::Storage(StorageError::RuntimePoisoned)
    );
    issuer.shutdown().await.unwrap();
    drop(issuer);

    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    let snapshot = reopened.membership_snapshot(workspace.id()).unwrap();
    // Genesis bundle (epochs 0..=2), then the invitee's group create and admission.
    assert_eq!(snapshot.epoch(), MembershipEpoch::new(4));
    assert_eq!(snapshot.members().len(), 2);
    let recovered = reopened.redeem_invitation(&claim).unwrap();
    assert_eq!(recovered.epoch(), MembershipEpoch::new(4));

    reopened.shutdown().await.unwrap();
    drop(reopened);
    invitee.shutdown().await.unwrap();
    drop(invitee);
    fs::remove_dir_all(issuer_directory).unwrap();
    fs::remove_dir_all(invitee_directory).unwrap();
}

#[tokio::test]
async fn canonical_path_admits_revokes_and_rejects_invalid_authority_without_advancing_state() {
    let directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-membership-validation-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Membership validation").unwrap();
    let manager = runtime_secret(&runtime);
    let member = member_secret(31);
    let genesis = runtime_genesis(&runtime, workspace.id());
    let base = workspace.created_at_unix_ms();

    let (group_create, admission) = admit(
        &manager,
        &member,
        workspace.id(),
        genesis,
        base + 1,
        runtime_heads(&runtime, workspace.id()),
        1,
    );
    // The invitee's group create alone grants nothing and waits for its admission.
    let parked = runtime
        .apply_membership_envelope(group_create.canonical_bytes())
        .unwrap();
    assert!(!parked.accepted && parked.quarantined);
    let accepted = runtime
        .apply_membership_envelope(admission.canonical_bytes())
        .unwrap();
    assert!(accepted.accepted);
    assert!(!accepted.quarantined);
    assert_eq!(accepted.snapshot.epoch(), MembershipEpoch::new(4));

    let duplicate = runtime
        .apply_membership_envelope(admission.canonical_bytes())
        .unwrap();
    assert!(!duplicate.accepted);
    assert_eq!(duplicate.snapshot.epoch(), MembershipEpoch::new(4));
    let transition_count: i64 = runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .connection
        .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(transition_count, 4);

    let outsider = member_secret(32);
    let wrong_token = sign_token(&outsider, workspace.id(), base, base + 100_000, [2; 32]).unwrap();
    let wrong_claim = sign_claim(&member_secret(33), &wrong_token, genesis).unwrap();
    let invalid_proof = MembershipEnvelope::sign(
        &manager,
        workspace.id(),
        base + 2,
        MembershipAction::Admit {
            target: wrong_claim.invitee,
            proof: Box::new(AdmissionProof { claim: wrong_claim }),
        },
        runtime_heads(&runtime, workspace.id()),
    )
    .unwrap();
    assert_eq!(
        runtime
            .apply_membership_envelope(invalid_proof.canonical_bytes())
            .unwrap_err(),
        MembershipError::InvalidAdmissionProof
    );

    let writer_action = MembershipEnvelope::sign(
        &member,
        workspace.id(),
        base + 2,
        MembershipAction::Revoke {
            target: runtime.public_identity(),
            group: group_create.id,
        },
        runtime_heads(&runtime, workspace.id()),
    )
    .unwrap();
    assert_eq!(
        runtime
            .apply_membership_envelope(writer_action.canonical_bytes())
            .unwrap_err(),
        MembershipError::NotManager
    );
    assert_eq!(
        runtime.membership_snapshot(workspace.id()).unwrap().epoch(),
        MembershipEpoch::new(4)
    );

    let revocation = runtime
        .revoke_member(
            workspace.id(),
            PublicIdentity::from_bytes(*member.public().as_bytes()).unwrap(),
        )
        .unwrap();
    assert_eq!(revocation.epoch(), MembershipEpoch::new(5));
    let (readmission_group, readmission) = admit(
        &manager,
        &member,
        workspace.id(),
        genesis,
        base + 100_000,
        vec![revocation.cause()],
        3,
    );
    assert!(
        runtime
            .apply_membership_envelope(readmission_group.canonical_bytes())
            .unwrap()
            .quarantined
    );
    assert_eq!(
        runtime
            .apply_membership_envelope(readmission.canonical_bytes())
            .unwrap_err(),
        MembershipError::CausalReadmission
    );
    let final_snapshot = runtime.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(final_snapshot.epoch(), MembershipEpoch::new(5));
    assert_eq!(final_snapshot.members().len(), 1);

    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

fn topological_orders(prerequisites: &[Vec<usize>]) -> Vec<Vec<usize>> {
    fn visit(prerequisites: &[Vec<usize>], current: &mut Vec<usize>, orders: &mut Vec<Vec<usize>>) {
        if current.len() == prerequisites.len() {
            orders.push(current.clone());
            return;
        }
        for candidate in 0..prerequisites.len() {
            if current.contains(&candidate)
                || !prerequisites[candidate]
                    .iter()
                    .all(|required| current.contains(required))
            {
                continue;
            }
            current.push(candidate);
            visit(prerequisites, current, orders);
            current.pop();
        }
    }
    let mut orders = Vec::new();
    visit(prerequisites, &mut Vec::new(), &mut orders);
    orders
}

#[tokio::test]
async fn every_reachable_topological_order_converges_for_single_manager_branches() {
    let baseline = std::env::temp_dir().join(format!(
        "teamarness-runtime-convergence-baseline-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&baseline);
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&baseline))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Reachable convergence").unwrap();
    let manager = runtime_secret(&runtime);
    let genesis = runtime_genesis(&runtime, workspace.id());
    let heads = runtime_heads(&runtime, workspace.id());
    let base = workspace.created_at_unix_ms();
    let member_a = member_secret(41);
    let member_b = member_secret(42);

    let (group_a, admit_a) = admit(
        &manager,
        &member_a,
        workspace.id(),
        genesis,
        base + 1,
        heads.clone(),
        11,
    );
    let (group_b, admit_b) = admit(
        &manager,
        &member_b,
        workspace.id(),
        genesis,
        base + 3,
        heads,
        13,
    );
    let revoke_a = MembershipEnvelope::sign(
        &manager,
        workspace.id(),
        base + 4,
        MembershipAction::Revoke {
            target: PublicIdentity::from_bytes(*member_a.public().as_bytes()).unwrap(),
            group: group_a.id,
        },
        vec![admit_a.id],
    )
    .unwrap();
    let operations = [group_a, admit_a, group_b, admit_b, revoke_a]
        .map(|operation| operation.canonical_bytes().to_vec());
    runtime.shutdown().await.unwrap();
    drop(runtime);

    // A group create precedes its admission; a revocation follows its admission.
    let orders = topological_orders(&[vec![], vec![0], vec![], vec![2], vec![1]]);
    assert!(orders.len() > 1);
    let mut expected = None;
    for (index, order) in orders.into_iter().enumerate() {
        let directory = std::env::temp_dir().join(format!(
            "teamarness-runtime-convergence-{}-{index}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::copy(
            baseline.join(crate::storage::DATABASE_FILE_NAME),
            directory.join(crate::storage::DATABASE_FILE_NAME),
        )
        .unwrap();
        let branch = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        for operation in order {
            let applied = branch
                .apply_membership_envelope(&operations[operation])
                .unwrap();
            // A member's group create is parked until its admission arrives.
            if operation == 0 || operation == 2 {
                assert!(applied.quarantined);
            } else {
                assert!(applied.accepted);
            }
        }
        let snapshot = branch.membership_snapshot(workspace.id()).unwrap();
        assert_eq!(snapshot.epoch(), MembershipEpoch::new(7));
        let member_a_identity = PublicIdentity::from_bytes(*member_a.public().as_bytes()).unwrap();
        assert!(
            branch
                .was_member(workspace.id(), member_a_identity, base + 1)
                .unwrap()
        );
        assert!(
            branch
                .was_member(workspace.id(), member_a_identity, base + 3)
                .unwrap()
        );
        assert!(
            !branch
                .was_member(workspace.id(), member_a_identity, base + 4)
                .unwrap()
        );
        match &expected {
            Some(expected) => assert_eq!(&snapshot, expected),
            None => expected = Some(snapshot),
        }
        branch.shutdown().await.unwrap();
        drop(branch);
        fs::remove_dir_all(directory).unwrap();
    }

    fs::remove_dir_all(baseline).unwrap();
}

#[tokio::test]
async fn quarantine_retries_dependency_first_and_never_advances_twice() {
    let directory = std::env::temp_dir().join(format!(
        "teamarness-runtime-quarantine-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    let now = Arc::new(AtomicI64::new(1_000));
    let config = CoreRuntimeConfig::new(&directory);
    let runtime =
        CoreRuntime::open_with_clock(config.clone(), Box::new(AdjustableClock(Arc::clone(&now))))
            .await
            .unwrap();
    let workspace = runtime.create_workspace("Quarantine").unwrap();
    let manager = runtime_secret(&runtime);
    let member = member_secret(51);
    let genesis = runtime_genesis(&runtime, workspace.id());
    let (group_create, admission) = admit(
        &manager,
        &member,
        workspace.id(),
        genesis,
        301_001,
        runtime_heads(&runtime, workspace.id()),
        21,
    );
    let revocation = MembershipEnvelope::sign(
        &manager,
        workspace.id(),
        301_002,
        MembershipAction::Revoke {
            target: PublicIdentity::from_bytes(*member.public().as_bytes()).unwrap(),
            group: group_create.id,
        },
        vec![admission.id],
    )
    .unwrap();

    let parked_group = runtime
        .apply_membership_envelope(group_create.canonical_bytes())
        .unwrap();
    let first = runtime
        .apply_membership_envelope(admission.canonical_bytes())
        .unwrap();
    let second = runtime
        .apply_membership_envelope(revocation.canonical_bytes())
        .unwrap();
    assert!(parked_group.quarantined && first.quarantined && second.quarantined);
    assert_eq!(
        runtime.membership_snapshot(workspace.id()).unwrap().epoch(),
        MembershipEpoch::new(2)
    );
    let member_identity = PublicIdentity::from_bytes(*member.public().as_bytes()).unwrap();
    assert!(
        !runtime
            .was_member(workspace.id(), member_identity, 301_001)
            .unwrap()
    );
    let transition_count: i64 = runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .connection
        .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(transition_count, 2);

    runtime.shutdown().await.unwrap();
    drop(runtime);
    now.store(1_002, Ordering::Relaxed);
    let recovered =
        CoreRuntime::open_with_clock(config.clone(), Box::new(AdjustableClock(Arc::clone(&now))))
            .await
            .unwrap();
    let eligible = recovered.membership_snapshot(workspace.id()).unwrap();
    // Group create, admission and revocation leave the quarantine in order.
    assert_eq!(eligible.epoch(), MembershipEpoch::new(5));
    assert_eq!(eligible.members().len(), 1);
    assert!(
        recovered
            .was_member(workspace.id(), member_identity, 301_001)
            .unwrap()
    );
    assert!(
        !recovered
            .was_member(workspace.id(), member_identity, 301_002)
            .unwrap()
    );
    let transition_count: i64 = recovered
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .connection
        .query_row("SELECT COUNT(*) FROM membership_transitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(transition_count, 5);
    assert_eq!(
        recovered.membership_snapshot(workspace.id()).unwrap(),
        eligible
    );
    recovered.shutdown().await.unwrap();
    drop(recovered);

    let reopened =
        CoreRuntime::open_with_clock(config, Box::new(AdjustableClock(Arc::clone(&now))))
            .await
            .unwrap();
    assert_eq!(
        reopened.membership_snapshot(workspace.id()).unwrap(),
        eligible
    );

    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}
