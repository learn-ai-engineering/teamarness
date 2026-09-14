#![forbid(unsafe_code)]

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, params};
use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::{
    Backend, BackendError, BackendRequest, EventSubscription, Package, event_channel,
};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinHandle;

const BACKGROUND_INTERVAL: Duration = Duration::from_millis(100);
const SIGNING_PAYLOAD: &str = "cmVmZXJlbmNlLXBhY2thZ2UtYWN0aXZhdGlvbg";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateSnapshot {
    pub counter: i64,
    pub background_marker: i64,
}

#[derive(Clone)]
pub struct ReferenceState {
    connection: Arc<Mutex<Connection>>,
}

impl ReferenceState {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS reference_state (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               counter INTEGER NOT NULL,
               background_marker INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO reference_state(singleton, counter, background_marker)
             VALUES (1, 0, 0);",
        )?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn snapshot(&self) -> Result<StateSnapshot, rusqlite::Error> {
        self.connection
            .lock()
            .expect("reference state mutex poisoned")
            .query_row(
                "SELECT counter, background_marker FROM reference_state WHERE singleton = 1",
                [],
                |row| {
                    Ok(StateSnapshot {
                        counter: row.get(0)?,
                        background_marker: row.get(1)?,
                    })
                },
            )
    }

    pub fn increment_counter(&self) -> Result<StateSnapshot, rusqlite::Error> {
        self.connection
            .lock()
            .expect("reference state mutex poisoned")
            .execute(
                "UPDATE reference_state SET counter = counter + 1 WHERE singleton = ?1",
                params![1],
            )?;
        self.snapshot()
    }

    pub fn advance_background_marker(&self) -> Result<StateSnapshot, rusqlite::Error> {
        self.connection
            .lock()
            .expect("reference state mutex poisoned")
            .execute(
                "UPDATE reference_state SET background_marker = background_marker + 1 WHERE singleton = ?1",
                params![1],
            )?;
        self.snapshot()
    }

    #[must_use]
    pub fn start_background_activity(&self, interval: Duration) -> BackgroundActivity {
        let state = self.clone();
        let task = tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            timer.tick().await;
            loop {
                timer.tick().await;
                if state.advance_background_marker().is_err() {
                    return;
                }
            }
        });
        BackgroundActivity { task }
    }
}

pub struct BackgroundActivity {
    task: JoinHandle<()>,
}

impl BackgroundActivity {
    pub async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

#[derive(Clone)]
pub struct ReferenceBackend {
    active: Arc<AsyncMutex<Option<ActivePackage>>>,
    activity: Arc<AsyncMutex<Option<JoinHandle<()>>>>,
    changes: watch::Sender<u64>,
}

#[derive(Clone)]
struct ActivePackage {
    state: ReferenceState,
    package_version: String,
    process_generation: String,
    membership_version: String,
    signing_verified: bool,
}

impl Default for ReferenceBackend {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            active: Arc::new(AsyncMutex::new(None)),
            activity: Arc::new(AsyncMutex::new(None)),
            changes,
        }
    }
}

impl ReferenceBackend {
    pub async fn serve() -> Result<(), teamarness_package_sdk::SdkError> {
        Package::builder(Arc::new(Self::default())).serve().await
    }

    async fn activate(&self, request: &BackendRequest) -> Result<JsonValue, BackendError> {
        self.stop_activity().await;
        let workspace_grant = grant(&request.initialization, "teamarness.workspace-context")?;
        let membership_grant = grant(&request.initialization, "teamarness.membership")?;
        let signing_grant = grant(&request.initialization, "teamarness.signing")?;
        let workspace = request
            .capabilities
            .call(&workspace_grant, "get", empty_object())
            .await
            .map_err(capability_error)?;
        let membership = request
            .capabilities
            .call(&membership_grant, "current", empty_object())
            .await
            .map_err(capability_error)?;
        let signed = request
            .capabilities
            .call(
                &signing_grant,
                "sign",
                object([
                    ("purpose", JsonValue::String("reference-activation".into())),
                    ("payload", JsonValue::String(SIGNING_PAYLOAD.into())),
                ]),
            )
            .await
            .map_err(capability_error)?;
        let signer = string_value(&signed, "signer")?;
        let signature = string_value(&signed, "signature")?;
        let verified = request
            .capabilities
            .call(
                &signing_grant,
                "verify",
                object([
                    ("signer", JsonValue::String(signer)),
                    ("purpose", JsonValue::String("reference-activation".into())),
                    ("payload", JsonValue::String(SIGNING_PAYLOAD.into())),
                    ("signature", JsonValue::String(signature)),
                ]),
            )
            .await
            .map_err(capability_error)?;
        let signing_verified = verified
            .get("valid")
            .and_then(JsonValue::as_bool)
            .ok_or_else(invalid_capability_response)?;
        let membership_version = string_value(&membership, "membershipVersion")?;
        if workspace
            .get("workspaceId")
            .and_then(JsonValue::as_str)
            .is_none()
        {
            return Err(invalid_capability_response());
        }
        let data_path = request
            .initialization
            .data_directory()
            .get("path")
            .and_then(JsonValue::as_str)
            .ok_or_else(invalid_initialization)?;
        let package_version = request
            .initialization
            .package()
            .get("version")
            .and_then(JsonValue::as_str)
            .ok_or_else(invalid_initialization)?
            .to_owned();
        let state = ReferenceState::open(Path::new(data_path).join("state.sqlite3"))
            .map_err(storage_error)?;
        *self.active.lock().await = Some(ActivePackage {
            state: state.clone(),
            package_version,
            process_generation: request.initialization.process_generation().to_owned(),
            membership_version,
            signing_verified,
        });
        let changes = self.changes.clone();
        *self.activity.lock().await = Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(BACKGROUND_INTERVAL);
            timer.tick().await;
            loop {
                timer.tick().await;
                if state.advance_background_marker().is_err() {
                    return;
                }
                changes.send_modify(|revision| *revision = revision.saturating_add(1));
            }
        }));
        Ok(empty_object())
    }

    async fn stop_activity(&self) {
        if let Some(task) = self.activity.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
    }

    async fn read(&self) -> Result<JsonValue, BackendError> {
        let active = self.active.lock().await;
        let active = active.as_ref().ok_or_else(not_active)?;
        let state = active.state.snapshot().map_err(storage_error)?;
        Ok(snapshot_value(active, state))
    }

    async fn increment(&self) -> Result<JsonValue, BackendError> {
        let value = {
            let active = self.active.lock().await;
            let active = active.as_ref().ok_or_else(not_active)?;
            let state = active.state.increment_counter().map_err(storage_error)?;
            snapshot_value(active, state)
        };
        self.changes
            .send_modify(|revision| *revision = revision.saturating_add(1));
        Ok(value)
    }
}

