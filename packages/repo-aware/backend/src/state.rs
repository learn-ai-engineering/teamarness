use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{self, ErrorInfo};

const STATE_FILE_NAME: &str = "repo-aware.v1.json";
const TEMP_FILE_NAME: &str = "repo-aware.v1.json.tmp";
const STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateDocument {
    schema_version: u32,
    registrations: Vec<StoredRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRegistration {
    id: String,
    canonical_path: String,
    added_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub id: String,
    pub path: String,
    pub added_at: String,
}

impl From<&StoredRegistration> for Registration {
    fn from(stored: &StoredRegistration) -> Self {
        Self {
            id: stored.id.clone(),
            path: stored.canonical_path.clone(),
            added_at: stored.added_at.clone(),
        }
    }
}

#[derive(Debug)]
pub struct StateStore {
    directory: PathBuf,
    state: StateDocument,
}

impl StateStore {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, ErrorInfo> {
        let directory = directory.as_ref().to_owned();
        std::fs::create_dir_all(&directory).map_err(|_| error::STORAGE_FAILED)?;
        let path = directory.join(STATE_FILE_NAME);
        let state = if path.exists() {
            let bytes = std::fs::read(&path).map_err(|_| error::STORAGE_FAILED)?;
            let state: StateDocument =
                serde_json::from_slice(&bytes).map_err(|_| error::STORAGE_FAILED)?;
            if state.schema_version != STATE_SCHEMA_VERSION {
                return Err(error::STORAGE_FAILED);
            }
            state
        } else {
            let state = StateDocument {
                schema_version: STATE_SCHEMA_VERSION,
                registrations: Vec::new(),
            };
            write_state(&directory, &state)?;
            state
        };
        Ok(Self { directory, state })
    }

    pub fn registrations(&self) -> Vec<Registration> {
        self.state
            .registrations
            .iter()
            .map(Registration::from)
            .collect()
    }

    pub fn register(&mut self, canonical_path: &str) -> Result<Registration, ErrorInfo> {
        if self
            .state
            .registrations
            .iter()
            .any(|r| r.canonical_path == canonical_path)
        {
            return Err(error::DUPLICATE_REGISTRATION);
        }
        let id = Uuid::new_v4().to_string();
        let added_at = Utc::now().to_rfc3339();
        let stored = StoredRegistration {
            id: id.clone(),
            canonical_path: canonical_path.to_owned(),
            added_at: added_at.clone(),
        };
        let mut next = StateDocument {
            schema_version: STATE_SCHEMA_VERSION,
            registrations: self.state.registrations.clone(),
        };
        next.registrations.push(stored);
        write_state(&self.directory, &next)?;
        self.state = next;
        Ok(Registration {
            id,
            path: canonical_path.to_owned(),
            added_at,
        })
    }

    pub fn remove(&mut self, id: &str) -> Result<(), ErrorInfo> {
        let index = self
            .state
            .registrations
            .iter()
            .position(|r| r.id == id)
            .ok_or(error::REGISTRATION_NOT_FOUND)?;
        let mut next = self.state.clone();
        next.registrations.remove(index);
        write_state(&self.directory, &next)?;
        self.state = next;
        Ok(())
    }
}

fn write_state(directory: &Path, state: &StateDocument) -> Result<(), ErrorInfo> {
    let temporary = directory.join(TEMP_FILE_NAME);
    let destination = directory.join(STATE_FILE_NAME);
    let bytes = serde_json::to_vec_pretty(state).map_err(|_| error::STORAGE_FAILED)?;
    std::fs::write(&temporary, bytes).map_err(|_| error::STORAGE_FAILED)?;
    if let Err(_write_error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error::STORAGE_FAILED);
    }
    Ok(())
}
