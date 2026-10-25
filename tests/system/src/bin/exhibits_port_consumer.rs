//! Test-only Exhibits Port Consumer. It never ships.
//!
//! It stands in for an Agent's Package: it uploads embedded fixtures through the real
//! `dev.teamarness.exhibits` Ports, as the Installation Person when unbound or as the Workspace
//! Agent it is bound to. Its UI operations (`tests/system/exhibits-consumer/ui-contract.json`)
//! are the seam a test, a packaged smoke or a manual check drives it through:
//!
//! - `fixture.publish {fixture, exhibitId?, title?}` uploads one fixture the way an Agent should
//!   (`files.missing`, then only the missing chunks, then `draft.update` with the revision it
//!   read) and returns `{exhibitId, revision, chunksSent}`.
//! - `agent.bind` binds this Instance to the first Workspace Agent this Installation stewards;
//!   `agent.unbind` clears it. The Host then derives the Port actor from the binding.
//! - `proof.start {prefix}` runs the system Port proof in the background, and `proof.result`
//!   reports `idle`, `running` or `done` with the proof's report.
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use teamarness_package_sdk::capabilities::agent_binding::AgentBinding;
use teamarness_package_sdk::{BackendError, Context, Empty, Router, ports::PortClient};
use teamarness_system_tests::exhibit_fixtures::{self, FixtureFile};

const CONTRACT: &str = "dev.teamarness.exhibits";
const CHUNK: usize = 131_072;

struct Consumer {
    ports: PortClient,
    binding: AgentBinding,
    proof: Arc<Mutex<ProofState>>,
}

