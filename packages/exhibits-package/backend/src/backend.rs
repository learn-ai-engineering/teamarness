use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use teamarness_package_contract::media_types::media_type_for_path;
use teamarness_package_sdk::capabilities::content_views::{
    ContentFile, ContentSource, ContentViews,
};
use teamarness_package_sdk::{
    BackendError, Context, Empty, Router, event_channel, ports::PortActor,
};
use tokio::sync::broadcast;

use crate::domain::{ExhibitsError, LIST_PAGE, rfc3339, validate_chunk_header};
use crate::guide;
use crate::store::{
    Author, Clock, DraftUpdate, ExhibitStore, Missing, content_source_path, system_clock,
};

/// The Port Contract this Package provides.
pub const CONTRACT: &str = "dev.teamarness.exhibits";
/// How often garbage collection runs while the Backend runs, beside the pass at startup.
const GC_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Draft changes buffered per UI subscription. A subscriber that falls this far behind has its
/// subscription ended, and re-lists when it subscribes again.
const CHANGE_BUFFER: usize = 64;

/// One Draft change, as the `exhibits.changed` subscription delivers it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DraftChange {
    pub exhibit_id: String,
    /// The new revision, or 0 when the Draft was deleted.
    pub revision: u64,
    pub deleted: bool,
}

