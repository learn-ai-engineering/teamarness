//! Supported capability majors shared by Host negotiation and author diagnostics.
use crate::manifest::CapabilityRequirement;

/// One supported major and its inclusive minor range.
#[derive(Debug, Clone, Copy)]
pub struct CapabilityVersion {
    pub family: &'static str,
    pub major: u16,
    pub min_minor: u16,
    pub max_minor: u16,
    pub superseded: bool,
}

/// Simultaneously supported versions. Package Ports 2 has no removal scheduled here.
pub const SUPPORTED_VERSIONS: &[CapabilityVersion] = &[
    CapabilityVersion {
        family: "teamarness.workspace-context",
        major: 1,
        min_minor: 0,
        max_minor: 0,
        superseded: false,
    },
    CapabilityVersion {
        family: "teamarness.membership",
        major: 1,
        min_minor: 0,
        max_minor: 0,
        superseded: false,
    },
    CapabilityVersion {
        family: "teamarness.principals",
        major: 1,
        min_minor: 0,
        max_minor: 0,
        superseded: false,
    },
    CapabilityVersion {
        family: "teamarness.agent-binding",
        major: 1,
        min_minor: 0,
        max_minor: 0,
        superseded: false,
    },
    CapabilityVersion {
        family: "teamarness.signing",
        major: 1,
        min_minor: 0,
        max_minor: 0,
        superseded: false,
    },
    CapabilityVersion {
        family: "teamarness.package-ports",
        major: 2,
        min_minor: 0,
        max_minor: 0,
        superseded: true,
    },
    CapabilityVersion {
        family: "teamarness.package-ports",
        major: 3,
        min_minor: 0,
        max_minor: 0,
        superseded: false,
    },
    CapabilityVersion {
        family: "teamarness.content-views",
        major: 1,
        min_minor: 0,
        max_minor: 0,
        superseded: false,
    },
    CapabilityVersion {
        family: "teamarness.synchronization.immutable-record-log",
        major: 1,
        min_minor: 0,
        max_minor: 1,
        superseded: false,
    },
];

/// Nonfatal, read-only diagnostics; never included in capability grants.
#[must_use]
pub fn compatibility_warnings(requirements: &[CapabilityRequirement]) -> Vec<String> {
    requirements.iter().filter_map(|requirement| {
        SUPPORTED_VERSIONS.iter().find(|version| {
            version.family == requirement.family && version.major == requirement.major && version.superseded
        }).map(|_| format!("capability.superseded_major: {} major {} remains supported; a newer major is available", requirement.family, requirement.major))
    }).collect()
}