enum ProofState {
    Idle,
    Running,
    Done(Value),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishRequest {
    fixture: String,
    #[serde(default)]
    exhibit_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProofRequest {
    prefix: String,
}

fn failure(code: &str, details: Value) -> BackendError {
    BackendError::new(code, false, details)
}

/// The code an Agent acts on: the Provider's own `exhibits.*` code when the Host forwarded a
/// Provider refusal, otherwise the Host's code.
fn provider_code(error: &BackendError) -> String {
    error.details()["provider"]["code"]
        .as_str()
        .unwrap_or(error.code())
        .to_owned()
}

fn port_failure(port: &str, error: &BackendError) -> BackendError {
    failure(
        "fixture.port_failed",
        json!({"port": port, "code": provider_code(error), "uncertain": error.outcome_uncertain()}),
    )
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn chunk_count(size: usize) -> usize {
    size.div_ceil(CHUNK).max(1)
}

fn chunk(bytes: &[u8], index: usize) -> &[u8] {
    let start = index * CHUNK;
    &bytes[start..(start + CHUNK).min(bytes.len())]
}

#[tokio::main]
async fn main() -> ExitCode {
    Router::new(|activation| async move {
        let initialization = activation.initialization().clone();
        let grant = initialization
            .capability("teamarness.package-ports")
            .ok_or_else(|| failure("fixture.no_ports", json!({})))?
            .grant
            .clone();
        let ports = PortClient::new(activation.capabilities().clone(), grant);
        let binding =
            AgentBinding::from_initialization(&initialization, activation.capabilities().clone())?;
        Ok(Consumer {
            ports,
            binding,
            proof: Arc::new(Mutex::new(ProofState::Idle)),
        })
    })
    .request(
        "fixture.publish",
        |context: Context<Consumer>, request: PublishRequest| async move {
            let files = exhibit_fixtures::fixture(&request.fixture)
                .ok_or_else(|| failure("fixture.unknown", json!({})))?;
            let id = request
                .exhibit_id
                .unwrap_or_else(|| format!("fixture-{}", request.fixture));
            let title = request
                .title
                .unwrap_or_else(|| exhibit_fixtures::title(&request.fixture).to_owned());
            let ports = &context.state().ports;
            let current = current_revision(ports, &id).await?;
            let (revision, sent) = publish(ports, &id, &title, &files, Some(current)).await?;
            Ok(json!({"exhibitId": id, "revision": revision, "chunksSent": sent}))
        },
    )
    .request(
        "agent.bind",
        |context: Context<Consumer>, _: Empty| async move {
            let binding = &context.state().binding;
            let candidate = binding
                .candidates()
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| failure("fixture.no_agent", json!({})))?;
            let bound = binding.bind(&candidate.principal_id).await?;
            Ok(json!({"principalId": bound.principal_id, "identity": bound.identity}))
        },
    )
    .request(
        "agent.unbind",
        |context: Context<Consumer>, _: Empty| async move {
            context.state().binding.unbind().await?;
            Ok(json!({}))
        },
    )
    .request(
        "proof.start",
        |context: Context<Consumer>, request: ProofRequest| async move {
            let state = context.state().clone();
            {
                let mut proof = state
                    .proof
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if matches!(*proof, ProofState::Running) {
                    return Err(failure("fixture.proof_running", json!({})));
                }
                *proof = ProofState::Running;
            }
            tokio::spawn(async move {
                let report = match proof(&state.ports, &request.prefix).await {
                    Ok(report) => json!({"ok": true, "result": report}),
                    Err(error) => {
                        json!({"ok": false, "code": error.code(), "details": error.details()})
                    }
                };
                *state
                    .proof
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = ProofState::Done(report);
            });
            Ok(json!({"started": true}))
        },
    )
    .request(
        "proof.result",
        |context: Context<Consumer>, _: Empty| async move {
            let proof = context
                .state()
                .proof
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(match &*proof {
                ProofState::Idle => json!({"state": "idle"}),
                ProofState::Running => json!({"state": "running"}),
                ProofState::Done(report) => json!({"state": "done", "report": report}),
            })
        },
    )
    .run()
    .await
}

/// The Draft's current revision, or 0 when it does not exist.
async fn current_revision(ports: &PortClient, id: &str) -> Result<u64, BackendError> {
    match ports
        .query(CONTRACT, "exhibit.read", json!({"exhibitId": id}))
        .await
    {
        Ok(read) => Ok(read["revision"].as_u64().unwrap_or_default()),
        Err(error) if provider_code(&error) == "exhibits.not_found" => Ok(0),
        Err(error) => Err(port_failure("exhibit.read", &error)),
    }
}

async fn put(ports: &PortClient, bytes: &[u8], index: usize) -> Result<Value, BackendError> {
    ports
        .command(
            CONTRACT,
            "file.put",
            json!({
                "sha256": sha(bytes), "size": bytes.len(), "chunkIndex": index,
                "chunkCount": chunk_count(bytes.len()), "data": STANDARD.encode(chunk(bytes, index)),
            }),
        )
        .await
}

/// What `files.missing` reports, by hash: `None` for "all", or the missing chunk indices.
async fn missing(
    ports: &PortClient,
    hashes: &[String],
) -> Result<BTreeMap<String, Option<Vec<usize>>>, BackendError> {
    let answer = ports
        .query(CONTRACT, "files.missing", json!({"sha256s": hashes}))
        .await
        .map_err(|error| port_failure("files.missing", &error))?;
    Ok(answer["missing"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let chunks = entry["chunks"].as_array().map(|chunks| {
                chunks
                    .iter()
                    .filter_map(|index| {
                        index.as_u64().and_then(|index| usize::try_from(index).ok())
                    })
                    .collect()
            });
            (
                entry["sha256"].as_str().unwrap_or_default().to_owned(),
                chunks,
            )
        })
        .collect())
}

/// Uploads only what the store is missing, then names every file in `draft.update`. Returns the
/// new revision and the number of chunks sent.
async fn publish(
    ports: &PortClient,
    id: &str,
    title: &str,
    files: &[FixtureFile],
    expected: Option<u64>,
) -> Result<(u64, usize), BackendError> {
    let hashes: Vec<String> = files.iter().map(|(_, bytes)| sha(bytes)).collect();
    let missing = missing(ports, &hashes).await?;
    let mut sent = 0;
    for (_, bytes) in files {
        let Some(chunks) = missing.get(&sha(bytes)) else {
            continue;
        };
        let chunks = chunks
            .clone()
            .unwrap_or_else(|| (0..chunk_count(bytes.len())).collect());
        for index in chunks {
            put(ports, bytes, index)
                .await
                .map_err(|error| port_failure("file.put", &error))?;
            sent += 1;
        }
    }
    let mut request = json!({
        "exhibitId": id, "title": title, "entry": files[0].0,
        "files": files.iter().map(|(path, bytes)| json!({"path": path, "sha256": sha(bytes)})).collect::<Vec<_>>(),
    });
    if let Some(expected) = expected {
        request["expectedRevision"] = json!(expected);
    }
    let answer = ports
        .command(CONTRACT, "draft.update", request)
        .await
        .map_err(|error| port_failure("draft.update", &error))?;
    Ok((answer["revision"].as_u64().unwrap_or_default(), sent))
}

/// Reads a stored file back through `file.read`, chunk by chunk.
async fn read_back(ports: &PortClient, sha256: &str) -> Result<Vec<u8>, BackendError> {
    let mut bytes = Vec::new();
    let mut index = 0;
    loop {
        let answer = ports
            .query(
                CONTRACT,
                "file.read",
                json!({"sha256": sha256, "chunkIndex": index}),
            )
            .await
            .map_err(|error| port_failure("file.read", &error))?;
        bytes.extend(
            STANDARD
                .decode(answer["data"].as_str().unwrap_or_default())
                .map_err(|_| failure("fixture.bad_chunk", json!({})))?,
        );
        index += 1;
        if index >= answer["chunkCount"].as_u64().unwrap_or(1) {
            return Ok(bytes);
        }
    }
}

/// The system Port proof. Every check is recorded in the report; the test asserts it.
async fn proof(ports: &PortClient, prefix: &str) -> Result<Value, BackendError> {
    let guide = ports
        .query(CONTRACT, "runtime.guide", json!({}))
        .await
        .map_err(|error| port_failure("runtime.guide", &error))?;
    let site = format!("{prefix}-site");
    let media = format!("{prefix}-media");

    // Revision 1, then revision 2 of the same Exhibit: only the changed entry page is uploaded.
    let revision_1 = exhibit_fixtures::fixture("revision-1").unwrap_or_default();
    let (first, first_sent) = publish(ports, &site, "Site", &revision_1, Some(0)).await?;
    let revision_2 = exhibit_fixtures::fixture("revision-2").unwrap_or_default();
    let (second, second_sent) = publish(ports, &site, "Site", &revision_2, Some(first)).await?;

    // The media fixture's clip spans three chunks, more than one Port message. The response to
    // chunk 0 is dropped, as when a command ends `outcome_unknown`, and chunk 2 is sent before
    // chunk 1; `files.missing` then says exactly what is left.
    let media_files = exhibit_fixtures::fixture("media").unwrap_or_default();
    let clip = media_files
        .iter()
        .find(|(path, _)| path.ends_with(".mp4"))
        .map(|(_, bytes)| *bytes)
        .unwrap_or_default();
    let clip_sha = sha(clip);
    let before_upload = missing(ports, std::slice::from_ref(&clip_sha)).await?;
    let _dropped = put(ports, clip, 0).await;
    let last_first = put(ports, clip, 2)
        .await
        .map_err(|error| port_failure("file.put", &error))?;
    let resumed = missing(ports, std::slice::from_ref(&clip_sha)).await?;
    let mut resumed_sent = 0;
    for index in resumed
        .get(&clip_sha)
        .cloned()
        .flatten()
        .unwrap_or_default()
    {
        let answer = put(ports, clip, index)
            .await
            .map_err(|error| port_failure("file.put", &error))?;
        resumed_sent += 1;
        if answer["complete"] != true {
            return Err(failure("fixture.not_complete", json!({"index": index})));
        }
    }
    let after_upload = missing(ports, std::slice::from_ref(&clip_sha)).await?;
    let (media_revision, media_sent) =
        publish(ports, &media, "Media", &media_files, Some(0)).await?;

    let stale = publish(ports, &site, "Site", &revision_1, Some(first))
        .await
        .err()
        .map(|error| error.details()["code"].clone());
    let refused = ports
        .command(
            CONTRACT,
            "draft.update",
            json!({"exhibitId": site, "title": "Site", "entry": "index.html",
                "files": [{"path": "index.html", "sha256": sha(revision_2[0].1)},
                          {"path": "tool.exe", "sha256": sha(revision_2[0].1)}]}),
        )
        .await
        .err()
        .map(|error| provider_code(&error));

    let list = ports
        .query(CONTRACT, "exhibit.list", json!({}))
        .await
        .map_err(|error| port_failure("exhibit.list", &error))?;
    let read = ports
        .query(CONTRACT, "exhibit.read", json!({"exhibitId": site}))
        .await
        .map_err(|error| port_failure("exhibit.read", &error))?;
    let mut byte_for_byte = true;
    for exhibit in [&revision_2, &media_files] {
        for (_, bytes) in exhibit {
            byte_for_byte &= read_back(ports, &sha(bytes)).await? == *bytes;
        }
    }
    let read_files: Vec<(String, String)> = read["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|file| {
            (
                file["path"].as_str().unwrap_or_default().to_owned(),
                file["sha256"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let mut expected_files: Vec<(String, String)> = revision_2
        .iter()
        .map(|(path, bytes)| ((*path).to_owned(), sha(bytes)))
        .collect();
    expected_files.sort();

    let deleted = ports
        .command(
            CONTRACT,
            "draft.delete",
            json!({"exhibitId": media, "expectedRevision": media_revision}),
        )
        .await
        .map_err(|error| port_failure("draft.delete", &error))?;
    let after_delete = ports
        .query(CONTRACT, "exhibit.read", json!({"exhibitId": media}))
        .await
        .err()
        .map(|error| provider_code(&error));

    Ok(json!({
        "guideChunkBytes": guide["limits"]["chunkBytes"],
        "guideHasHtml": guide["mediaTypes"]["html"].is_string(),
        "first": first, "firstSent": first_sent,
        "second": second, "secondSent": second_sent,
        "clipBytes": clip.len(),
        "clipMissingBefore": before_upload.get(&clip_sha).map(|chunks| chunks.is_none()),
        "lastChunkFirstComplete": last_first["complete"],
        "resumedChunks": resumed.get(&clip_sha).cloned().flatten(),
        "resumedSent": resumed_sent,
        "clipMissingAfter": after_upload.contains_key(&clip_sha),
        "mediaRevision": media_revision, "mediaSent": media_sent,
        "stale": stale, "refused": refused,
        "listIds": list["exhibits"].as_array().into_iter().flatten().map(|entry| entry["id"].clone()).collect::<Vec<_>>(),
        "readRevision": read["revision"], "readEntry": read["entry"], "readTitle": read["title"],
        "readUpdatedBy": read["updatedBy"],
        "readFilesMatch": read_files == expected_files,
        "byteForByte": byte_for_byte,
        "deleted": deleted["deleted"], "afterDelete": after_delete,
    }))
}