/// Everything one activation of Exhibits owns.
pub struct ExhibitsActive {
    store: ExhibitStore,
    views: ContentViews,
    installation: String,
    changes: broadcast::Sender<DraftChange>,
    collector: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PutRequest {
    sha256: String,
    size: u64,
    chunk_index: u64,
    chunk_count: u64,
    data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MissingRequest {
    sha256s: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateFile {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    exhibit_id: String,
    title: String,
    entry: String,
    files: Vec<UpdateFile>,
    #[serde(default)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListRequest {
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadRequest {
    exhibit_id: String,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileReadRequest {
    sha256: String,
    chunk_index: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteRequest {
    exhibit_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenRequest {
    exhibit_id: String,
}

/// The Exhibits Backend on the wall clock.
#[must_use]
pub fn router() -> Router<ExhibitsActive> {
    router_with_clock(system_clock())
}

/// The Exhibits Backend on `clock`, which a test controls.
#[must_use]
pub fn router_with_clock(clock: Clock) -> Router<ExhibitsActive> {
    Router::new(move |activation| {
        let clock = clock.clone();
        let initialization = activation.initialization().clone();
        let capabilities = activation.capabilities().clone();
        let cancellation = activation.cancellation().clone();
        async move {
            let views = ContentViews::from_initialization(&initialization, capabilities)
                .map_err(|cause| capability_error("content-views", &cause))?;
            let directory = initialization.data_directory().path().to_owned();
            let store = teamarness_package_sdk::blocking(&cancellation, move |_| {
                let store = ExhibitStore::open(directory, clock)?;
                store.collect_garbage()?;
                Ok(store)
            })
            .await?;
            let (changes, _) = broadcast::channel(CHANGE_BUFFER);
            let collector = tokio::spawn(collect_periodically(store.clone()));
            Ok(ExhibitsActive {
                store,
                views,
                installation: initialization.installation().identity.clone(),
                changes,
                collector: Mutex::new(Some(collector)),
            })
        }
    })
    .on_dispose(|state: Arc<ExhibitsActive>| async move {
        state.stop_collector();
        Ok(())
    })
    .port_request(
        CONTRACT,
        "runtime.guide",
        "query",
        |context: Context<ExhibitsActive>, actor: PortActor, _: Empty| async move {
            authorize(&context, &actor)?;
            Ok(guide::guide())
        },
    )
    .port_request(CONTRACT, "file.put", "command", file_put)
    .port_request(CONTRACT, "files.missing", "query", files_missing)
    .port_request(CONTRACT, "draft.update", "command", draft_update)
    .port_request(
        CONTRACT,
        "exhibit.list",
        "query",
        |context: Context<ExhibitsActive>, actor: PortActor, request: ListRequest| async move {
            authorize(&context, &actor)?;
            list(&context, request).await
        },
    )
    .port_request(CONTRACT, "exhibit.read", "query", exhibit_read)
    .port_request(CONTRACT, "file.read", "query", file_read)
    .port_request(CONTRACT, "draft.delete", "command", draft_delete)
    .request(
        "exhibit.list",
        |context: Context<ExhibitsActive>, request: ListRequest| async move {
            list(&context, request).await
        },
    )
    .request("exhibit.open", exhibit_open)
    .subscription(
        "exhibits.changed",
        |context: Context<ExhibitsActive>, _: Empty| async move {
            let (sender, subscription) = event_channel(CHANGE_BUFFER);
            let mut changes = context.state().changes.subscribe();
            tokio::spawn(async move {
                // A lagged subscriber is ended rather than silently skipped, so it re-lists.
                while let Ok(change) = changes.recv().await {
                    let Ok(event) = serde_json::to_value(&change) else {
                        return;
                    };
                    if sender.send(event).await.is_err() {
                        return;
                    }
                }
            });
            Ok(subscription)
        },
    )
}

impl ExhibitsActive {
    fn stop_collector(&self) {
        if let Some(collector) = self
            .collector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            collector.abort();
        }
    }

    fn changed(&self, exhibit_id: &str, revision: u64, deleted: bool) {
        // No subscriber is not an error: the UI re-lists when it subscribes.
        let _ = self.changes.send(DraftChange {
            exhibit_id: exhibit_id.to_owned(),
            revision,
            deleted,
        });
    }
}

impl Drop for ExhibitsActive {
    fn drop(&mut self) {
        self.stop_collector();
    }
}

async fn collect_periodically(store: ExhibitStore) {
    let mut interval = tokio::time::interval(GC_INTERVAL);
    // The first tick completes at once; the activation already collected.
    interval.tick().await;
    loop {
        interval.tick().await;
        let store = store.clone();
        let outcome = tokio::task::spawn_blocking(move || store.collect_garbage()).await;
        if !matches!(outcome, Ok(Ok(_))) {
            eprintln!("exhibits garbage collection failed; it runs again next interval");
        }
    }
}

fn capability_error(operation: &str, cause: &BackendError) -> BackendError {
    eprintln!(
        "exhibits capability failure: operation={operation} code={} retryable={}",
        cause.code(),
        cause.retryable()
    );
    ExhibitsError::ContentViewsUnavailable {
        cause: cause.code().to_owned(),
    }
    .into()
}

/// Both the local Installation Person and a locally bound Agent may call every Port. A Person
/// of another Installation is refused.
fn authorize(context: &Context<ExhibitsActive>, actor: &PortActor) -> Result<(), BackendError> {
    match actor {
        PortActor::Person { identity } if *identity == context.state().installation => Ok(()),
        PortActor::Agent { .. } => Ok(()),
        PortActor::Person { .. } => Err(ExhibitsError::ActorUnauthorized.into()),
    }
}

/// Runs store work off the async runtime.
async fn on_store<T, F>(context: &Context<ExhibitsActive>, work: F) -> Result<T, BackendError>
where
    F: FnOnce(&ExhibitStore) -> Result<T, ExhibitsError> + Send + 'static,
    T: Send + 'static,
{
    let store = context.state().store.clone();
    context
        .blocking(move |_| work(&store).map_err(BackendError::from))
        .await
}

async fn file_put(
    context: Context<ExhibitsActive>,
    actor: PortActor,
    request: PutRequest,
) -> Result<JsonValue, BackendError> {
    authorize(&context, &actor)?;
    // The header is checked before the data is decoded, so an oversized file is refused at its
    // first chunk without work.
    validate_chunk_header(
        &request.sha256,
        request.size,
        request.chunk_index,
        request.chunk_count,
    )?;
    let data = STANDARD
        .decode(request.data.as_bytes())
        .map_err(|_| ExhibitsError::InvalidChunk { reason: "base64" })?;
    let complete = on_store(&context, move |store| {
        store.put_chunk(
            &request.sha256,
            request.size,
            request.chunk_index,
            request.chunk_count,
            &data,
        )
    })
    .await?;
    Ok(json!({ "complete": complete }))
}

async fn files_missing(
    context: Context<ExhibitsActive>,
    actor: PortActor,
    request: MissingRequest,
) -> Result<JsonValue, BackendError> {
    authorize(&context, &actor)?;
    let missing = on_store(&context, move |store| store.missing(&request.sha256s)).await?;
    let missing: Vec<JsonValue> = missing
        .into_iter()
        .map(|(sha256, missing)| match missing {
            Missing::All => json!({ "sha256": sha256, "chunks": "all" }),
            Missing::Chunks(chunks) => json!({ "sha256": sha256, "chunks": chunks }),
        })
        .collect();
    Ok(json!({ "missing": missing }))
}

async fn draft_update(
    context: Context<ExhibitsActive>,
    actor: PortActor,
    request: UpdateRequest,
) -> Result<JsonValue, BackendError> {
    authorize(&context, &actor)?;
    let update = DraftUpdate {
        author: match actor {
            PortActor::Person { identity } => Author {
                kind: "person",
                identity,
            },
            PortActor::Agent { identity, .. } => Author {
                kind: "agent",
                identity,
            },
        },
        id: request.exhibit_id,
        title: request.title,
        entry: request.entry,
        files: request
            .files
            .into_iter()
            .map(|file| (file.path, file.sha256))
            .collect(),
        expected_revision: request.expected_revision,
    };
    let id = update.id.clone();
    let revision = on_store(&context, move |store| store.update_draft(&update)).await?;
    context.state().changed(&id, revision, false);
    Ok(json!({ "revision": revision }))
}

async fn list(
    context: &Context<ExhibitsActive>,
    request: ListRequest,
) -> Result<JsonValue, BackendError> {
    let limit = request.limit.unwrap_or(LIST_PAGE);
    if limit == 0 || limit > LIST_PAGE {
        return Err(ExhibitsError::InvalidRequest.into());
    }
    let page = on_store(context, move |store| {
        store.list(request.before.as_deref(), limit)
    })
    .await?;
    let exhibits: Vec<JsonValue> = page
        .drafts
        .into_iter()
        .map(|draft| {
            json!({
                "id": draft.id,
                "title": draft.title,
                "updatedAt": rfc3339(draft.updated_at),
                "kind": "draft",
                "revision": draft.revision,
            })
        })
        .collect();
    let mut answer = json!({ "exhibits": exhibits });
    if let Some(next) = page.next_before {
        answer["nextBefore"] = json!(next);
    }
    Ok(answer)
}

async fn exhibit_read(
    context: Context<ExhibitsActive>,
    actor: PortActor,
    request: ReadRequest,
) -> Result<JsonValue, BackendError> {
    authorize(&context, &actor)?;
    let id = request.exhibit_id.clone();
    let offset = request.offset.unwrap_or(0);
    let manifest = on_store(&context, move |store| store.read(&id, offset)).await?;
    let files: Vec<JsonValue> = manifest
        .files
        .into_iter()
        .map(|file| {
            json!({
                "path": file.path,
                "sha256": file.sha256,
                "size": file.size,
                "mediaType": media_type_for_path(&file.path).unwrap_or("application/octet-stream"),
            })
        })
        .collect();
    let mut answer = json!({
        "exhibitId": request.exhibit_id,
        "title": manifest.title,
        "entry": manifest.entry,
        "revision": manifest.revision,
        "updatedAt": rfc3339(manifest.updated_at),
        "updatedBy": { "kind": manifest.updated_by.kind, "identity": manifest.updated_by.identity },
        "fileCount": manifest.file_count,
        "files": files,
    });
    if let Some(next) = manifest.next_offset {
        answer["nextOffset"] = json!(next);
    }
    Ok(answer)
}

async fn file_read(
    context: Context<ExhibitsActive>,
    actor: PortActor,
    request: FileReadRequest,
) -> Result<JsonValue, BackendError> {
    authorize(&context, &actor)?;
    let (data, count) = on_store(&context, move |store| {
        store.read_chunk(&request.sha256, request.chunk_index)
    })
    .await?;
    Ok(json!({ "data": STANDARD.encode(data), "chunkCount": count }))
}

async fn draft_delete(
    context: Context<ExhibitsActive>,
    actor: PortActor,
    request: DeleteRequest,
) -> Result<JsonValue, BackendError> {
    authorize(&context, &actor)?;
    let id = request.exhibit_id.clone();
    let deleted = on_store(&context, move |store| {
        store.delete(&id, request.expected_revision)
    })
    .await?;
    if deleted {
        context.state().changed(&request.exhibit_id, 0, true);
    }
    Ok(json!({ "deleted": deleted }))
}

/// Opens a content view over the Draft's current files and returns its URL and revision. A
/// later update needs a new view: the Host refuses a file whose bytes changed after `open`.
async fn exhibit_open(
    context: Context<ExhibitsActive>,
    request: OpenRequest,
) -> Result<JsonValue, BackendError> {
    let id = request.exhibit_id;
    let (entry, revision, files) = on_store(&context, move |store| store.view_files(&id)).await?;
    let files: Vec<ContentFile> = files
        .into_iter()
        .map(|file| ContentFile {
            source: ContentSource::File {
                path: content_source_path(&file.sha256),
            },
            path: file.path,
            sha256: file.sha256,
            size: file.size,
        })
        .collect();
    let view = context
        .state()
        .views
        .open(&entry, &files)
        .await
        .map_err(|cause| capability_error("content-views.open", &cause))?;
    Ok(json!({ "url": view.url, "revision": revision }))
}

#[cfg(test)]
mod tests;
