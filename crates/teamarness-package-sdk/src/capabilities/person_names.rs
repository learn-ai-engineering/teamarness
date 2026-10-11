//! Current Workspace Person names, independently of membership authority.

use teamarness_package_protocol::JsonValue;

use super::{
    decimal_field, field, grant_for, invalid_request, invalid_response, is_lower_hex, string_field,
};
use crate::capability::CapabilitySubscription;
use crate::{BackendError, CapabilityClient, Initialization, object};

pub const IDENTITY: &str = "teamarness.person-names";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonName {
    pub identity: String,
    pub person_id: String,
    pub name: Option<String>,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonNameSnapshot {
    pub workspace_id: String,
    /// Durable profile invalidation cursor; not a membership epoch or record cursor.
    pub cursor: u64,
    pub persons: Vec<PersonName>,
}

fn decode_person(value: &JsonValue) -> Result<PersonName, BackendError> {
    let identity = string_field(value, "identity")?;
    let person_id = string_field(value, "personId")?;
    let fingerprint = string_field(value, "fingerprint")?;
    if !is_lower_hex(&identity, 64)
        || !is_lower_hex(&person_id, 64)
        || fingerprint.len() > 64
        || fingerprint.is_empty()
    {
        return Err(invalid_response());
    }
    let name = match field(value, "name")? {
        JsonValue::Null => None,
        JsonValue::String(name) if !name.trim().is_empty() && name.len() <= 256 => {
            Some(name.clone())
        }
        _ => return Err(invalid_response()),
    };
    Ok(PersonName {
        identity,
        person_id,
        name,
        fingerprint,
    })
}

fn decode_snapshot(value: &JsonValue) -> Result<PersonNameSnapshot, BackendError> {
    let workspace_id = string_field(value, "workspaceId")?;
    if !is_lower_hex(&workspace_id, 32) {
        return Err(invalid_response());
    }
    let persons = field(value, "persons")?
        .as_array()
        .ok_or_else(invalid_response)?
        .iter()
        .map(decode_person)
        .collect::<Result<Vec<_>, _>>()?;
    if persons.len() > 1024 {
        return Err(invalid_response());
    }
    Ok(PersonNameSnapshot {
        workspace_id,
        cursor: decimal_field(value, "cursor")?,
        persons,
    })
}

#[derive(Clone)]
pub struct PersonNames {
    client: CapabilityClient,
    grant: String,
}

impl PersonNames {
    /// # Errors
    /// Returns `package.capability_not_granted` if the manifest does not require names.
    pub fn from_initialization(
        initialization: &Initialization,
        client: CapabilityClient,
    ) -> Result<Self, BackendError> {
        Ok(Self {
            grant: grant_for(initialization, IDENTITY)?,
            client,
        })
    }

    /// # Errors
    /// Returns a Host error or an invalid-response error for a malformed snapshot.
    pub async fn current(&self) -> Result<PersonNameSnapshot, BackendError> {
        decode_snapshot(
            &self
                .client
                .call(&self.grant, "current", super::empty())
                .await?,
        )
    }

    /// A nonmember has no current Person; missing name for a member is not an error.
    /// # Errors
    /// Returns an invalid-request error for an invalid Installation identity.
    pub async fn lookup(&self, identity: &str) -> Result<Option<PersonName>, BackendError> {
        if !is_lower_hex(identity, 64) {
            return Err(invalid_request());
        }
        let result = self
            .client
            .call(
                &self.grant,
                "lookup",
                object([("identity", JsonValue::String(identity.into()))]),
            )
            .await?;
        match field(&result, "person")? {
            JsonValue::Null => Ok(None),
            value => decode_person(value).map(Some),
        }
    }

    /// Resumes after a durable profile cursor; each notification carries a fresh snapshot.
    /// # Errors
    /// Returns a Host stream error when subscription cannot be opened.
    pub async fn subscribe(
        &self,
        after_cursor: u64,
    ) -> Result<PersonNameSubscription, BackendError> {
        let inner = self
            .client
            .subscribe(
                &self.grant,
                object([
                    ("method", JsonValue::String("subscribe".into())),
                    ("afterCursor", JsonValue::String(after_cursor.to_string())),
                ]),
            )
            .await?;
        Ok(PersonNameSubscription { inner })
    }
}

pub struct PersonNameSubscription {
    inner: CapabilitySubscription,
}

impl PersonNameSubscription {
    /// The next coalesced snapshot, or `None` after closure.
    pub async fn next(&mut self) -> Option<Result<PersonNameSnapshot, BackendError>> {
        self.inner
            .recv()
            .await
            .map(|value| value.and_then(|value| decode_snapshot(&value)))
    }

    pub async fn close(&mut self) {
        self.inner.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn independent_client_bounds_and_missing_names() {
        let identity = "a".repeat(64);
        let person_id = "b".repeat(64);
        let snapshot = json!({"workspaceId":"c".repeat(32), "cursor":"3", "persons":[
            {"identity":identity,"personId":person_id,"name":null,"fingerprint":"abcd1234"}
        ]});
        let parsed = decode_snapshot(&snapshot).unwrap();
        assert_eq!(parsed.cursor, 3);
        assert_eq!(parsed.persons[0].name, None);
        let mut invalid = snapshot.clone();
        invalid["persons"][0]["name"] = json!("x".repeat(257));
        assert!(decode_snapshot(&invalid).is_err());
        invalid["persons"][0]["name"] = json!("👩‍💻");
        assert_eq!(
            decode_snapshot(&invalid).unwrap().persons[0]
                .name
                .as_deref(),
            Some("👩‍💻")
        );
        invalid["cursor"] = json!("00");
        assert!(decode_snapshot(&invalid).is_err());
        invalid["cursor"] = json!("3");
        invalid["persons"][0]["identity"] = json!("not-an-installation");
        assert!(decode_snapshot(&invalid).is_err());
    }

    #[test]
    fn old_manifest_cannot_bind_the_additive_name_client() {
        let initialization: Initialization = serde_json::from_value(json!({
            "package":{"identity":"dev.example.fixture","version":"1.0.0","manifestDigest":"a","backendDigest":"b"},
            "workspace":{"id":"c","name":"Fixture"},
            "installation":{"identity":"d","displayName":"Local"},
            "dataDirectory":{"path":"/tmp/fixture","continuityId":"opaque"},
            "processGeneration":"1", "capabilities":[]
        })).unwrap();
        assert!(grant_for(&initialization, IDENTITY).is_err());
    }
}
