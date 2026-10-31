use serde::{Deserialize, Serialize};
use teamarness_package_host::{
    AuthoringCatalogEntry, HostRevision, HostStateNotification, InspectedPackage,
    InstallationRegistrationSnapshot, PackageCatalogEntry, PackageHostSnapshot, PackageInstanceKey,
    PackageInstanceSnapshot, PackageValue, RegistrationStatus, UiEventNotification,
    UiLoaderDescriptor, UiStreamCompletedNotification,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    views: Vec<String>,
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
            views: value.views.into_iter().map(wire_name).collect(),
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

/// One installation registration, with its admission tier and result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRegistrationDto {
    directory: String,
    expected_identity: String,
    #[serde(rename = "override")]
    override_application: bool,
    tier: String,
    status: PackageRegistrationStatusDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageRegistrationStatusDto {
    Admitted,
    Refused { code: String },
    PendingRestart { change: String },
}

impl From<InstallationRegistrationSnapshot> for PackageRegistrationDto {
    fn from(value: InstallationRegistrationSnapshot) -> Self {
        Self {
            directory: value.directory,
            expected_identity: value.expected_identity,
            override_application: value.override_application,
            tier: wire_name(value.tier),
            status: match value.status {
                RegistrationStatus::Admitted => PackageRegistrationStatusDto::Admitted,
                RegistrationStatus::Refused { code } => PackageRegistrationStatusDto::Refused {
                    code: wire_name(code),
                },
                RegistrationStatus::PendingRestart { change } => {
                    PackageRegistrationStatusDto::PendingRestart {
                        change: wire_name(change),
                    }
                }
            },
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
    registrations: Vec<PackageRegistrationDto>,
}

impl From<PackageHostSnapshot> for PackageHostSnapshotDto {
    fn from(value: PackageHostSnapshot) -> Self {
        Self {
            revision: value.revision.0,
            state: wire_name(value.state),
            packages: value.packages.into_iter().map(Into::into).collect(),
            instances: value.instances.into_iter().map(Into::into).collect(),
            diagnostics: value.diagnostics.into_iter().map(Into::into).collect(),
            registrations: value.registrations.into_iter().map(Into::into).collect(),
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
    pub view: String,
    pub nested_content: bool,
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
            view: wire_name(value.view),
            nested_content: value.nested_content,
        }
    }
}

/// A Package Directory read for the install preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedPackageDto {
    directory: String,
    identity: String,
    version: String,
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
    capabilities: Vec<InspectedCapabilityDto>,
    provided: Vec<ProvidedPortContractDto>,
    consumed: Vec<ConsumedPortContractDto>,
    /// `new`, `reload`, `replaces_registration`, or `overrides_team`.
    kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedCapabilityDto {
    identity: String,
    major: u16,
    min_minor: u16,
    max_minor: u16,
}

/// A Port contract the Package provides, without its Port schemas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidedPortContractDto {
    identity: String,
    major: u16,
    minor: u16,
    description: String,
}

/// A Port contract the Package consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumedPortContractDto {
    identity: String,
    major: u16,
    min_minor: u16,
    max_minor: u16,
    required: bool,
}

impl From<InspectedPackage> for InspectedPackageDto {
    fn from(value: InspectedPackage) -> Self {
        Self {
            directory: value.directory.to_string_lossy().into_owned(),
            identity: value.identity.into(),
            version: value.version,
            display_name: value.presentation.display_name,
            description: value.presentation.description,
            author: value.presentation.author,
            homepage: value.presentation.homepage,
            capabilities: value
                .capabilities
                .into_iter()
                .map(|capability| InspectedCapabilityDto {
                    identity: capability.identity,
                    major: capability.major,
                    min_minor: capability.min_minor,
                    max_minor: capability.max_minor,
                })
                .collect(),
            provided: value
                .provided
                .into_iter()
                .map(|contract| ProvidedPortContractDto {
                    identity: contract.identity,
                    major: contract.major,
                    minor: contract.minor,
                    description: contract.description,
                })
                .collect(),
            consumed: value
                .consumed
                .into_iter()
                .map(|contract| ConsumedPortContractDto {
                    identity: contract.identity,
                    major: contract.major,
                    min_minor: contract.min_minor,
                    max_minor: contract.max_minor,
                    required: contract.required,
                })
                .collect(),
            kind: wire_name(value.kind),
        }
    }
}

/// One admitted Package's declared Semantic Capabilities and Ports, for its
/// Package page. Port schemas are left out; the page shows only what a person reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageAuthoringEntryDto {
    identity: String,
    capabilities: Vec<InspectedCapabilityDto>,
    provided: Vec<ProvidedPortContractDto>,
    consumed: Vec<ConsumedPortAvailabilityDto>,
}

/// A consumed Port contract and whether the installed graph can route it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumedPortAvailabilityDto {
    identity: String,
    major: u16,
    min_minor: u16,
    max_minor: u16,
    required: bool,
    /// The Package that provides it, when one is selected.
    provider: Option<String>,
    /// The selected minor version, when one is selected.
    minor: Option<u16>,
    /// Why it cannot be routed, as a Package Host error code.
    unavailable: Option<String>,
}

