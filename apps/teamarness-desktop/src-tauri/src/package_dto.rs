use serde::{Deserialize, Serialize};
use teamarness_package_host::{
    HostStateNotification, PackageCatalogEntry, PackageHostSnapshot, PackageInstanceKey,
    PackageInstanceSnapshot, PackageValue, UiEventNotification, UiLoaderDescriptor,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstanceKeyDto {
    pub package_identity: String,
    pub workspace_id: String,
}

impl From<PackageInstanceKey> for PackageInstanceKeyDto {
    fn from(value: PackageInstanceKey) -> Self {
        Self {
            package_identity: value.package_identity.into(),
            workspace_id: value.workspace_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageCatalogEntryDto {
    identity: String,
    version: String,
    display_name: String,
    source: String,
    trust: String,
    ui_contract_digest: String,
}

impl From<PackageCatalogEntry> for PackageCatalogEntryDto {
    fn from(value: PackageCatalogEntry) -> Self {
        Self {
            identity: value.identity.into(),
            version: value.version,
            display_name: value.display_name,
            source: wire_name(value.source),
            trust: wire_name(value.trust),
            ui_contract_digest: value.ui_contract_digest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstanceDto {
    key: PackageInstanceKeyDto,
    lifecycle: String,
    process_generation: Option<u64>,
    mount_state: Option<String>,
}

impl From<PackageInstanceSnapshot> for PackageInstanceDto {
    fn from(value: PackageInstanceSnapshot) -> Self {
        Self {
            key: value.key.into(),
            lifecycle: value.lifecycle.as_str().to_owned(),
            process_generation: value.process_generation,
            mount_state: value.mount_state.map(|state| state.as_str().to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageHostSnapshotDto {
    revision: u64,
    state: String,
    packages: Vec<PackageCatalogEntryDto>,
    instances: Vec<PackageInstanceDto>,
    diagnostics: Vec<crate::package_error::PackageBridgeError>,
}

impl From<PackageHostSnapshot> for PackageHostSnapshotDto {
    fn from(value: PackageHostSnapshot) -> Self {
        Self {
            revision: value.revision.0,
            state: wire_name(value.state),
            packages: value.packages.into_iter().map(Into::into).collect(),
            instances: value.instances.into_iter().map(Into::into).collect(),
            diagnostics: value.diagnostics.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiLoaderDto {
    pub token: String,
    pub entrypoint: String,
    pub ui_host_version: u16,
    pub contract_digest: String,
    pub package: UiPackageContextDto,
    pub workspace: UiWorkspaceContextDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPackageContextDto {
    identity: String,
    display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiWorkspaceContextDto {
    id: String,
    name: String,
}

impl From<UiLoaderDescriptor> for UiLoaderDto {
    fn from(value: UiLoaderDescriptor) -> Self {
        Self {
            token: value.token.into(),
            entrypoint: value.entrypoint,
            ui_host_version: value.ui_host_version,
            contract_digest: value.contract_digest,
            package: UiPackageContextDto {
                identity: value.package.identity.into(),
                display_name: value.package.display_name,
            },
            workspace: UiWorkspaceContextDto {
                id: value.workspace.id.into(),
                name: value.workspace.name,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenUiMountDto {
    pub mount: String,
    pub loader: UiLoaderDto,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiEventDto {
    pub mount: String,
    pub subscription: String,
    pub sequence: String,
    pub payload: PackageValue,
}

impl From<UiEventNotification> for UiEventDto {
    fn from(value: UiEventNotification) -> Self {
        Self {
            mount: value.mount.into(),
            subscription: value.subscription.into(),
            sequence: value.sequence,
            payload: value.payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStateNotificationDto {
    revision: u64,
    revision_gap: bool,
}

impl From<HostStateNotification> for HostStateNotificationDto {
    fn from(value: HostStateNotification) -> Self {
        Self {
            revision: value.revision.0,
            revision_gap: value.revision_gap,
        }
    }
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

    #[test]
    fn package_instance_input_uses_only_public_identity_values() {
        let dto: PackageInstanceKeyDto = serde_json::from_value(serde_json::json!({
            "packageIdentity": "com.example.fixture",
            "workspaceId": "00112233445566778899aabbccddeeff"
        }))
        .unwrap();
        assert_eq!(dto.package_identity, "com.example.fixture");
        let encoded = serde_json::to_string(&dto).unwrap();
        for forbidden in ["sourcePath", "cachePath", "dataPath", "processHandle"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn event_and_state_dtos_match_the_shared_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../contracts/fixtures/package-bridge.json"))
                .unwrap();
        let event = UiEventDto {
            mount: "mount-1".into(),
            subscription: "subscription-1".into(),
            sequence: "9007199254740993".into(),
            payload: PackageValue::Object(std::collections::BTreeMap::from([(
                "value".into(),
                PackageValue::Boolean(true),
            )])),
        };
        let state = HostStateNotificationDto {
            revision: 8,
            revision_gap: false,
        };
        assert_eq!(serde_json::to_value(event).unwrap(), fixture["event"]);
        assert_eq!(serde_json::to_value(state).unwrap(), fixture["hostState"]);
    }
}
