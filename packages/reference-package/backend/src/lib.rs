#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, params};
use teamarness_package_protocol::JsonValue;
use teamarness_package_sdk::capabilities::membership::Membership;
use teamarness_package_sdk::capabilities::signing::Signing;
use teamarness_package_sdk::capabilities::workspace_context::WorkspaceContext;
use teamarness_package_sdk::{BackendError, Context, Empty, Router, event_channel};
use tokio::sync::watch;
use tokio::task::JoinHandle;

const BACKGROUND_INTERVAL: Duration = Duration::from_millis(100);
const SIGNING_PURPOSE: &str = "reference-activation";
const SIGNING_PAYLOAD: &[u8] = b"reference-package-activation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateSnapshot {
    pub counter: i64,
    pub background_marker: i64,
}

/// Why the Reference state could not be read or written.
///
/// A poisoned connection is reported rather than panicked on: one failed request must not take
/// the whole Package Instance down with it.
#[derive(Debug)]
pub enum StateError {
    Database(rusqlite::Error),
    /// A previous caller panicked while holding the connection.
    Unavailable,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "database: {error}"),
            Self::Unavailable => formatter.write_str("state unavailable"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<rusqlite::Error> for StateError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

#[derive(Clone)]
pub struct ReferenceState {
    connection: Arc<Mutex<Connection>>,
}

impl ReferenceState {
    /// Opens or creates the Reference database.
    ///
    /// # Errors
    ///
    /// Returns the database failure.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StateError> {
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

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StateError> {
        self.connection.lock().map_err(|_| StateError::Unavailable)
    }

    /// Reads the counter and the background marker.
    ///
    /// # Errors
    ///
    /// Returns the database failure, or `Unavailable` when the connection is poisoned.
    pub fn snapshot(&self) -> Result<StateSnapshot, StateError> {
        Ok(self.connection()?.query_row(
            "SELECT counter, background_marker FROM reference_state WHERE singleton = 1",
            [],
            |row| {
                Ok(StateSnapshot {
                    counter: row.get(0)?,
                    background_marker: row.get(1)?,
                })
            },
        )?)
    }

    /// Advances the counter the UI shows.
    ///
    /// # Errors
    ///
    /// Returns the database failure, or `Unavailable` when the connection is poisoned.
    pub fn increment_counter(&self) -> Result<StateSnapshot, StateError> {
        self.connection()?.execute(
            "UPDATE reference_state SET counter = counter + 1 WHERE singleton = ?1",
            params![1],
        )?;
        self.snapshot()
    }

    /// Advances the marker that proves background work is running.
    ///
    /// # Errors
    ///
    /// Returns the database failure, or `Unavailable` when the connection is poisoned.
    pub fn advance_background_marker(&self) -> Result<StateSnapshot, StateError> {
        self.connection()?.execute(
            "UPDATE reference_state SET background_marker = background_marker + 1 WHERE singleton = ?1",
            params![1],
        )?;
        self.snapshot()
    }

    /// Starts a timer that advances the background marker, for tests of the state alone.
    #[must_use]
    pub fn start_background_activity(&self, interval: Duration) -> BackgroundActivity {
        BackgroundActivity {
            task: spawn_activity(self.clone(), interval, None),
        }
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

/// Advances the background marker on a timer, off the async runtime.
///
/// Each tick's database work runs on the blocking pool, so the Package's own workers stay free
/// even while SQLite is busy.
fn spawn_activity(
    state: ReferenceState,
    interval: Duration,
    changes: Option<watch::Sender<u64>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        timer.tick().await;
        loop {
            timer.tick().await;
            let worker = state.clone();
            match tokio::task::spawn_blocking(move || worker.advance_background_marker()).await {
                Ok(Ok(_)) => {}
                _ => return,
            }
            if let Some(changes) = &changes {
                changes.send_modify(|revision| *revision = revision.saturating_add(1));
            }
        }
    })
}

/// Everything one activation of the Reference Package owns.
pub struct ReferenceActive {
    state: ReferenceState,
    package_version: String,
    process_generation: String,
    membership_version: String,
    signing_verified: bool,
    activity: Mutex<Option<JoinHandle<()>>>,
    changes: watch::Sender<u64>,
}

impl ReferenceActive {
    /// Takes the running background task, if any, so the caller can stop it.
    fn take_activity(&self) -> Option<JoinHandle<()>> {
        self.activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn set_activity(&self, task: JoinHandle<()>) {
        *self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
    }

    fn snapshot_value(&self) -> Result<JsonValue, BackendError> {
        let state = self.state.snapshot().map_err(storage_error)?;
        Ok(object([
            ("counter", JsonValue::from(state.counter)),
            ("backgroundMarker", JsonValue::from(state.background_marker)),
            (
                "membershipVersion",
                JsonValue::String(self.membership_version.clone()),
            ),
            ("signingVerified", JsonValue::Bool(self.signing_verified)),
            (
                "processGeneration",
                JsonValue::String(self.process_generation.clone()),
            ),
            (
                "packageVersion",
                JsonValue::String(self.package_version.clone()),
            ),
        ]))
    }
}

async fn stop_activity(state: &ReferenceActive) {
    if let Some(task) = state.take_activity() {
        task.abort();
        let _ = task.await;
    }
}

/// The Reference Package Backend.
#[must_use]
pub fn router() -> Router<ReferenceActive> {
    Router::new(|activation| {
        let initialization = activation.initialization().clone();
        let capabilities = activation.capabilities().clone();
        let cancellation = activation.cancellation().clone();
        async move {
            let workspace =
                WorkspaceContext::from_initialization(&initialization, capabilities.clone())
                    .map_err(initialization_error)?;
            let membership = Membership::from_initialization(&initialization, capabilities.clone())
                .map_err(initialization_error)?;
            let signing = Signing::from_initialization(&initialization, capabilities)
                .map_err(initialization_error)?;

            let context = workspace.get().await.map_err(capability_error)?;
            if context.workspace_id.is_empty() {
                return Err(invalid_capability_response());
            }
            let roster = membership.current().await.map_err(capability_error)?;
            let signature = signing
                .sign(SIGNING_PURPOSE, SIGNING_PAYLOAD)
                .await
                .map_err(capability_error)?;
            let signing_verified = signing
                .verify(
                    &signature.signer,
                    SIGNING_PURPOSE,
                    SIGNING_PAYLOAD,
                    &signature.signature,
                )
                .await
                .map_err(capability_error)?;

            let database = initialization.data_directory().path().join("state.sqlite3");
            let state = teamarness_package_sdk::blocking(&cancellation, move |_| {
                ReferenceState::open(database).map_err(storage_error)
            })
            .await?;

            let (changes, _) = watch::channel(0);
            let active = ReferenceActive {
                state: state.clone(),
                package_version: initialization.package().version.clone(),
                process_generation: initialization.process_generation().to_owned(),
                membership_version: roster.membership_version,
                signing_verified,
                activity: Mutex::new(None),
                changes: changes.clone(),
            };
            active.set_activity(spawn_activity(state, BACKGROUND_INTERVAL, Some(changes)));
            Ok(active)
        }
    })
    .on_suspend(|state: Arc<ReferenceActive>| async move {
        stop_activity(&state).await;
        Ok(())
    })
    .on_resume(|state: Arc<ReferenceActive>| async move {
        stop_activity(&state).await;
        state.set_activity(spawn_activity(
            state.state.clone(),
            BACKGROUND_INTERVAL,
            Some(state.changes.clone()),
        ));
        Ok(())
    })
    .on_dispose(|state: Arc<ReferenceActive>| async move {
        stop_activity(&state).await;
        Ok(())
    })
    .request(
        "reference.read",
        |context: Context<ReferenceActive>, _: Empty| async move {
            let state = context.state().clone();
            context.blocking(move |_| state.snapshot_value()).await
        },
    )
    .request(
        "reference.increment",
        |context: Context<ReferenceActive>, _: Empty| async move {
            let state = context.state().clone();
            let value = context
                .blocking(move |_| {
                    state.state.increment_counter().map_err(storage_error)?;
                    state.snapshot_value()
                })
                .await?;
            context
                .state()
                .changes
                .send_modify(|revision| *revision = revision.saturating_add(1));
            Ok(value)
        },
    )
    // The live fixture for Package-owned error fidelity: the code, retryability and details
    // reach the Reference UI unchanged.
    .request(
        "reference.fail",
        |_context: Context<ReferenceActive>, _: Empty| async move {
            Err::<JsonValue, _>(BackendError::new(
                "reference.requested_failure",
                false,
                object([("reason", JsonValue::String("requested".into()))]),
            ))
        },
    )
    .subscription(
        "reference.changes",
        |context: Context<ReferenceActive>, _: Empty| async move {
            let (sender, subscription) = event_channel(32);
            let mut changes = context.state().changes.subscribe();
            tokio::spawn(async move {
                loop {
                    let state = context.state().clone();
                    let Ok(value) = context.blocking(move |_| state.snapshot_value()).await else {
                        return;
                    };
                    if sender.send(value).await.is_err() {
                        return;
                    }
                    if changes.changed().await.is_err() {
                        return;
                    }
                }
            });
            Ok(subscription)
        },
    )
}

fn capability_error(error: BackendError) -> BackendError {
    BackendError::new(
        "reference.capability_failed",
        true,
        object([("capabilityCode", JsonValue::String(error.code().to_owned()))]),
    )
}

fn invalid_capability_response() -> BackendError {
    BackendError::new("reference.capability_invalid", false, empty_object())
}

fn initialization_error(_error: BackendError) -> BackendError {
    BackendError::new("reference.initialization_invalid", false, empty_object())
}

fn storage_error(_error: StateError) -> BackendError {
    BackendError::new("reference.storage_failed", true, empty_object())
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