impl From<AuthoringCatalogEntry> for PackageAuthoringEntryDto {
    fn from(value: AuthoringCatalogEntry) -> Self {
        let availability = value.availability;
        Self {
            identity: value.package.identity.into(),
            capabilities: value
                .capabilities
                .into_iter()
                .map(|capability| InspectedCapabilityDto {
                    identity: capability.identity,
                    major: capability.major,
                    min_minor: capability.min_minor,
                    max_minor: capability.max_minor,
                })
                .collect(),
            provided: value
                .provided
                .into_iter()
                .map(|contract| ProvidedPortContractDto {
                    identity: contract.identity,
                    major: contract.major,
                    minor: contract.minor,
                    description: contract.description,
                })
                .collect(),
            consumed: value
                .consumed
                .into_iter()
                .map(|contract| {
                    let routed = availability
                        .iter()
                        .find(|availability| availability.contract == contract.identity);
                    ConsumedPortAvailabilityDto {
                        provider: routed
                            .and_then(|routed| routed.provider.clone())
                            .map(Into::into),
                        minor: routed.and_then(|routed| routed.minor),
                        unavailable: routed.and_then(|routed| routed.unavailable).map(wire_name),
                        identity: contract.identity,
                        major: contract.major,
                        min_minor: contract.min_minor,
                        max_minor: contract.max_minor,
                        required: contract.required,
                    }
                })
                .collect(),
        }
    }
}

/// The result of an install, reload, remove, or override write.
///
/// A live change carries the Host revision that shows it. A change to an
/// override has no revision: it applies only after Teamarness restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageChangeDto {
    revision: Option<u64>,
    restart_required: bool,
}

impl PackageChangeDto {
    pub fn live(revision: HostRevision) -> Self {
        Self {
            revision: Some(revision.0),
            restart_required: false,
        }
    }

