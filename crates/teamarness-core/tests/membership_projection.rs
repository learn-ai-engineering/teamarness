use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use teamarness_core::{
    CoreError, CoreRuntime, CoreRuntimeConfig, MembershipEpoch, MembershipError, MembershipRole,
    StorageError, WorkspaceId,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn private_data_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "teamarness-core-{test_name}-{}-{sequence}",
        std::process::id()
    ))
}

fn database(directory: &Path) -> Connection {
    Connection::open(directory.join("core.sqlite3")).unwrap()
}

#[tokio::test]
async fn workspace_snapshot_is_available_through_core_runtime_and_survives_reopen() {
    let directory = private_data_directory("membership-snapshot");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let creator = runtime.public_identity();
    let workspace = runtime.create_workspace("Projected").unwrap();

    let before = runtime.membership_snapshot(workspace.id()).unwrap();
    assert_eq!(before.workspace_id(), workspace.id());
    assert_eq!(before.epoch(), MembershipEpoch::new(2));
    assert_eq!(before.members().len(), 1);
    assert_eq!(before.members()[0].identity(), creator);
    assert_eq!(before.members()[0].role(), MembershipRole::Manager);
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let reopened = CoreRuntime::open_loopback_for_tests(config).await.unwrap();
    assert_eq!(
        reopened.membership_snapshot(workspace.id()).unwrap(),
        before
    );

    reopened.shutdown().await.unwrap();
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn invalid_public_membership_commands_leave_the_projection_unchanged() {
    let directory = private_data_directory("membership-invalid");
    let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Guarded membership").unwrap();
    let before = runtime.membership_snapshot(workspace.id()).unwrap();

    assert_eq!(
        runtime
            .revoke_member(workspace.id(), runtime.public_identity())
            .unwrap_err(),
        MembershipError::CannotRevokeManager
    );
    assert_eq!(runtime.membership_snapshot(workspace.id()).unwrap(), before);
    assert_eq!(
        runtime
            .membership_snapshot(WorkspaceId::from_str("00000000000000000000000000000000").unwrap())
            .unwrap_err(),
        MembershipError::WorkspaceNotFound
    );

    runtime.shutdown().await.unwrap();
    drop(runtime);
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn derived_rows_are_rebuilt_but_corrupt_canonical_bytes_refuse_open() {
    let directory = private_data_directory("membership-rebuild");
    let config = CoreRuntimeConfig::new(&directory);
    let runtime = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    let workspace = runtime.create_workspace("Repairable").unwrap();
    let expected = runtime.membership_snapshot(workspace.id()).unwrap();
    runtime.shutdown().await.unwrap();
    drop(runtime);

    let connection = database(&directory);
    connection
        .execute("DELETE FROM membership_snapshot_members", [])
        .unwrap();
    connection
        .execute("DELETE FROM workspace_membership_state", [])
        .unwrap();
    drop(connection);

    let reopened = CoreRuntime::open_loopback_for_tests(config.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened.membership_snapshot(workspace.id()).unwrap(),
        expected
    );
    reopened.shutdown().await.unwrap();
    drop(reopened);

    let connection = database(&directory);
    connection
        .execute_batch("DROP TRIGGER membership_operations_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE membership_operations SET canonical_envelope = X'00' WHERE action = 0",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        CoreRuntime::open_loopback_for_tests(config).await,
        Err(CoreError::Storage(StorageError::CorruptWorkspace))
    ));

    fs::remove_dir_all(directory).unwrap();
}

// ---------------------------------------------------------------------------
// Workspace Persons as key groups (ADR 0023): replay authority, determinism and
// the flattened snapshot.
// ---------------------------------------------------------------------------

mod groups {
    use std::fs;
    use std::str::FromStr;
    use std::time::{SystemTime, UNIX_EPOCH};

    use teamarness_core::test_authoring::{
        TestAction, identity_of, operation_id, replay_for_tests, sign_admission, sign_operation,
    };
    use teamarness_core::{
        CoreRuntime, CoreRuntimeConfig, MembershipApplyOutcomeForTests, MembershipEpoch,
        MembershipError, MembershipOperationId, MembershipRole, MembershipSnapshot, PublicIdentity,
        WorkspaceId,
    };

    const CREATOR: [u8; 32] = [1; 32];
    const MEMBER: [u8; 32] = [2; 32];
    const AGENT: [u8; 32] = [3; 32];
    const OUTSIDER: [u8; 32] = [4; 32];
    const REVOKED: [u8; 32] = [5; 32];
    const NEW_KEY: [u8; 32] = [8; 32];
    const NEW_AGENT: [u8; 32] = [9; 32];

    fn now_unix_ms() -> i64 {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    }

    /// A Workspace with a creator, an admitted Member holding one Agent key,
    /// and a revoked Person. Every operation is causally after `heads()`.
    struct Fixture {
        workspace: WorkspaceId,
        operations: Vec<Vec<u8>>,
        genesis: MembershipOperationId,
        creator_group: MembershipOperationId,
        member_group: MembershipOperationId,
        revoked_group: MembershipOperationId,
        head: MembershipOperationId,
    }

    impl Fixture {
        fn build() -> Self {
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
            let creator_group_id = operation_id(&creator_group);
            let attach = sign_operation(
                &CREATOR,
                workspace,
                10,
                TestAction::AttachPerson {
                    group: creator_group_id,
                },
                &[creator_group_id],
            );
            let (member_group, admit_member) = sign_admission(
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
            let member_group_id = operation_id(&member_group);
            let add_agent = sign_operation(
                &MEMBER,
                workspace,
                40,
                TestAction::AddAgentKey {
                    group: member_group_id,
                    agent_secret: AGENT,
                },
                &[operation_id(&admit_member)],
            );
            let (revoked_group, admit_revoked) = sign_admission(
                &CREATOR,
                &REVOKED,
                workspace,
                genesis_id,
                45,
                1_000,
                [2; 32],
                50,
                &[operation_id(&add_agent)],
            );
            let revoked_group_id = operation_id(&revoked_group);
            let revoke = sign_operation(
                &CREATOR,
                workspace,
                60,
                TestAction::Revoke {
                    target: identity_of(&REVOKED),
                    group: revoked_group_id,
                },
                &[operation_id(&admit_revoked)],
            );
            let head = operation_id(&revoke);
            Self {
                workspace,
                operations: vec![
                    genesis,
                    creator_group,
                    attach,
                    member_group,
                    admit_member,
                    add_agent,
                    revoked_group,
                    admit_revoked,
                    revoke,
                ],
                genesis: genesis_id,
                creator_group: creator_group_id,
                member_group: member_group_id,
                revoked_group: revoked_group_id,
                head,
            }
        }

        fn replay(&self, extra: &[Vec<u8>]) -> Result<MembershipSnapshot, MembershipError> {
            let mut all = self.operations.clone();
            all.extend_from_slice(extra);
            replay_for_tests(self.workspace, identity_of(&CREATOR), &all)
        }

        fn sign(&self, author: &[u8; 32], action: TestAction, at: i64) -> Vec<u8> {
            sign_operation(author, self.workspace, at, action, &[self.head])
        }
    }

    /// One matrix row: a name, the operations added to the fixture, and the
    /// replay verdict they must produce.
    type MatrixRow = (String, Vec<Vec<u8>>, Result<(), MembershipError>);

    /// Every actor against every action. Verifies: TEAM.SYS.MEM-R02, MEM-R04,
    /// MEM-R08.
    #[test]
    fn authority_matrix_accepts_only_the_authority_each_group_requires() {
        let fixture = Fixture::build();
        let base = fixture.replay(&[]).expect("the fixture replays");
        assert_eq!(base.members().len(), 3, "creator, Member and its Agent");

        let actors: [(&str, [u8; 32]); 5] = [
            ("creator", CREATOR),
            ("member", MEMBER),
            ("agent", AGENT),
            ("outsider", OUTSIDER),
            ("revoked", REVOKED),
        ];
        // The group each actor manages, when it has one.
        let own_group = |actor: &str| match actor {
            "creator" => fixture.creator_group,
            "member" | "agent" | "outsider" => fixture.member_group,
            _ => fixture.revoked_group,
        };
        let not_manager = |actor: &str| {
            if actor == "agent" {
                Err(MembershipError::AgentKeyCannotAuthor)
            } else {
                Err(MembershipError::NotManager)
            }
        };

        let mut rows: Vec<MatrixRow> = Vec::new();
        for (actor, secret) in actors {
            // Admit: a token the actor issues for a fresh key.
            let (group, admit) = sign_admission(
                &secret,
                &NEW_KEY,
                fixture.workspace,
                fixture.genesis,
                65,
                1_000,
                [3; 32],
                70,
                &[fixture.head],
            );
            rows.push((
                format!("{actor} admits a new key"),
                vec![group, admit],
                if actor == "creator" {
                    Ok(())
                } else {
                    not_manager(actor)
                },
            ));
            // Revoke the Member's group.
            rows.push((
                format!("{actor} revokes the Member"),
                vec![fixture.sign(
                    &secret,
                    TestAction::Revoke {
                        target: identity_of(&MEMBER),
                        group: fixture.member_group,
                    },
                    70,
                )],
                if actor == "creator" {
                    Ok(())
                } else {
                    not_manager(actor)
                },
            ));
            // CreatePerson: inert on its own for any non-Agent key; the runtime
            // parks a non-creator group create until a Manager attaches it.
            rows.push((
                format!("{actor} creates a group"),
                vec![fixture.sign(&secret, TestAction::CreatePerson, 70)],
                if actor == "agent" {
                    Err(MembershipError::AgentKeyCannotAuthor)
                } else {
                    Ok(())
                },
            ));
            // AttachPerson of a group the actor just created.
            let group = fixture.sign(&secret, TestAction::CreatePerson, 70);
            let attach = sign_operation(
                &secret,
                fixture.workspace,
                71,
                TestAction::AttachPerson {
                    group: operation_id(&group),
                },
                &[operation_id(&group)],
            );
            rows.push((
                format!("{actor} attaches its own new group"),
                vec![group, attach],
                match actor {
                    "creator" => Err(MembershipError::DuplicateMemberKey),
                    _ => not_manager(actor),
                },
            ));
            // AddAgentKey into the group the actor manages (or the Member's).
            rows.push((
                format!("{actor} adds an agent key"),
                vec![fixture.sign(
                    &secret,
                    TestAction::AddAgentKey {
                        group: own_group(actor),
                        agent_secret: NEW_AGENT,
                    },
                    70,
                )],
                match actor {
                    "creator" | "member" => Ok(()),
                    "revoked" => Err(MembershipError::PersonNotAdmitted),
                    _ => not_manager(actor),
                },
            ));
            // RemoveAgentKey of the Member's Agent from the Member's group (the
            // Manager included: it is not that group's manager), or from the
            // revoked Person's own group.
            rows.push((
                format!("{actor} removes an agent key"),
                vec![fixture.sign(
                    &secret,
                    TestAction::RemoveAgentKey {
                        group: if actor == "revoked" {
                            fixture.revoked_group
                        } else {
                            fixture.member_group
                        },
                        agent: identity_of(&AGENT),
                    },
                    70,
                )],
                match actor {
                    "member" => Ok(()),
                    "revoked" => Err(MembershipError::PersonNotAdmitted),
                    _ => not_manager(actor),
                },
            ));
            // Removing a key that is not in the actor's own group.
            if actor == "creator" {
                rows.push((
                    "creator removes a key its own group does not hold".to_owned(),
                    vec![fixture.sign(
                        &secret,
                        TestAction::RemoveAgentKey {
                            group: fixture.creator_group,
                            agent: identity_of(&AGENT),
                        },
                        70,
                    )],
                    Err(MembershipError::MemberNotFound),
                ));
            }
        }

        // Cross-group and readmission rules.
        rows.push((
            "member adds a key to another Person's group".to_owned(),
            vec![fixture.sign(
                &MEMBER,
                TestAction::AddAgentKey {
                    group: fixture.creator_group,
                    agent_secret: NEW_AGENT,
                },
                70,
            )],
            Err(MembershipError::NotManager),
        ));
        let unattached = fixture.sign(&MEMBER, TestAction::CreatePerson, 70);
        rows.push((
            "member adds a key to a group that is not admitted".to_owned(),
            vec![
                unattached.clone(),
                sign_operation(
                    &MEMBER,
                    fixture.workspace,
                    71,
                    TestAction::AddAgentKey {
                        group: operation_id(&unattached),
                        agent_secret: NEW_AGENT,
                    },
                    &[operation_id(&unattached)],
                ),
            ],
            Err(MembershipError::PersonNotAdmitted),
        ));
        rows.push((
            "creator adds the Member's Agent key to its own group".to_owned(),
            vec![fixture.sign(
                &CREATOR,
                TestAction::AddAgentKey {
                    group: fixture.creator_group,
                    agent_secret: AGENT,
                },
                70,
            )],
            Err(MembershipError::DuplicateMemberKey),
        ));
        rows.push((
            "member adds its own Installation key as an agent".to_owned(),
            vec![fixture.sign(
                &MEMBER,
                TestAction::AddAgentKey {
                    group: fixture.member_group,
                    agent_secret: MEMBER,
                },
                70,
            )],
            Err(MembershipError::DuplicateMemberKey),
        ));
        let (member_again_group, member_again) = sign_admission(
            &CREATOR,
            &MEMBER,
            fixture.workspace,
            fixture.genesis,
            65,
            1_000,
            [4; 32],
            70,
            &[fixture.head],
        );
        rows.push((
            "creator admits a current Member again".to_owned(),
            vec![member_again_group, member_again],
            Err(MembershipError::DuplicateMemberKey),
        ));
        let (revoked_again_group, revoked_again) = sign_admission(
            &CREATOR,
            &REVOKED,
            fixture.workspace,
            fixture.genesis,
            65,
            1_000,
            [5; 32],
            70,
            &[fixture.head],
        );
        rows.push((
            "creator readmits a revoked Person".to_owned(),
            vec![revoked_again_group, revoked_again],
            Err(MembershipError::CausalReadmission),
        ));
        let remove = fixture.sign(
            &MEMBER,
            TestAction::RemoveAgentKey {
                group: fixture.member_group,
                agent: identity_of(&AGENT),
            },
            70,
        );
        rows.push((
            "member re-adds its removed Agent key".to_owned(),
            vec![
                remove.clone(),
                sign_operation(
                    &MEMBER,
                    fixture.workspace,
                    71,
                    TestAction::AddAgentKey {
                        group: fixture.member_group,
                        agent_secret: AGENT,
                    },
                    &[operation_id(&remove)],
                ),
            ],
            Err(MembershipError::CausalReadmission),
        ));
        rows.push((
            "creator adds a removed Agent key to its own group".to_owned(),
            vec![
                remove.clone(),
                sign_operation(
                    &CREATOR,
                    fixture.workspace,
                    71,
                    TestAction::AddAgentKey {
                        group: fixture.creator_group,
                        agent_secret: AGENT,
                    },
                    &[operation_id(&remove)],
                ),
            ],
            Err(MembershipError::CausalReadmission),
        ));
        rows.push((
            "creator revokes itself".to_owned(),
            vec![fixture.sign(
                &CREATOR,
                TestAction::Revoke {
                    target: identity_of(&CREATOR),
                    group: fixture.creator_group,
                },
                70,
            )],
            Err(MembershipError::CannotRevokeManager),
        ));
        rows.push((
            "creator revokes an already revoked Person".to_owned(),
            vec![fixture.sign(
                &CREATOR,
                TestAction::Revoke {
                    target: identity_of(&REVOKED),
                    group: fixture.revoked_group,
                },
                70,
            )],
            Err(MembershipError::MemberNotFound),
        ));
        rows.push((
            "creator revokes with a group the target does not manage".to_owned(),
            vec![fixture.sign(
                &CREATOR,
                TestAction::Revoke {
                    target: identity_of(&MEMBER),
                    group: fixture.creator_group,
                },
                70,
            )],
            Err(MembershipError::UnknownPersonGroup),
        ));

        let mut failures = Vec::new();
        for (name, extra, expected) in rows {
            let outcome = fixture.replay(&extra).map(|_| ());
            if outcome != expected {
                failures.push(format!("{name}: expected {expected:?}, got {outcome:?}"));
            }
        }
        assert!(
            failures.is_empty(),
            "authority matrix:\n{}",
            failures.join("\n")
        );
    }

    /// A group create from anyone but the creator is parked by the runtime
    /// until a Manager attaches it; it never enters the accepted set alone.
    #[tokio::test]
    async fn runtime_parks_a_bare_group_create_until_a_manager_attaches_it() {
        let directory = super::private_data_directory("group-create-parked");
        let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        let workspace = runtime.create_workspace("Parked").unwrap().id();
        let before = runtime.membership_snapshot(workspace).unwrap();
        let heads = runtime.membership_heads_for_tests(workspace);
        let create = sign_operation(
            &OUTSIDER,
            workspace,
            now_unix_ms(),
            TestAction::CreatePerson,
            &heads,
        );
        assert_eq!(
            runtime
                .apply_membership_operation_for_tests(&create)
                .unwrap(),
            MembershipApplyOutcomeForTests::Quarantined
        );
        assert_eq!(runtime.membership_snapshot(workspace).unwrap(), before);
        runtime.shutdown().await.unwrap();
        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }

    /// Verifies: TEAM.SYS.MEM-R07. The same operation set applied in three
    /// causal-respecting orders yields the same snapshot, version and epoch.
    #[tokio::test]
    async fn deterministic_replay_in_three_causal_orders_yields_one_snapshot() {
        let baseline = super::private_data_directory("deterministic-baseline");
        let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&baseline))
            .await
            .unwrap();
        let workspace = runtime.create_workspace("Deterministic").unwrap().id();
        let creator = runtime.installation_secret_for_tests();
        let genesis = runtime.genesis_id_for_tests(workspace);
        let heads = runtime.membership_heads_for_tests(workspace);
        runtime.shutdown().await.unwrap();
        drop(runtime);

        let now = now_unix_ms();
        let (group_m, admit_m) = sign_admission(
            &creator,
            &MEMBER,
            workspace,
            genesis,
            now,
            now + 100_000,
            [1; 32],
            now,
            &heads,
        );
        let add_a = sign_operation(
            &MEMBER,
            workspace,
            now + 1,
            TestAction::AddAgentKey {
                group: operation_id(&group_m),
                agent_secret: AGENT,
            },
            &[operation_id(&admit_m)],
        );
        let (group_n, admit_n) = sign_admission(
            &creator,
            &REVOKED,
            workspace,
            genesis,
            now,
            now + 100_000,
            [2; 32],
            now,
            &heads,
        );
        let revoke_m = sign_operation(
            &creator,
            workspace,
            now + 2,
            TestAction::Revoke {
                target: identity_of(&MEMBER),
                group: operation_id(&group_m),
            },
            &[operation_id(&add_a)],
        );
        let operations = [group_m, admit_m, add_a, group_n, admit_n, revoke_m];
        // Indices: 0 group_m, 1 admit_m, 2 add_a, 3 group_n, 4 admit_n, 5 revoke_m.
        let orders: [[usize; 6]; 3] = [[0, 1, 2, 3, 4, 5], [3, 4, 0, 1, 2, 5], [0, 3, 1, 4, 2, 5]];
        let mut snapshots = Vec::new();
        for (index, order) in orders.iter().enumerate() {
            let directory = super::private_data_directory(&format!("deterministic-{index}"));
            fs::create_dir_all(&directory).unwrap();
            fs::copy(
                baseline.join("core.sqlite3"),
                directory.join("core.sqlite3"),
            )
            .unwrap();
            let branch = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
                .await
                .unwrap();
            for position in order {
                let outcome = branch
                    .apply_membership_operation_for_tests(&operations[*position])
                    .unwrap();
                if *position == 0 || *position == 3 {
                    assert_eq!(outcome, MembershipApplyOutcomeForTests::Quarantined);
                } else {
                    assert!(matches!(
                        outcome,
                        MembershipApplyOutcomeForTests::Accepted(_)
                    ));
                }
            }
            let snapshot = branch.membership_snapshot(workspace).unwrap();
            assert_eq!(snapshot.epoch(), MembershipEpoch::new(8));
            snapshots.push((
                snapshot,
                format!("{:?}", branch.membership_snapshot(workspace).unwrap()),
            ));
            branch.shutdown().await.unwrap();
            drop(branch);
            fs::remove_dir_all(directory).unwrap();
        }
        for (snapshot, rendered) in &snapshots[1..] {
            assert_eq!(snapshot, &snapshots[0].0);
            assert_eq!(snapshot.version(), snapshots[0].0.version());
            assert_eq!(rendered, &snapshots[0].1, "byte-identical rendering");
        }
        // The pure replay of the whole set agrees with the incremental runtimes.
        let mut all = vec![];
        let reopened = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&baseline))
            .await
            .unwrap();
        all.extend(reopened.export_membership_operations_for_tests(workspace));
        reopened.shutdown().await.unwrap();
        drop(reopened);
        all.extend(operations.iter().cloned());
        let pure = replay_for_tests(workspace, identity_of(&creator), &all).unwrap();
        assert_eq!(pure.members(), snapshots[0].0.members());
        assert_eq!(pure.version(), snapshots[0].0.version());
        fs::remove_dir_all(baseline).unwrap();
    }

    /// Verifies: TEAM.SYS.MEM-R04, MEM-R07, MEM-R08. Every key in the flattened
    /// snapshot resolves to the creator or exactly one Person group, only the
    /// creator manages, a revoked Person's keys are absent, and an Agent key's
    /// history is exactly its Person's interval.
    #[tokio::test]
    async fn flattened_snapshot_resolves_every_key_and_bounds_agent_history() {
        let directory = super::private_data_directory("flattened-snapshot");
        let runtime = CoreRuntime::open_loopback_for_tests(CoreRuntimeConfig::new(&directory))
            .await
            .unwrap();
        let workspace = runtime.create_workspace("Flattened").unwrap().id();
        let creator_secret = runtime.installation_secret_for_tests();
        let creator = runtime.public_identity();
        let genesis = runtime.genesis_id_for_tests(workspace);
        let now = now_unix_ms();

        let (group_m, admit_m) = sign_admission(
            &creator_secret,
            &MEMBER,
            workspace,
            genesis,
            now,
            now + 100_000,
            [1; 32],
            now,
            &runtime.membership_heads_for_tests(workspace),
        );
        runtime
            .apply_membership_operation_for_tests(&group_m)
            .unwrap();
        runtime
            .apply_membership_operation_for_tests(&admit_m)
            .unwrap();
        let added_at = now;
        let add_a = sign_operation(
            &MEMBER,
            workspace,
            added_at,
            TestAction::AddAgentKey {
                group: operation_id(&group_m),
                agent_secret: AGENT,
            },
            &runtime.membership_heads_for_tests(workspace),
        );
        runtime
            .apply_membership_operation_for_tests(&add_a)
            .unwrap();

        let member = identity_of(&MEMBER);
        let agent = identity_of(&AGENT);
        let snapshot = runtime.membership_snapshot(workspace).unwrap();
        let roles = snapshot
            .members()
            .iter()
            .map(|entry| (entry.identity(), entry.role()))
            .collect::<Vec<_>>();
        assert_eq!(roles.len(), 3);
        assert!(roles.contains(&(creator, MembershipRole::Manager)));
        assert!(roles.contains(&(member, MembershipRole::Member)));
        assert!(roles.contains(&(agent, MembershipRole::Member)));
        let managers = roles
            .iter()
            .filter(|(_, role)| *role == MembershipRole::Manager)
            .map(|(identity, _)| *identity)
            .collect::<Vec<PublicIdentity>>();
        assert_eq!(managers, vec![creator]);

        // Every key resolves to the creator or exactly one Person group.
        let creator_group = runtime.person_group_for_tests(workspace, creator).unwrap();
        let member_group = runtime.person_group_for_tests(workspace, member).unwrap();
        assert_ne!(creator_group, member_group);
        assert_eq!(member_group, operation_id(&group_m));
        assert_eq!(runtime.person_group_for_tests(workspace, agent), None);
        let mut installations = runtime.installation_members_for_tests(workspace);
        installations.sort();
        let mut expected = vec![creator, member];
        expected.sort();
        assert_eq!(
            installations, expected,
            "agent keys are Members but not Installations"
        );

        assert!(!runtime.was_member(workspace, agent, added_at - 1).unwrap());
        assert!(runtime.was_member(workspace, agent, added_at).unwrap());

        let revocation = runtime.revoke_member(workspace, member).unwrap();
        let revoked_at = runtime
            .membership_snapshot(workspace)
            .map(|_| now_unix_ms())
            .unwrap();
        let after = runtime.membership_snapshot(workspace).unwrap();
        assert_eq!(after.members().len(), 1);
        assert_eq!(after.members()[0].identity(), creator);
        assert_eq!(after.epoch(), revocation.epoch());
        assert!(runtime.was_member(workspace, agent, added_at).unwrap());
        assert!(
            !runtime
                .was_member(workspace, agent, revoked_at + 1)
                .unwrap()
        );
        assert!(
            !runtime
                .was_member(workspace, member, revoked_at + 1)
                .unwrap()
        );
        assert_eq!(runtime.person_group_for_tests(workspace, member), None);

        runtime.shutdown().await.unwrap();
        drop(runtime);
        fs::remove_dir_all(directory).unwrap();
    }
}
