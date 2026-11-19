//! Local operator adapters. Core remains the authority for membership and stewardship.
use std::sync::Arc;

use serde_json::{Value, json};
use teamarness_core::{
    CoreRuntime, IdentityError, MembershipError, OnboardingError, PersonNameError, PrincipalKind,
    StorageError, WorkspaceError, WorkspaceInvitation,
};
use teamarness_package_host::{
    InstallPersonal, PackageErrorCode, PackageErrorEnvelope, PackageHost, PackageHostCommand,
    PackageInstanceKey, ReconcileWorkspaces, RecoverPackage, ReloadPersonal, RemovePersonal,
    RestartPersonal, SuspendPackage,
};

use crate::control::Operation;
use crate::{ControlError, ErrorCode, ErrorDetails};

pub(crate) async fn dispatch(
    core: Arc<CoreRuntime>,
    host: PackageHost,
    operation: Operation,
) -> Result<Value, ControlError> {
    match operation {
        Operation::PackagesList {} => encode(host.snapshot().await),
        Operation::PackagesInspect { directory } => encode(
            host.inspect_directory(directory)
                .await
                .map_err(host_error)?,
        ),
        Operation::PackagesInstall {
            directory,
            expected_identity,
            accepted_risks,
        } => {
            execute(
                &host,
                PackageHostCommand::InstallPersonal(InstallPersonal {
                    directory,
                    expected_identity,
                    accepted_risks,
                }),
            )
            .await
        }
        Operation::PackagesReload { package } => {
            execute(
                &host,
                PackageHostCommand::ReloadPersonal(ReloadPersonal { identity: package }),
            )
            .await
        }
        Operation::PackagesRemove { package } => {
            execute(
                &host,
                PackageHostCommand::RemovePersonal(RemovePersonal { identity: package }),
            )
            .await
        }
        Operation::PackagesRestart { workspace, package } => {
            execute(
                &host,
                PackageHostCommand::RestartPersonal(RestartPersonal {
                    instance: instance(&workspace, package)?,
                }),
            )
            .await
        }
        Operation::PackagesPause { workspace, package } => {
            let instance = instance(&workspace, package)?;
            require_personal_instance(&host, &instance).await?;
            execute(
                &host,
                PackageHostCommand::Suspend(SuspendPackage { instance }),
            )
            .await
        }
        Operation::PackagesResume { workspace, package } => {
            let instance = instance(&workspace, package)?;
            require_personal_instance(&host, &instance).await?;
            execute(
                &host,
                PackageHostCommand::Recover(RecoverPackage { instance }),
            )
            .await
        }
        Operation::WorkspaceJoin {
            invitation,
            person_name,
        } => {
            let invitation: WorkspaceInvitation = invitation
                .parse()
                .map_err(|_| ControlError::new(ErrorCode::InvalidInput))?;
            // The existing async join also performs synchronous recovery. Run its entire future on
            // a blocking thread, not an async worker, while retaining the server's operation permit.
            let runtime = tokio::runtime::Handle::current();
            let joined = tokio::task::spawn_blocking(move || {
                runtime.block_on(core.join_workspace_with_person_name(&invitation, &person_name))
            })
            .await
            .map_err(|_| uncertain_internal())?
            .map_err(onboarding_error)?;
            let workspace = workspace(&joined.0);
            match host
                .execute(PackageHostCommand::ReconcileWorkspaces(
                    ReconcileWorkspaces {},
                ))
                .await
            {
                Ok(outcome) => Ok(
                    json!({"membershipCommitted":true,"workspace":workspace,"reconciliation":outcome}),
                ),
                Err(error) => {
                    let mut failure = host_error(error.clone());
                    failure.code = ErrorCode::ReconciliationFailed;
                    failure.details = Some(ErrorDetails::Joined {
                        membership_committed: true,
                        workspace,
                        reconciliation: Box::new(error),
                    });
                    Err(failure)
                }
            }
        }
        operation => tokio::task::spawn_blocking(move || core_dispatch(&core, operation))
            .await
            .map_err(|_| uncertain_internal())?,
    }
}

