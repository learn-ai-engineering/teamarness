use serde::{Deserialize, Serialize};
use teamarness_package_host::{
    HostStateNotification, PackageCatalogEntry, PackageHostSnapshot, PackageInstanceKey,
    PackageInstanceSnapshot, PackageValue, UiEventNotification, UiLoaderDescriptor,
    UiStreamCompletedNotification,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
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
            description: value.description,
            author: value.author,
            homepage: value.homepage,
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
    version: String,
    display_name: String,
    source: String,
    trust: String,
    lifecycle: String,
    process_generation: u64,
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
                version: value.package.version,
                display_name: value.package.display_name,
                source: wire_name(value.package.source),
                trust: wire_name(value.package.trust),
                lifecycle: wire_name(value.package.lifecycle),
                process_generation: value.package.process_generation,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum UiStreamMessageDto {
    Event(UiEventDto),
    Failure(crate::package_error::PackageBridgeError),
    /// The Package ended the stream normally; the mount stays healthy.
    Completed(UiStreamCompletedDto),
}

/// A Package UI stream the Package ended normally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStreamCompletedDto {
    pub mount: String,
    pub subscription: String,
}

impl From<UiStreamCompletedNotification> for UiStreamCompletedDto {
    fn from(value: UiStreamCompletedNotification) -> Self {
        Self {
            mount: value.mount.into(),
            subscription: value.subscription.into(),
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

    // Verifies: TEAM.SYS.PKG-R01
    #[test]
    fn catalog_dto_carries_optional_presentation_metadata_in_camel_case() {
        let entry = |description: Option<&str>, author: Option<&str>, homepage: Option<&str>| {
            PackageCatalogEntryDto::from(PackageCatalogEntry {
                identity: "com.example.fixture".parse().unwrap(),
                version: "1.0.0".into(),
                display_name: "Fixture".into(),
                description: description.map(str::to_owned),
                author: author.map(str::to_owned),
                homepage: homepage.map(str::to_owned),
                source: teamarness_package_host::PackageSourceKind::Local,
                trust: teamarness_package_host::PackageTrustKind::TrustedNativeCodeV1,
                ui_contract_digest: "a".repeat(64),
            })
        };
        let full = serde_json::to_value(entry(
            Some("Describes the fixture."),
            Some("Example Author"),
            Some("https://example.com/fixture"),
        ))
        .unwrap();
        assert_eq!(full["displayName"], "Fixture");
        assert_eq!(full["description"], "Describes the fixture.");
        assert_eq!(full["author"], "Example Author");
        assert_eq!(full["homepage"], "https://example.com/fixture");

        let bare = serde_json::to_value(entry(None, None, None)).unwrap();
        for key in ["description", "author", "homepage"] {
            assert!(bare.get(key).is_none(), "{key} is omitted when absent");
        }
    }

    #[test]
    fn loader_dto_carries_generic_runtime_presentation() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../contracts/fixtures/package-bridge.json"))
                .unwrap();
        let descriptor = teamarness_package_host::UiLoaderDescriptor {
            token: teamarness_package_host::UiResourceToken::new(
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
            .unwrap(),
            entrypoint: "ui/index.js".into(),
            ui_host_version: 1,
            contract_digest: "a".repeat(64),
            package: teamarness_package_host::UiPackageContext {
                identity: "com.example.fixture".parse().unwrap(),
                version: "1.0.0".into(),
                display_name: "Fixture".into(),
                source: teamarness_package_host::PackageSourceKind::Bundled,
                trust: teamarness_package_host::PackageTrustKind::ApplicationBundled,
                lifecycle: teamarness_package_host::LifecycleState::Active,
                process_generation: 3,
            },
            workspace: teamarness_package_host::UiWorkspaceContext {
                id: teamarness_package_host::WorkspaceId::new("00112233445566778899aabbccddeeff")
                    .unwrap(),
                name: "Fixture Workspace".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(UiLoaderDto::from(descriptor)).unwrap(),
            fixture["loader"]
        );
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

    #[test]
    fn stream_completed_matches_the_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../contracts/fixtures/package-bridge.json"))
                .unwrap();
        let completed = UiStreamMessageDto::Completed(UiStreamCompletedDto {
            mount: "mount-1".into(),
            subscription: "subscription-1".into(),
        });
        assert_eq!(
            serde_json::to_value(completed).unwrap(),
            fixture["streamCompleted"]
        );
    }
}
