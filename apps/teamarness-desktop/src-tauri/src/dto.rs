use serde::{Deserialize, Serialize};
use teamarness_core::{
    MemberSnapshot, MembershipRole, MembershipSnapshot, PublicIdentity, WorkspacePerson,
    WorkspaceSummary,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationDto {
    pub identity: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
    pub id: String,
    pub name: String,
    pub created_by: String,
    pub created_at_unix_ms: i64,
}

impl From<WorkspaceSummary> for WorkspaceDto {
    fn from(value: WorkspaceSummary) -> Self {
        Self {
            id: value.id().to_string(),
            name: value.name().to_owned(),
            created_by: value.created_by().to_string(),
            created_at_unix_ms: value.created_at_unix_ms(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberRoleDto {
    Member,
    Manager,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberDto {
    pub identity: String,
    pub role: MemberRoleDto,
}

impl From<&MemberSnapshot> for MemberDto {
    fn from(value: &MemberSnapshot) -> Self {
        Self {
            identity: value.identity().to_string(),
            role: match value.role() {
                MembershipRole::Member => MemberRoleDto::Member,
                MembershipRole::Manager => MemberRoleDto::Manager,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipDto {
    pub workspace_id: String,
    pub epoch: String,
    pub version: String,
    pub members: Vec<MemberDto>,
}

impl From<MembershipSnapshot> for MembershipDto {
    fn from(value: MembershipSnapshot) -> Self {
        Self {
            workspace_id: value.workspace_id().to_string(),
            epoch: value.epoch().value().to_string(),
            version: value.version().to_string(),
            members: value.members().iter().map(MemberDto::from).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePersonDto {
    pub identity: String,
    pub person_id: String,
    pub name: Option<String>,
    pub fingerprint: String,
}

impl WorkspacePersonDto {
    pub fn from_core(identity: PublicIdentity, person: WorkspacePerson) -> Self {
        Self {
            identity: identity.to_string(),
            person_id: person.id().to_string(),
            name: person.name().map(str::to_owned),
            fingerprint: person.fingerprint().to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonNameSnapshotDto {
    pub workspace_id: String,
    pub cursor: String,
    pub persons: Vec<WorkspacePersonDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStateDto {
    pub workspace: WorkspaceDto,
    pub membership: MembershipDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationDto {
    pub value: String,
    pub sensitivity: InvitationSensitivity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationSensitivity {
    BearerAdmission,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_owned_dtos_to_the_shared_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/fixtures/bridge-contract.json"
        ))
        .expect("fixture must be JSON");
        let workspace = WorkspaceDto {
            id: "00112233445566778899aabbccddeeff".into(),
            name: "Design team".into(),
            created_by: "a".repeat(64),
            created_at_unix_ms: 1_789_264_800_000,
        };
        let membership = MembershipDto {
            workspace_id: workspace.id.clone(),
            epoch: "2".into(),
            version: "b".repeat(64),
            members: vec![
                MemberDto {
                    identity: "1".repeat(64),
                    role: MemberRoleDto::Member,
                },
                MemberDto {
                    identity: "a".repeat(64),
                    role: MemberRoleDto::Manager,
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(workspace).unwrap(),
            fixture["workspace"]
        );
        assert_eq!(
            serde_json::to_value(membership).unwrap(),
            fixture["membership"]
        );
        assert_eq!(
            serde_json::to_value(InvitationDto {
                value: "teamarness-workspace-v1.00".into(),
                sensitivity: InvitationSensitivity::BearerAdmission,
            })
            .unwrap(),
            fixture["invitation"]
        );
        assert_eq!(
            serde_json::to_value(InstallationDto {
                identity: "a".repeat(64),
                display_name: "Local installation".into(),
            })
            .unwrap(),
            fixture["installation"]
        );
        assert_ne!(fixture["workspace"], json!({ "canonicalBytes": [] }));
    }
}