fn core_dispatch(core: &CoreRuntime, operation: Operation) -> Result<Value, ControlError> {
    let agents_only = matches!(operation, Operation::AgentList { .. });
    match operation {
        Operation::IdentityShow {} => Ok(
            json!({"installation":core.public_identity().to_string(),"displayName":core.display_name()}),
        ),
        Operation::IdentitySetName { name } => {
            core.set_display_name(&name).map_err(identity_error)?;
            Ok(json!({"changed":true}))
        }
        Operation::WorkspaceList {} => Ok(Value::Array(
            core.list_workspaces()
                .map_err(workspace_error)?
                .iter()
                .map(workspace)
                .collect(),
        )),
        Operation::WorkspaceMembers { workspace } | Operation::AgentList { workspace } => {
            let (membership, rows) = core
                .principal_roster(parse_workspace(&workspace)?)
                .map_err(|error| principal_error(error, false))?;
            Ok(Value::Array(rows.iter()
                .filter(|row| !agents_only || row.kind() == PrincipalKind::Agent)
                .map(|row| {
                    let manager = membership.members().iter().any(|member| {
                        row.keys().contains(&member.identity()) && member.role() == teamarness_core::MembershipRole::Manager
                    });
                    json!({
                        "id": row.id().to_string(),
                        "kind": match row.kind() { PrincipalKind::Person => "person", PrincipalKind::Agent => "agent" },
                        "role": if manager { "manager" } else { "member" },
                        "name": row.name(),
                        "fingerprint": row.fingerprint(),
                        "steward": row.steward_principal_id().map(|id| id.to_string()),
                    })
                }).collect()))
        }
        Operation::WorkspaceRenameSelf { workspace, name } => {
            core.set_own_person_name(parse_workspace(&workspace)?, &name)
                .map_err(|error| principal_error(error, true))?;
            Ok(json!({"changed":true}))
        }
        Operation::AgentAdd { workspace, name } => {
            let id = core
                .add_agent(parse_workspace(&workspace)?, &name)
                .map_err(|error| principal_error(error, true))?;
            Ok(json!({"id":id.to_string()}))
        }
        Operation::AgentRename {
            workspace,
            agent,
            name,
        } => {
            let workspace = parse_workspace(&workspace)?;
            let agent = agent
                .parse()
                .map_err(|_| ControlError::new(ErrorCode::InvalidInput))?;
            let row = core
                .principal(workspace, agent)
                .map_err(|error| principal_error(error, false))?;
            if !row.is_some_and(|row| row.kind() == PrincipalKind::Agent) {
                return Err(principal_error(PersonNameError::AgentNotFound, false));
            }
            core.rename_principal(workspace, agent, &name)
                .map_err(|error| principal_error(error, true))?;
            Ok(json!({"changed":true}))
        }
        Operation::AgentRemove { workspace, agent } => {
            core.remove_agent(
                parse_workspace(&workspace)?,
                agent
                    .parse()
                    .map_err(|_| ControlError::new(ErrorCode::InvalidInput))?,
            )
            .map_err(|error| principal_error(error, true))?;
            Ok(json!({"changed":true}))
        }
        _ => Err(ControlError::new(ErrorCode::InvalidInput)),
    }
}

pub(crate) fn parse_workspace(value: &str) -> Result<teamarness_core::WorkspaceId, ControlError> {
    value
        .parse()
        .map_err(|_| ControlError::new(ErrorCode::InvalidInput))
}

fn instance(
    workspace: &str,
    package: teamarness_package_host::PackageIdentity,
) -> Result<PackageInstanceKey, ControlError> {
    let workspace = parse_workspace(workspace)?.to_string();
    Ok(PackageInstanceKey::new(
        package,
        teamarness_package_host::WorkspaceId::new(workspace)
            .map_err(|_| ControlError::new(ErrorCode::InvalidInput))?,
    ))
}

fn workspace(value: &teamarness_core::WorkspaceSummary) -> Value {
    json!({"id":value.id().to_string(),"name":value.name(),"createdBy":value.created_by().to_string(),"createdAtUnixMs":value.created_at_unix_ms()})
}

async fn require_personal_instance(
    host: &PackageHost,
    instance: &PackageInstanceKey,
) -> Result<(), ControlError> {
    use teamarness_package_host::{
        DiagnosticId, PackageErrorStage, PackageSourceKind, PackageTier, RecoveryAction,
    };
    let snapshot = host.snapshot().await;
    if !snapshot.instances.iter().any(|row| row.key == *instance) {
        // Let the generic Host command report an absent Instance normally.
        return Ok(());
    }
    // Team membership is immutable for this Host lifetime, including local overrides.
    let team = snapshot.packages.iter().any(|row| {
        row.identity == instance.package_identity && row.source == PackageSourceKind::Bundled
    }) || snapshot.registrations.iter().any(|row| {
        row.expected_identity == instance.package_identity.as_str() && row.tier == PackageTier::Team
    });
    if team {
        let mut envelope = PackageErrorEnvelope::new(
            PackageErrorCode::OverrideRequired,
            PackageErrorStage::Admission,
            false,
            RecoveryAction::CorrectPackageSource,
            DiagnosticId::new("operator-team-lifecycle").expect("static diagnostic ID"),
        );
        envelope.affected_package = Some(instance.package_identity.clone());
        return Err(host_error(envelope));
    }
    Ok(())
}