impl Backend for ReferenceBackend {
    fn handle(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, BackendError>> + Send + '_>> {
        Box::pin(async move {
            match request.operation.as_str() {
                "lifecycle.activate" | "lifecycle.recover" => self.activate(&request).await,
                "lifecycle.suspend" => {
                    self.stop_activity().await;
                    Ok(empty_object())
                }
                "lifecycle.dispose" => {
                    self.stop_activity().await;
                    *self.active.lock().await = None;
                    Ok(empty_object())
                }
                "reference.read" => self.read().await,
                "reference.increment" => self.increment().await,
                _ => Err(BackendError::new(
                    "reference.unknown_operation",
                    false,
                    empty_object(),
                )),
            }
        })
    }

    fn subscribe(
        &self,
        request: BackendRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventSubscription, BackendError>> + Send + '_>> {
        Box::pin(async move {
            if request.operation != "reference.changes" {
                return Err(BackendError::new(
                    "reference.unknown_subscription",
                    false,
                    empty_object(),
                ));
            }
            let (sender, subscription) = event_channel(32);
            let backend = self.clone();
            let mut changes = self.changes.subscribe();
            tokio::spawn(async move {
                if let Ok(initial) = backend.read().await
                    && sender.send(initial).await.is_err()
                {
                    return;
                }
                while changes.changed().await.is_ok() {
                    let Ok(value) = backend.read().await else {
                        return;
                    };
                    if sender.send(value).await.is_err() {
                        return;
                    }
                }
            });
            Ok(subscription)
        })
    }
}

fn grant(
    initialization: &teamarness_package_sdk::Initialization,
    identity: &str,
) -> Result<String, BackendError> {
    initialization
        .capabilities()
        .as_array()
        .and_then(|values| {
            values.iter().find_map(|value| {
                (value.get("identity").and_then(JsonValue::as_str) == Some(identity))
                    .then(|| {
                        value
                            .get("grant")
                            .and_then(JsonValue::as_str)
                            .map(str::to_owned)
                    })
                    .flatten()
            })
        })
        .ok_or_else(invalid_initialization)
}

fn snapshot_value(active: &ActivePackage, state: StateSnapshot) -> JsonValue {
    object([
        ("counter", JsonValue::from(state.counter)),
        ("backgroundMarker", JsonValue::from(state.background_marker)),
        (
            "membershipVersion",
            JsonValue::String(active.membership_version.clone()),
        ),
        ("signingVerified", JsonValue::Bool(active.signing_verified)),
        (
            "processGeneration",
            JsonValue::String(active.process_generation.clone()),
        ),
        (
            "packageVersion",
            JsonValue::String(active.package_version.clone()),
        ),
    ])
}

fn string_value(value: &JsonValue, field: &str) -> Result<String, BackendError> {
    value
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(invalid_capability_response)
}

fn capability_error(_error: BackendError) -> BackendError {
    BackendError::new("reference.capability_failed", true, empty_object())
}

fn invalid_capability_response() -> BackendError {
    BackendError::new("reference.capability_invalid", false, empty_object())
}

fn invalid_initialization() -> BackendError {
    BackendError::new("reference.initialization_invalid", false, empty_object())
}

fn storage_error(_error: rusqlite::Error) -> BackendError {
    BackendError::new("reference.storage_failed", true, empty_object())
}

fn not_active() -> BackendError {
    BackendError::new("reference.not_active", true, empty_object())
}

fn empty_object() -> JsonValue {
    JsonValue::Object(Default::default())
}

fn object<const N: usize>(entries: [(&str, JsonValue); N]) -> JsonValue {
    let mut value = JsonValue::Object(Default::default());
    let fields = value.as_object_mut().expect("new object");
    for (name, entry) in entries {
        fields.insert(name.to_owned(), entry);
    }
    value
}