    pub fn after_restart() -> Self {
        Self {
            revision: None,
            restart_required: true,
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

    // Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
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
                views: vec![],
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

    // Verifies: TEAM.SYS.PKG-R01, TEAM.SYS.PKG-R19
    #[test]
    fn catalog_dto_lists_declared_views_and_omits_none() {
        let entry = |views| {
            serde_json::to_value(PackageCatalogEntryDto::from(PackageCatalogEntry {
                identity: "com.example.fixture".parse().unwrap(),
                version: "1.0.0".into(),
                display_name: "Fixture".into(),
                description: None,
                author: None,
                homepage: None,
                source: teamarness_package_host::PackageSourceKind::Bundled,
                trust: teamarness_package_host::PackageTrustKind::ApplicationBundled,
                ui_contract_digest: "a".repeat(64),
                views,
            }))
            .unwrap()
        };
        assert_eq!(
            entry(vec![teamarness_package_host::PackageUiView::Settings])["views"],
            serde_json::json!(["settings"])
        );
        assert!(entry(vec![]).get("views").is_none());
    }

    // Verifies: TEAM.SYS.PKG-R09
    #[test]
    fn registration_dto_carries_tier_and_status_in_camel_case() {
        let registration = |tier, status| {
            serde_json::to_value(PackageRegistrationDto::from(
                InstallationRegistrationSnapshot {
                    directory: "/Users/person/Packages/fixture".into(),
                    expected_identity: "com.example.fixture".into(),
                    override_application: tier == teamarness_package_host::PackageTier::Team,
                    tier,
                    status,
                },
            ))
            .unwrap()
        };
        assert_eq!(
            registration(
                teamarness_package_host::PackageTier::Personal,
                RegistrationStatus::Admitted
            ),
            serde_json::json!({
                "directory": "/Users/person/Packages/fixture",
                "expectedIdentity": "com.example.fixture",
                "override": false,
                "tier": "personal",
                "status": {"kind": "admitted"}
            })
        );
        assert_eq!(
            registration(
                teamarness_package_host::PackageTier::Team,
                RegistrationStatus::Refused {
                    code: teamarness_package_host::PackageErrorCode::PortProviderConflict
                }
            )["status"],
            serde_json::json!({"kind": "refused", "code": "port_provider_conflict"})
        );
        for (change, wire) in [
            (
                teamarness_package_host::PendingRegistrationChange::Written,
                "written",
            ),
            (
                teamarness_package_host::PendingRegistrationChange::Removed,
                "removed",
            ),
        ] {
            assert_eq!(
                registration(
                    teamarness_package_host::PackageTier::Team,
                    RegistrationStatus::PendingRestart { change }
                )["status"],
                serde_json::json!({"kind": "pending_restart", "change": wire})
            );
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
            ui_host_version: 2,
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
            view: teamarness_package_host::PackageUiView::Settings,
            nested_content: false,
        };
        assert_eq!(
            serde_json::to_value(UiLoaderDto::from(descriptor.clone())).unwrap(),
            fixture["loader"]
        );
        let nested = teamarness_package_host::UiLoaderDescriptor {
            nested_content: true,
            ..descriptor
        };
        assert_eq!(
            serde_json::to_value(UiLoaderDto::from(nested)).unwrap()["nestedContent"],
            serde_json::Value::Bool(true)
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

    fn bridge_fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../../contracts/fixtures/package-bridge.json")).unwrap()
    }

    // Verifies: TEAM.SYS.PKG-R09
    #[test]
    fn snapshot_with_registrations_round_trips_the_shared_fixture() {
        let fixture = bridge_fixture();
        let snapshot: PackageHostSnapshot =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(snapshot.registrations.len(), 4);
        assert_eq!(
            serde_json::to_value(PackageHostSnapshotDto::from(snapshot)).unwrap(),
            fixture["snapshot"]
        );
    }

    #[test]
    fn open_mount_for_the_main_view_with_nested_content_matches_the_fixture() {
        let fixture = bridge_fixture();
        let open = &fixture["openMount"];
        let descriptor: UiLoaderDescriptor =
            serde_json::from_value(open["loader"].clone()).unwrap();
        assert_eq!(
            descriptor.view,
            teamarness_package_host::PackageUiView::Main
        );
        assert!(descriptor.nested_content);
        let dto = OpenUiMountDto {
            mount: open["mount"].as_str().unwrap().to_owned(),
            loader: descriptor.into(),
        };
        assert_eq!(serde_json::to_value(dto).unwrap(), *open);

        let settings: UiLoaderDescriptor =
            serde_json::from_value(fixture["loader"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(UiLoaderDto::from(settings)).unwrap(),
            fixture["loader"]
        );
    }

    #[test]
    fn inspected_package_matches_the_shared_fixture() {
        let fixture = bridge_fixture();
        let sample = &fixture["inspectedPackage"];
        let without_ports = |value: &serde_json::Value| {
            let mut value = value.clone();
            value["ports"] = serde_json::json!([]);
            value
        };
        let inspected = InspectedPackage {
            directory: sample["directory"].as_str().unwrap().into(),
            identity: sample["identity"].as_str().unwrap().parse().unwrap(),
            version: sample["version"].as_str().unwrap().into(),
            presentation: serde_json::from_value(sample.clone()).unwrap(),
            capabilities: serde_json::from_value(sample["capabilities"].clone()).unwrap(),
            provided: sample["provided"]
                .as_array()
                .unwrap()
                .iter()
                .map(|contract| serde_json::from_value(without_ports(contract)).unwrap())
                .collect(),
            consumed: sample["consumed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|contract| serde_json::from_value(without_ports(contract)).unwrap())
                .collect(),
            kind: serde_json::from_value(sample["kind"].clone()).unwrap(),
        };
        assert_eq!(
            serde_json::to_value(InspectedPackageDto::from(inspected)).unwrap(),
            *sample
        );
    }

    #[test]
    fn authoring_catalog_entry_matches_the_shared_fixture() {
        let fixture = bridge_fixture();
        let sample = &fixture["authoringCatalog"][0];
        let contract_fields = |contract: &serde_json::Value| {
            serde_json::json!({
                "identity": contract["identity"],
                "major": contract["major"],
                "minMinor": contract["minMinor"],
                "maxMinor": contract["maxMinor"],
                "required": contract["required"],
                "ports": [],
            })
        };
        let consumed = sample["consumed"].as_array().unwrap();
        let entry = AuthoringCatalogEntry {
            package: PackageCatalogEntry {
                identity: sample["identity"].as_str().unwrap().parse().unwrap(),
                version: "0.2.0".into(),
                display_name: "Notes".into(),
                description: None,
                author: None,
                homepage: None,
                source: teamarness_package_host::PackageSourceKind::Local,
                trust: teamarness_package_host::PackageTrustKind::TrustedNativeCodeV1,
                ui_contract_digest: "a".repeat(64),
                views: vec![],
            },
            capabilities: serde_json::from_value(sample["capabilities"].clone()).unwrap(),
            provided: sample["provided"]
                .as_array()
                .unwrap()
                .iter()
                .map(|contract| {
                    let mut contract = contract.clone();
                    contract["ports"] = serde_json::json!([]);
                    serde_json::from_value(contract).unwrap()
                })
                .collect(),
            definitions: serde_json::Map::new(),
            consumed: consumed
                .iter()
                .map(|contract| serde_json::from_value(contract_fields(contract)).unwrap())
                .collect(),
            availability: consumed
                .iter()
                .map(|contract| teamarness_package_host::PortAvailability {
                    contract: contract["identity"].as_str().unwrap().into(),
                    provider: contract["provider"]
                        .as_str()
                        .map(|provider| provider.parse().unwrap()),
                    minor: contract["minor"]
                        .as_u64()
                        .map(|minor| u16::try_from(minor).unwrap()),
                    unavailable: serde_json::from_value(contract["unavailable"].clone()).unwrap(),
                })
                .collect(),
            diagnostics: vec![],
        };
        assert_eq!(
            serde_json::to_value(PackageAuthoringEntryDto::from(entry)).unwrap(),
            *sample
        );
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