async fn execute(host: &PackageHost, command: PackageHostCommand) -> Result<Value, ControlError> {
    let outcome = host.execute(command).await.map_err(|envelope| {
        let uncertain_storage = envelope.code == PackageErrorCode::DataDirectoryUnavailable;
        let mut error = host_error(envelope);
        error.outcome_uncertain |= uncertain_storage;
        error
    })?;
    encode(outcome)
}

fn encode(value: impl serde::Serialize) -> Result<Value, ControlError> {
    serde_json::to_value(value).map_err(|_| ControlError::new(ErrorCode::Internal))
}

fn uncertain_internal() -> ControlError {
    let mut error = ControlError::new(ErrorCode::Internal);
    error.outcome_uncertain = true;
    error
}

fn host_error(envelope: PackageErrorEnvelope) -> ControlError {
    let mut error = ControlError::new(
        if matches!(
            envelope.code,
            PackageErrorCode::InvalidRegistration
                | PackageErrorCode::ManifestMalformed
                | PackageErrorCode::ManifestUnsupported
                | PackageErrorCode::InvalidPackageIdentity
                | PackageErrorCode::IdentityMismatch
                | PackageErrorCode::TrustEvidenceMissing
                | PackageErrorCode::TargetUnsupported
                | PackageErrorCode::ArtifactChanged
                | PackageErrorCode::UiContractViolation
        ) {
            ErrorCode::InvalidInput
        } else {
            ErrorCode::HostFailure
        },
    );
    error.retryable = envelope.retryable;
    error.outcome_uncertain = envelope.code == PackageErrorCode::OutcomeUncertain;
    error.details = Some(ErrorDetails::Host {
        envelope: Box::new(envelope),
    });
    error
}

fn onboarding_error(value: OnboardingError) -> ControlError {
    // Core uses Authentication for partial writes and Overload for response-read failures too.
    // These coarse errors cannot prove that issuer admission did not commit.
    let uncertain = matches!(
        value,
        OnboardingError::Timeout
            | OnboardingError::Authentication
            | OnboardingError::Overload
            | OnboardingError::InvalidBootstrap
            | OnboardingError::LocalConflict
            | OnboardingError::UncertainStorageOutcome
    );
    let mut error = core_error(
        "onboarding",
        value,
        uncertain,
        value == OnboardingError::InvalidInput,
    );
    error.retryable = value.is_retryable();
    error
}

fn identity_error(value: IdentityError) -> ControlError {
    let uncertain = matches!(value, IdentityError::Storage(error) if uncertain_storage(error));
    core_error(
        "identity",
        value,
        uncertain,
        value == IdentityError::InvalidDisplayName,
    )
}

fn workspace_error(value: WorkspaceError) -> ControlError {
    core_error("workspace", value, false, false)
}

fn principal_error(value: PersonNameError, mutation: bool) -> ControlError {
    let uncertain = mutation
        && matches!(value,
        PersonNameError::Storage(error) | PersonNameError::Membership(MembershipError::Storage(error)) if uncertain_storage(error));
    core_error(
        "principal",
        value,
        uncertain,
        value == PersonNameError::InvalidName,
    )
}

fn uncertain_storage(value: StorageError) -> bool {
    matches!(
        value,
        StorageError::UncertainOutcome | StorageError::RuntimePoisoned | StorageError::Database
    )
}

fn core_error(
    family: &str,
    value: impl std::fmt::Debug,
    uncertain: bool,
    invalid: bool,
) -> ControlError {
    // Only Core's payload-free public error enums enter this adapter. Preserve their variant and
    // nested cause rather than erasing them or carrying arbitrary source errors/operator input.
    let mut error = ControlError::new(if invalid {
        ErrorCode::InvalidInput
    } else {
        ErrorCode::CoreFailure
    });
    error.outcome_uncertain = uncertain;
    error.details = Some(ErrorDetails::Core {
        family: family.to_owned(),
        code: format!("{value:?}"),
    });
    error
}
