use serde::Serialize;
use teamarness_package_host::{
    DiagnosticId, PackageErrorCode, PackageErrorEnvelope, PackageErrorStage, RecoveryAction,
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
                "version": 1,
                "namespace": "teamarness.package-host",
                "code": "stale_mount",
                "stage": "ui",
                "affectedPackage": null,
                "affectedInstance": null,
                "retryable": true,
                "recovery": "reload_ui",
                "diagnosticId": "diag-1"
            })
        );
    }
}
