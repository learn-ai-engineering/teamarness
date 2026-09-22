use std::path::PathBuf;
use std::str::FromStr;

use teamarness_package_host::{
    AcknowledgeUiEvents, CancelUiRequest, CloseUiMount, CloseUiSubscription, DisposePackage,
    HostRevision, LifecycleState, MountState, OpenUiMount, OpenUiSubscription, PackageErrorCode,
    PackageErrorEnvelope, PackageErrorStage, PackageHostCommand, PackageHostConfig,
    PackageHostOutcome, PackageHostSnapshot, PackageHostState, PackageIdentity, ReadUiResource,
    ReconcileWorkspaces, RecoverPackage, RecoveryAction, ReportUiFault, ShutdownHost,
    SuspendPackage, UiRequest, UiResourceToken,
};

#[test]
fn package_identity_enforces_reverse_dns_contract() {
    for valid in ["dev.teamarness.reference", "com.example.my-package", "a.bc"] {
        assert_eq!(PackageIdentity::from_str(valid).unwrap().as_str(), valid);
    }
    for invalid in [
        "ab",
        "Dev.example",
        "example",
        ".example",
        "example.",
        "example.-bad",
        "example.bad-",
        "example.bad_name",
    ] {
        assert!(PackageIdentity::from_str(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn complete_command_and_outcome_vocabulary_is_constructible() {
    let package = PackageIdentity::from_str("dev.teamarness.reference").unwrap();
    let instance = teamarness_package_host::PackageInstanceKey::new(
        package,
        teamarness_package_host::WorkspaceId::new("0123456789abcdef0123456789abcdef").unwrap(),
    );
    let mount = teamarness_package_host::UiMountId::new("mount-1").unwrap();
    let request = teamarness_package_host::UiRequestId::new("request-1").unwrap();
    let subscription = teamarness_package_host::UiSubscriptionId::new("subscription-1").unwrap();

    let commands = vec![
        PackageHostCommand::ReconcileWorkspaces(ReconcileWorkspaces {}),
        PackageHostCommand::Suspend(SuspendPackage {
            instance: instance.clone(),
        }),
        PackageHostCommand::Recover(RecoverPackage {
            instance: instance.clone(),
        }),
        PackageHostCommand::Dispose(DisposePackage {
            instance: instance.clone(),
        }),
        PackageHostCommand::OpenUiMount(OpenUiMount {
            instance: instance.clone(),
        }),
        PackageHostCommand::CloseUiMount(CloseUiMount {
            mount: mount.clone(),
        }),
        PackageHostCommand::UiRequest(UiRequest {
            mount: mount.clone(),
            request: request.clone(),
            operation: "reference.read".into(),
            payload: teamarness_package_host::PackageValue::Object(Default::default()),
        }),
        PackageHostCommand::CancelUiRequest(CancelUiRequest {
            mount: mount.clone(),
            request,
        }),
        PackageHostCommand::OpenUiSubscription(OpenUiSubscription {
            mount: mount.clone(),
            subscription: subscription.clone(),
            operation: "reference.changes".into(),
            payload: teamarness_package_host::PackageValue::Object(Default::default()),
        }),
        PackageHostCommand::AcknowledgeUiEvents(AcknowledgeUiEvents {
            mount: mount.clone(),
            subscription: subscription.clone(),
            through_sequence: 1,
        }),
        PackageHostCommand::CloseUiSubscription(CloseUiSubscription {
            mount: mount.clone(),
            subscription,
        }),
        PackageHostCommand::ReportUiFault(ReportUiFault {
            mount: mount.clone(),
            code: "ui_exception".into(),
        }),
        PackageHostCommand::ReadUiResource(ReadUiResource {
            mount,
            token: UiResourceToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            relative_path: "ui/index.js".into(),
        }),
        PackageHostCommand::Shutdown(ShutdownHost),
    ];
    assert_eq!(commands.len(), 14);
    for command in &commands {
        match command {
            PackageHostCommand::ReconcileWorkspaces(_)
            | PackageHostCommand::Suspend(_)
            | PackageHostCommand::Recover(_)
            | PackageHostCommand::Dispose(_)
            | PackageHostCommand::OpenUiMount(_)
            | PackageHostCommand::CloseUiMount(_)
            | PackageHostCommand::UiRequest(_)
            | PackageHostCommand::CancelUiRequest(_)
            | PackageHostCommand::OpenUiSubscription(_)
            | PackageHostCommand::AcknowledgeUiEvents(_)
            | PackageHostCommand::CloseUiSubscription(_)
            | PackageHostCommand::ReportUiFault(_)
            | PackageHostCommand::ReadUiResource(_)
            | PackageHostCommand::Shutdown(_) => {}
        }
    }

    let outcomes = vec![
        PackageHostOutcome::Reconciled {
            revision: HostRevision::INITIAL,
        },
        PackageHostOutcome::Suspended {
            revision: HostRevision::INITIAL,
        },
        PackageHostOutcome::Recovered {
            revision: HostRevision::INITIAL,
        },
        PackageHostOutcome::Disposed {
            revision: HostRevision::INITIAL,
        },
        PackageHostOutcome::UiMountOpened {
            mount: teamarness_package_host::UiMountId::new("mount-2").unwrap(),
            loader: teamarness_package_host::UiLoaderDescriptor {
                token: UiResourceToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
                entrypoint: "ui/index.js".into(),
                ui_host_version: 1,
                contract_digest: "digest".into(),
                package: teamarness_package_host::UiPackageContext {
                    identity: PackageIdentity::from_str("dev.teamarness.reference").unwrap(),
                    version: "1.0.0".into(),
                    display_name: "Reference".into(),
                    source: teamarness_package_host::PackageSourceKind::Bundled,
                    trust: teamarness_package_host::PackageTrustKind::ApplicationBundled,
                    lifecycle: LifecycleState::Active,
                    process_generation: 1,
                },
                workspace: teamarness_package_host::UiWorkspaceContext {
                    id: teamarness_package_host::WorkspaceId::new("workspace").unwrap(),
                    name: "Workspace".into(),
                },
            },
            revision: HostRevision::INITIAL,
        },
        PackageHostOutcome::UiMountClosed {
            revision: HostRevision::INITIAL,
        },
        PackageHostOutcome::UiResponse {
            payload: teamarness_package_host::PackageValue::Object(Default::default()),
        },
        PackageHostOutcome::UiRequestCancelled,
        PackageHostOutcome::UiSubscriptionOpened,
        PackageHostOutcome::UiEventsAcknowledged,
        PackageHostOutcome::UiSubscriptionClosed,
        PackageHostOutcome::UiFaultRecorded {
            revision: HostRevision::INITIAL,
        },
        PackageHostOutcome::UiResource {
            bytes: vec![],
            media_type: "text/javascript".into(),
        },
        PackageHostOutcome::Shutdown,
    ];
    assert_eq!(outcomes.len(), 14);
    for outcome in &outcomes {
        match outcome {
            PackageHostOutcome::Reconciled { .. }
            | PackageHostOutcome::Suspended { .. }
            | PackageHostOutcome::Recovered { .. }
            | PackageHostOutcome::Disposed { .. }
            | PackageHostOutcome::UiMountOpened { .. }
            | PackageHostOutcome::UiMountClosed { .. }
            | PackageHostOutcome::UiResponse { .. }
            | PackageHostOutcome::UiRequestCancelled
            | PackageHostOutcome::UiSubscriptionOpened
            | PackageHostOutcome::UiEventsAcknowledged
            | PackageHostOutcome::UiSubscriptionClosed
            | PackageHostOutcome::UiFaultRecorded { .. }
            | PackageHostOutcome::UiResource { .. }
            | PackageHostOutcome::Shutdown => {}
        }
    }
}

#[test]
fn snapshots_and_errors_expose_only_host_values() {
    let config = PackageHostConfig::new(
        PathBuf::from("/application"),
        PathBuf::from("/application/packages/application-packages.v1.json"),
        PathBuf::from("/private"),
        teamarness_package_host::InstallationPublicIdentity::new("installation1").unwrap(),
    );
    assert_eq!(config.target(), "aarch64-apple-darwin");

    let snapshot = PackageHostSnapshot {
        revision: HostRevision::INITIAL,
        state: PackageHostState::Opening,
        packages: vec![],
        instances: vec![],
        diagnostics: vec![],
    };
    assert!(snapshot.packages.is_empty());

    let error = PackageErrorEnvelope::new(
        PackageErrorCode::ManifestMalformed,
        PackageErrorStage::Admission,
        false,
        RecoveryAction::CorrectPackageSource,
        teamarness_package_host::DiagnosticId::new("diag-1").unwrap(),
    );
    assert_eq!(error.namespace, "teamarness.package-host");
    assert_eq!(LifecycleState::Active.as_str(), "active");
    assert_eq!(MountState::Mounted.as_str(), "mounted");
    for state in [
        LifecycleState::Admitted,
        LifecycleState::Activating,
        LifecycleState::Active,
        LifecycleState::Suspending,
        LifecycleState::Suspended,
        LifecycleState::Recovering,
        LifecycleState::Failed,
        LifecycleState::CrashLoop,
        LifecycleState::Disposing,
        LifecycleState::Disposed,
    ] {
        match state {
            LifecycleState::Admitted
            | LifecycleState::Activating
            | LifecycleState::Active
            | LifecycleState::Suspending
            | LifecycleState::Suspended
            | LifecycleState::Recovering
            | LifecycleState::Failed
            | LifecycleState::CrashLoop
            | LifecycleState::Disposing
            | LifecycleState::Disposed => {}
        }
    }
    for state in [
        PackageHostState::Opening,
        PackageHostState::Running,
        PackageHostState::ShuttingDown,
        PackageHostState::Stopped,
        PackageHostState::Failed,
    ] {
        match state {
            PackageHostState::Opening
            | PackageHostState::Running
            | PackageHostState::ShuttingDown
            | PackageHostState::Stopped
            | PackageHostState::Failed => {}
        }
    }

    let codes = [
        PackageErrorCode::InvalidRegistration,
        PackageErrorCode::SourceNotFound,
        PackageErrorCode::ManifestMalformed,
        PackageErrorCode::ManifestUnsupported,
        PackageErrorCode::InvalidPackageIdentity,
        PackageErrorCode::IdentityMismatch,
        PackageErrorCode::DuplicateIdentity,
        PackageErrorCode::OverrideRequired,
        PackageErrorCode::TrustEvidenceMissing,
        PackageErrorCode::TargetUnsupported,
        PackageErrorCode::ArtifactMissing,
        PackageErrorCode::ArtifactChanged,
        PackageErrorCode::DataDirectoryUnavailable,
        PackageErrorCode::DataDirectoryPermission,
        PackageErrorCode::DataDirectoryUnrepresentable,
        PackageErrorCode::CapabilityUnavailable,
        PackageErrorCode::CapabilityVersionUnsupported,
        PackageErrorCode::CapabilityRequestInvalid,
        PackageErrorCode::SpawnFailed,
        PackageErrorCode::HandshakeTimeout,
        PackageErrorCode::HandshakeMismatch,
        PackageErrorCode::MalformedFrame,
        PackageErrorCode::FrameTooLarge,
        PackageErrorCode::ProtocolViolation,
        PackageErrorCode::Overloaded,
        PackageErrorCode::DeadlineExceeded,
        PackageErrorCode::Cancelled,
        PackageErrorCode::ProcessExited,
        PackageErrorCode::OutcomeUncertain,
        PackageErrorCode::OrphanUnresolved,
        PackageErrorCode::CrashLoop,
        PackageErrorCode::InvalidLifecycleState,
        PackageErrorCode::BackendError,
        PackageErrorCode::Disposed,
        PackageErrorCode::ShuttingDown,
        PackageErrorCode::UiEntrypointMissing,
        PackageErrorCode::UiContractViolation,
        PackageErrorCode::UiLoadFailed,
        PackageErrorCode::UiException,
        PackageErrorCode::StaleMount,
        PackageErrorCode::EventLagged,
    ];
    assert_eq!(codes.len(), 41);
    for code in codes {
        match code {
            PackageErrorCode::InvalidRegistration
            | PackageErrorCode::SourceNotFound
            | PackageErrorCode::ManifestMalformed
            | PackageErrorCode::ManifestUnsupported
            | PackageErrorCode::InvalidPackageIdentity
            | PackageErrorCode::IdentityMismatch
            | PackageErrorCode::DuplicateIdentity
            | PackageErrorCode::OverrideRequired
            | PackageErrorCode::TrustEvidenceMissing
            | PackageErrorCode::TargetUnsupported
            | PackageErrorCode::ArtifactMissing
            | PackageErrorCode::ArtifactChanged
            | PackageErrorCode::DataDirectoryUnavailable
            | PackageErrorCode::DataDirectoryPermission
            | PackageErrorCode::DataDirectoryUnrepresentable
            | PackageErrorCode::CapabilityUnavailable
            | PackageErrorCode::CapabilityVersionUnsupported
            | PackageErrorCode::CapabilityRequestInvalid
            | PackageErrorCode::SpawnFailed
            | PackageErrorCode::HandshakeTimeout
            | PackageErrorCode::HandshakeMismatch
            | PackageErrorCode::MalformedFrame
            | PackageErrorCode::FrameTooLarge
            | PackageErrorCode::ProtocolViolation
            | PackageErrorCode::Overloaded
            | PackageErrorCode::DeadlineExceeded
            | PackageErrorCode::Cancelled
            | PackageErrorCode::ProcessExited
            | PackageErrorCode::OutcomeUncertain
            | PackageErrorCode::OrphanUnresolved
            | PackageErrorCode::CrashLoop
            | PackageErrorCode::InvalidLifecycleState
            | PackageErrorCode::BackendError
            | PackageErrorCode::Disposed
            | PackageErrorCode::ShuttingDown
            | PackageErrorCode::UiEntrypointMissing
            | PackageErrorCode::UiContractViolation
            | PackageErrorCode::UiLoadFailed
            | PackageErrorCode::UiException
            | PackageErrorCode::StaleMount
            | PackageErrorCode::EventLagged => {}
        }
    }
}

#[test]
fn public_source_does_not_export_dependency_types() {
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    for forbidden in [
        "rusqlite::",
        "jsonschema::",
        "semver::",
        "pub tokio::",
        "pub nix::",
    ] {
        assert!(
            !source.contains(forbidden),
            "public source contains {forbidden}"
        );
    }

    assert!(!source.contains("pub process:"));
    assert!(!source.contains("pub child:"));
    assert!(!source.contains("pub admission_digest"));

    let token = UiResourceToken::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert_eq!(format!("{token:?}"), "UiResourceToken([redacted])");
}

#[test]
fn package_error_envelope_version_two_serializes_package_error_in_camel_case() {
    let mut envelope = PackageErrorEnvelope::new(
        PackageErrorCode::BackendError,
        PackageErrorStage::Ui,
        false,
        RecoveryAction::None,
        teamarness_package_host::DiagnosticId::new("diag-fixture").unwrap(),
    );
    assert_eq!(
        serde_json::to_value(&envelope).unwrap()["version"],
        serde_json::json!(2)
    );
    envelope.package_error = Some(teamarness_package_host::PackageOwnedError {
        code: "fixture.rejected".into(),
        retryable: false,
        details: serde_json::Map::new(),
    });
    let value = serde_json::to_value(&envelope).unwrap();
    assert_eq!(value["packageError"]["code"], "fixture.rejected");
    assert_eq!(value["packageError"]["retryable"], serde_json::json!(false));
    assert!(value["packageError"]["details"].is_object());
}
