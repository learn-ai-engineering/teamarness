use serde::Serialize;
use teamarness_package_host::{
    DiagnosticId, PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, PackageOwnedError,
    RecoveryAction,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageBridgeError {
    version: u16,
    namespace: String,
    code: String,
    stage: String,
    affected_package: Option<String>,
    affected_instance: Option<crate::package_dto::PackageInstanceKeyDto>,
    retryable: bool,
    recovery: String,
    diagnostic_id: String,
    /// The Package's own error, when the Package owns this failure.
    package_error: Option<PackageOwnedErrorDto>,
}

/// A Package-owned error as the Shell forwards it to that Package's frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageOwnedErrorDto {
    code: String,
    retryable: bool,
    details: serde_json::Map<String, serde_json::Value>,
}

impl From<PackageOwnedError> for PackageOwnedErrorDto {
    fn from(value: PackageOwnedError) -> Self {
        Self {
            code: value.code,
            retryable: value.retryable,
            details: value.details,
        }
    }
}

impl From<PackageErrorEnvelope> for PackageBridgeError {
    fn from(value: PackageErrorEnvelope) -> Self {
        Self {
            version: value.version,
            namespace: value.namespace,
            code: wire_name(value.code),
            stage: wire_name(value.stage),
            affected_package: value.affected_package.map(String::from),
            affected_instance: value.affected_instance.map(Into::into),
            retryable: value.retryable,
            recovery: wire_name(value.recovery),
            diagnostic_id: value.diagnostic_id.to_string(),
            package_error: value.package_error.map(Into::into),
        }
    }
}

pub fn invalid_input(code: PackageErrorCode, stage: PackageErrorStage) -> PackageBridgeError {
    PackageErrorEnvelope::new(
        code,
        stage,
        false,
        RecoveryAction::None,
        DiagnosticId::new("desktop-validation").expect("static diagnostic ID is valid"),
    )
    .into()
}

fn wire_name<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .expect("Package Host enum serializes")
        .as_str()
        .expect("Package Host enum serializes as text")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use teamarness_package_host::{PackageErrorCode, PackageErrorStage, RecoveryAction};

    #[test]
    fn maps_host_errors_without_internal_text() {
        let error = PackageErrorEnvelope::new(
            PackageErrorCode::StaleMount,
            PackageErrorStage::Ui,
            true,
            RecoveryAction::ReloadUi,
            DiagnosticId::new("diag-1").unwrap(),
        );
        assert_eq!(
            serde_json::to_value(PackageBridgeError::from(error)).unwrap(),
            serde_json::json!({
                "version": 2,
                "namespace": "teamarness.package-host",
                "code": "stale_mount",
                "stage": "ui",
                "affectedPackage": null,
                "affectedInstance": null,
                "retryable": true,
                "recovery": "reload_ui",
                "diagnosticId": "diag-1",
                "packageError": null
            })
        );
    }

    #[test]
    fn maps_package_owned_errors_from_the_golden_error_frame() {
        let golden =
            include_str!("../../../../contracts/package-process/v1/golden/protocol-error.jsonl");
        let line = golden.lines().nth(1).expect("the golden error frame");
        let frame: serde_json::Value = serde_json::from_str(line).unwrap();
        let frame = &frame["frame"];
        let mut envelope = PackageErrorEnvelope::new(
            PackageErrorCode::BackendError,
            PackageErrorStage::Ui,
            false,
            RecoveryAction::None,
            DiagnosticId::new("diag-fixture").unwrap(),
        );
        envelope.package_error = Some(PackageOwnedError {
            code: frame["code"].as_str().unwrap().to_owned(),
            retryable: frame["retryable"].as_bool().unwrap(),
            details: frame["details"].as_object().unwrap().clone(),
        });
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../contracts/fixtures/package-bridge.json"))
                .unwrap();
        assert_eq!(
            serde_json::to_value(PackageBridgeError::from(envelope)).unwrap(),
            fixture["packageErrorEnvelope"]
        );
    }
}
