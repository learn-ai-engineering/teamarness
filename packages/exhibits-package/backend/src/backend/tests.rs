use std::sync::atomic::{AtomicI64, Ordering};

use sha2::{Digest, Sha256};
use teamarness_package_contract::port_contract::{CompiledPortContracts, compile_port_contracts};
use teamarness_package_contract::ui_contract::MAX_UI_PAYLOAD_BYTES;
use teamarness_package_sdk::testing::{
    Harness, HarnessBuilder, ScriptedCapability, assert_router_matches_contract,
};

use super::*;
use crate::domain::{
    CHUNK_BYTES, MAX_FILES, MAX_MISSING_QUERY, MAX_PATH_BYTES, MAX_WRITE_BYTES, READ_PAGE,
    TEXT_PAGE_BYTES,
};
use crate::store::hex;

const ALICE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const AGENT_PRINCIPAL: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const AGENT_KEY: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const VIEW: &str = "0123456789abcdef0123456789abcdef";

fn ports() -> CompiledPortContracts {
    compile_port_contracts(include_bytes!("../../../ports.json")).unwrap()
}

fn person() -> JsonValue {
    json!({"kind":"person","identity":ALICE})
}

fn agent() -> JsonValue {
    json!({"kind":"agent","principal":AGENT_PRINCIPAL,"identity":AGENT_KEY})
}

fn envelope(port: &str, actor: &JsonValue, request: JsonValue) -> JsonValue {
    json!({"contract":CONTRACT,"major":1,"minor":2,"port":port,"actor":actor,"request":request})
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

struct Session {
    harness: Harness,
    views: ScriptedCapability,
    _directory: tempfile::TempDir,
}

async fn start() -> Session {
    let directory = tempfile::tempdir().unwrap();
    let views = ScriptedCapability::new().calls(|method, payload| match method {
        "open" => Ok(json!({
            "view": VIEW,
            "url": format!("teamarness-content://localhost/{VIEW}/{}", payload["entry"].as_str().unwrap()),
        })),
        "close" => Ok(json!({"closed": true})),
        _ => Err(BackendError::new("capability_unavailable", false, json!({}))),
    });
    let now = Arc::new(AtomicI64::new(1_793_000_000_000));
    let mut harness = HarnessBuilder::new(
        router_with_clock(Arc::new(move || now.fetch_add(1, Ordering::SeqCst))).into_backend(),
    )
    .package_identity(CONTRACT)
    .installation(ALICE, "Alice")
    .data_directory(directory.path())
    .capability("teamarness.content-views", views.clone())
    .start()
    .await;
    harness.activate().await.unwrap();
    Session {
        harness,
        views,
        _directory: directory,
    }
}

impl Session {
    /// Calls one Port as `actor`, checking the request and the answer against `ports.json` the
    /// way the Host's broker does.
    async fn port(
        &mut self,
        port: &str,
        actor: &JsonValue,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        let contract = ports();
        let kind = &contract.contracts[0]
            .ports
            .iter()
            .find(|entry| entry.name == port)
            .unwrap()
            .kind;
        let operation = match serde_json::to_value(kind).unwrap().as_str().unwrap() {
            "command" => "teamarness.port.command.v1",
            _ => "teamarness.port.query.v1",
        };
        contract
            .validate(CONTRACT, port, "request", &request)
            .unwrap_or_else(|_| panic!("{port} request must match ports.json"));
        let answer = self
            .harness
            .request(operation, envelope(port, actor, request))
            .await?;
        assert_eq!(answer["$teamarnessPortEnvelope"], 1);
        let value = answer["$teamarnessPortValue"].clone();
        contract
            .validate(CONTRACT, port, "response", &value)
            .unwrap_or_else(|_| panic!("{port} response must match ports.json: {value}"));
        Ok(value)
    }

    async fn upload(&mut self, actor: &JsonValue, bytes: &[u8]) -> String {
        let sha256 = sha(bytes);
        let count = crate::domain::chunk_count(bytes.len() as u64);
        for index in 0..count {
            let start = usize::try_from(index).unwrap() * CHUNK_BYTES;
            let data = &bytes[start..(start + CHUNK_BYTES).min(bytes.len())];
            let answer = self
                .port(
                    "file.put",
                    actor,
                    json!({"sha256":sha256,"size":bytes.len(),"chunkIndex":index,"chunkCount":count,"data":STANDARD.encode(data)}),
                )
                .await
                .unwrap();
            assert_eq!(answer["complete"], index + 1 == count);
        }
        sha256
    }
}

fn code(result: Result<JsonValue, BackendError>) -> String {
    result.unwrap_err().code().to_owned()
}

#[test]
fn router_matches_the_ui_contract() {
    assert_router_matches_contract(
        &router().operations(),
        include_str!("../../../ui-contract.json"),
    );
    teamarness_package_contract::ui_contract::compile_ui_contract(include_bytes!(
        "../../../ui-contract.json"
    ))
    .unwrap();
}

#[test]
fn the_port_contract_declares_every_port_with_agent_guidance() {
    let contract = ports();
    assert_eq!(contract.contracts.len(), 1);
    let provided = &contract.contracts[0];
    assert_eq!(provided.identity, CONTRACT);
    assert_eq!((provided.major, provided.minor), (1, 2));
    let mut names: Vec<(String, String)> = provided
        .ports
        .iter()
        .map(|port| {
            assert!(
                !port.description.is_empty(),
                "{} needs a description",
                port.name
            );
            (
                port.name.clone(),
                serde_json::to_value(port.kind)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            ("draft.delete", "command"),
            ("draft.update", "command"),
            ("exhibit.list", "query"),
            ("exhibit.read", "query"),
            ("file.put", "command"),
            ("file.read", "query"),
            ("file.text", "query"),
            ("file.write", "command"),
            ("files.missing", "query"),
            ("runtime.guide", "query"),
        ]
        .map(|(name, kind)| (name.to_owned(), kind.to_owned()))
    );
}

/// The largest valid request of each Port, and the largest answers, stay inside the Port
/// payload bound and match `ports.json`.
#[test]
fn the_largest_requests_and_answers_fit_one_port_message() {
    let contract = ports();
    let sha = "a".repeat(64);
    let longest_path = |index: usize| {
        let stem = format!("{index:04}");
        format!("{stem}{}.html", "p".repeat(MAX_PATH_BYTES - stem.len() - 5))
    };
    let files: Vec<JsonValue> = (0..MAX_FILES)
        .map(|index| json!({"path": longest_path(index), "sha256": sha}))
        .collect();
    let requests = [
        ("runtime.guide", json!({})),
        (
            "file.put",
            json!({"sha256":sha,"size":crate::domain::MAX_FILE_BYTES,"chunkIndex":0,"chunkCount":128,
                "data":STANDARD.encode(vec![0xff_u8; CHUNK_BYTES])}),
        ),
        ("file.write", json!({"text":"\"".repeat(MAX_WRITE_BYTES)})),
        ("file.write", json!({"text":"\\".repeat(MAX_WRITE_BYTES)})),
        (
            "files.missing",
            json!({"sha256s": vec![sha.clone(); MAX_MISSING_QUERY]}),
        ),
        (
            "draft.update",
            json!({"exhibitId":"a".repeat(48),"title":"\u{1F600}".repeat(120),"entry":longest_path(0),
                "files":files,"expectedRevision":9_007_199_254_740_991_u64}),
        ),
        (
            "exhibit.list",
            json!({"before":format!("{}:{}", i64::MAX, "a".repeat(48)),"limit":64}),
        ),
        (
            "exhibit.read",
            json!({"exhibitId":"a".repeat(48),"offset":500}),
        ),
        ("file.read", json!({"sha256":sha,"chunkIndex":127})),
        (
            "draft.delete",
            json!({"exhibitId":"a".repeat(48),"expectedRevision":1}),
        ),
    ];
    for (port, request) in &requests {
        let size = serde_json::to_vec(&envelope(port, &agent(), request.clone()))
            .unwrap()
            .len();
        assert!(
            size < MAX_UI_PAYLOAD_BYTES,
            "{port} request is {size} bytes"
        );
        assert!(
            contract
                .validate(CONTRACT, port, "request", request)
                .is_ok(),
            "{port} largest request must be valid"
        );
    }
    let summary = json!({"id":"a".repeat(48),"title":"\u{1F600}".repeat(120),
        "updatedAt":"2026-10-26T07:33:20.123Z","kind":"draft","revision":9_007_199_254_740_991_u64});
    let file = json!({"path":longest_path(0),"sha256":sha,"size":16_777_216,
        "mediaType":"text/javascript; charset=utf-8"});
    let answers = [
        ("runtime.guide", guide::guide()),
        (
            "file.read",
            json!({"data":STANDARD.encode(vec![0_u8; CHUNK_BYTES]),"chunkCount":128}),
        ),
        (
            "files.missing",
            json!({"missing": vec![json!({"sha256":sha,"chunks":(0..127).collect::<Vec<_>>()}); MAX_MISSING_QUERY]}),
        ),
        (
            "exhibit.list",
            json!({"exhibits":vec![summary; 64],"nextBefore":"x".repeat(128)}),
        ),
        (
            "exhibit.read",
            json!({"exhibitId":"a".repeat(48),"title":"t","entry":longest_path(0),"revision":1,
                "updatedAt":"2026-10-26T07:33:20.123Z","updatedBy":{"kind":"agent","identity":AGENT_KEY},
                "fileCount":1000,
                "files":vec![file; READ_PAGE],"nextOffset":500}),
        ),
    ];
    for (port, answer) in &answers {
        let size = serde_json::to_vec(answer).unwrap().len();
        assert!(size < MAX_UI_PAYLOAD_BYTES, "{port} answer is {size} bytes");
        assert!(
            contract
                .validate(CONTRACT, port, "response", answer)
                .is_ok(),
            "{port} largest answer must be valid"
        );
    }
}

#[test]
fn the_port_schemas_refuse_unbounded_or_forged_requests() {
    let contract = ports();
    let sha = "a".repeat(64);
    for (port, request) in [
        (
            "file.put",
            json!({"sha256":sha,"size":1,"chunkIndex":0,"chunkCount":1,"data":"x".repeat(174_765)}),
        ),
        (
            "file.put",
            json!({"sha256":"A".repeat(64),"size":1,"chunkIndex":0,"chunkCount":1,"data":""}),
        ),
        ("files.missing", json!({"sha256s": vec![sha.clone(); 257]})),
        ("files.missing", json!({"sha256s": []})),
        (
            "draft.update",
            json!({"exhibitId":"Bad","title":"t","entry":"index.html","files":[{"path":"index.html","sha256":sha}]}),
        ),
        (
            "draft.update",
            json!({"exhibitId":"ok","title":"t","entry":"index.html","files":[{"path":"index.html","sha256":sha}],"actor":"forged"}),
        ),
        ("exhibit.list", json!({"limit":65})),
        (
            "file.write",
            json!({"text":"x".repeat(MAX_WRITE_BYTES + 1)}),
        ),
        ("file.write", json!({})),
        ("file.write", json!({"text":"x","path":"index.html"})),
        ("runtime.guide", json!({"verbose":true})),
    ] {
        assert!(
            contract
                .validate(CONTRACT, port, "request", &request)
                .is_err(),
            "{port} must refuse {request}"
        );
    }
}

#[tokio::test]
async fn person_and_agent_upload_update_list_read_and_delete() {
    let mut session = start().await;
    for (actor, id, seed) in [(person(), "person-draft", 1), (agent(), "agent-draft", 2)] {
        let big: Vec<u8> = (0..CHUNK_BYTES * 2 + 7)
            .map(|i| (i % 251) as u8 ^ seed)
            .collect();
        let page = format!("<h1>{id}</h1>").into_bytes();
        let missing = session
            .port(
                "files.missing",
                &actor,
                json!({"sha256s":[sha(&big), sha(&page)]}),
            )
            .await
            .unwrap();
        assert_eq!(
            missing["missing"],
            json!([{"sha256":sha(&big),"chunks":"all"},{"sha256":sha(&page),"chunks":"all"}])
        );
        let big_sha = session.upload(&actor, &big).await;
        let page_sha = session.upload(&actor, &page).await;
        assert_eq!(
            session
                .port(
                    "files.missing",
                    &actor,
                    json!({"sha256s":[big_sha, page_sha]})
                )
                .await
                .unwrap(),
            json!({"missing":[]})
        );
        let files = json!([{"path":"index.html","sha256":page_sha},{"path":"assets/data.wasm","sha256":big_sha}]);
        let created = session
            .port(
                "draft.update",
                &actor,
                json!({"exhibitId":id,"title":"First","entry":"index.html","files":files,"expectedRevision":0}),
            )
            .await
            .unwrap();
        assert_eq!(created, json!({"revision":1}));
        let updated = session
            .port(
                "draft.update",
                &actor,
                json!({"exhibitId":id,"title":"Second","entry":"index.html","files":files,"expectedRevision":1}),
            )
            .await
            .unwrap();
        assert_eq!(updated, json!({"revision":2}));
        let list = session
            .port("exhibit.list", &actor, json!({}))
            .await
            .unwrap();
        assert_eq!(list["exhibits"][0]["id"], id);
        assert_eq!(list["exhibits"][0]["revision"], 2);
        assert_eq!(list["exhibits"][0]["kind"], "draft");
        let read = session
            .port("exhibit.read", &actor, json!({"exhibitId":id}))
            .await
            .unwrap();
        assert_eq!(read["title"], "Second");
        assert_eq!(read["updatedBy"]["kind"], actor["kind"]);
        assert_eq!(read["updatedBy"]["identity"], actor["identity"]);
        assert_eq!(read["entry"], "index.html");
        assert_eq!(read["fileCount"], 2);
        assert_eq!(
            read["files"],
            json!([
                {"path":"assets/data.wasm","sha256":big_sha,"size":big.len(),"mediaType":"application/wasm"},
                {"path":"index.html","sha256":page_sha,"size":page.len(),"mediaType":"text/html; charset=utf-8"},
            ])
        );
        let mut back = Vec::new();
        for index in 0..3 {
            let chunk = session
                .port(
                    "file.read",
                    &actor,
                    json!({"sha256":big_sha,"chunkIndex":index}),
                )
                .await
                .unwrap();
            assert_eq!(chunk["chunkCount"], 3);
            back.extend(STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap());
        }
        assert_eq!(back, big);
        let deleted = session
            .port(
                "draft.delete",
                &actor,
                json!({"exhibitId":id,"expectedRevision":2}),
            )
            .await
            .unwrap();
        assert_eq!(deleted, json!({"deleted":true}));
        assert_eq!(
            session
                .port("draft.delete", &actor, json!({"exhibitId":id}))
                .await
                .unwrap(),
            json!({"deleted":false})
        );
    }
    session.harness.shutdown().await.unwrap();
}

#[tokio::test]
async fn refusals_carry_their_exhibits_codes() {
    let mut session = start().await;
    let actor = agent();
    let page = session.upload(&actor, b"<p>x</p>").await;
    let absent = "e".repeat(64);
    let update = |id: &str, files: JsonValue, expected: Option<u64>| {
        let mut request = json!({"exhibitId":id,"title":"T","entry":"index.html","files":files});
        if let Some(expected) = expected {
            request["expectedRevision"] = json!(expected);
        }
        request
    };
    let missing = session
        .port(
            "draft.update",
            &actor,
            update(
                "x",
                json!([{"path":"index.html","sha256":page},{"path":"a.js","sha256":absent}]),
                None,
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "exhibits.missing_files");
    assert_eq!(missing.details()["missing"], json!([absent]));
    assert_eq!(missing.details()["missingCount"], 1);
    assert_eq!(
        code(
            session
                .port(
                    "draft.update",
                    &actor,
                    update("x", json!([{"path":"index.html","sha256":page}]), Some(4))
                )
                .await
        ),
        "exhibits.revision_conflict"
    );
    // The ID is checked past the schema too, for a Consumer that bypasses it.
    assert_eq!(
        code(
            session
                .harness
                .request(
                    "teamarness.port.command.v1",
                    envelope(
                        "draft.update",
                        &actor,
                        update("Bad_Id", json!([{"path":"index.html","sha256":page}]), None)
                    ),
                )
                .await
        ),
        "exhibits.invalid_id"
    );
    assert_eq!(
        code(
            session
                .port("draft.update", &actor, update("x", json!([{"path":"index.html","sha256":page},{"path":"../up.css","sha256":page}]), None))
                .await
        ),
        "exhibits.invalid_path"
    );
    assert_eq!(
        code(
            session
                .port("draft.update", &actor, update("x", json!([{"path":"index.html","sha256":page},{"path":"run.exe","sha256":page}]), None))
                .await
        ),
        "exhibits.type_refused"
    );
    let too_many: Vec<JsonValue> = (0..=MAX_FILES)
        .map(|index| json!({"path":format!("p{index}.html"),"sha256":page}))
        .collect();
    let count = session
        .port(
            "draft.update",
            &actor,
            json!({"exhibitId":"x","title":"T","entry":"p0.html","files":too_many}),
        )
        .await
        .unwrap_err();
    assert_eq!(count.code(), "exhibits.too_large");
    assert_eq!(count.details()["limit"], "count");
    let file = session
        .port(
            "file.put",
            &actor,
            json!({"sha256":absent,"size":crate::domain::MAX_FILE_BYTES + 1,"chunkIndex":0,"chunkCount":129,"data":""}),
        )
        .await
        .unwrap_err();
    assert_eq!(file.code(), "exhibits.too_large");
    assert_eq!(file.details()["limit"], "file");
    let short = session
        .port(
            "file.put",
            &actor,
            json!({"sha256":absent,"size":CHUNK_BYTES + 1,"chunkIndex":0,"chunkCount":2,"data":STANDARD.encode([1_u8; 10])}),
        )
        .await
        .unwrap_err();
    assert_eq!(short.code(), "exhibits.invalid_chunk");
    assert_eq!(short.details()["reason"], "chunk_size");
    assert_eq!(
        code(
            session
                .port(
                    "file.put",
                    &actor,
                    json!({"sha256":absent,"size":1,"chunkIndex":0,"chunkCount":1,"data":"*"})
                )
                .await
        ),
        "exhibits.invalid_chunk"
    );
    assert_eq!(
        code(
            session
                .port("file.put", &actor, json!({"sha256":absent,"size":1,"chunkIndex":0,"chunkCount":1,"data":STANDARD.encode([7_u8])}))
                .await
        ),
        "exhibits.content_mismatch"
    );
    assert_eq!(
        code(
            session
                .port("exhibit.read", &actor, json!({"exhibitId":"nothing"}))
                .await
        ),
        "exhibits.not_found"
    );
    assert_eq!(
        code(
            session
                .port("file.read", &actor, json!({"sha256":absent,"chunkIndex":0}))
                .await
        ),
        "exhibits.not_found"
    );
    let stranger = json!({"kind":"person","identity":"9".repeat(64)});
    assert_eq!(
        code(session.port("runtime.guide", &stranger, json!({})).await),
        "exhibits.actor_unauthorized"
    );
    session.harness.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_total_bound_is_refused_with_too_large() {
    let mut session = start().await;
    let directory = session._directory.path().join("content/sha256");
    let mut files = vec![];
    for seed in 0..5_u8 {
        let name = format!("{seed:064x}");
        std::fs::File::create(directory.join(&name))
            .unwrap()
            .set_len(crate::domain::MAX_FILE_BYTES)
            .unwrap();
        files.push(json!({"path":format!("v{seed}.mp4"),"sha256":name}));
    }
    let error = session
        .port(
            "draft.update",
            &agent(),
            json!({"exhibitId":"big","title":"T","entry":"v0.mp4","files":files}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), "exhibits.too_large");
    assert_eq!(error.details()["limit"], "total");
    session.harness.shutdown().await.unwrap();
}

#[tokio::test]
async fn runtime_guide_answers_through_the_port() {
    let mut session = start().await;
    let guide = session
        .port("runtime.guide", &agent(), json!({}))
        .await
        .unwrap();
    assert_eq!(guide, guide::guide());
    session.harness.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_ui_lists_opens_a_view_and_hears_changes() {
    let mut session = start().await;
    let mut changes = session
        .harness
        .subscribe("exhibits.changed", json!({}))
        .await
        .unwrap();
    let page = session.upload(&agent(), b"<h1>view</h1>").await;
    let style = session.upload(&agent(), b"h1{}").await;
    session
        .port(
            "draft.update",
            &agent(),
            json!({"exhibitId":"shown","title":"Shown","entry":"index.html",
                "files":[{"path":"index.html","sha256":page},{"path":"css/a.css","sha256":style}]}),
        )
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), changes.next_event())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        event,
        json!({"exhibitId":"shown","revision":1,"deleted":false})
    );
    let list = session
        .harness
        .request("exhibit.list", json!({}))
        .await
        .unwrap();
    assert_eq!(list["exhibits"][0]["id"], "shown");
    let opened = session
        .harness
        .request("exhibit.open", json!({"exhibitId":"shown"}))
        .await
        .unwrap();
    assert_eq!(
        opened,
        json!({"url":format!("teamarness-content://localhost/{VIEW}/index.html"),"revision":1})
    );
    let calls = session.views.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "open");
    assert_eq!(
        calls[0].payload,
        json!({"entry":"index.html","files":[
            {"path":"css/a.css","sha256":style,"size":4,"source":{"kind":"file","path":format!("content/sha256/{style}")}},
            {"path":"index.html","sha256":page,"size":13,"source":{"kind":"file","path":format!("content/sha256/{page}")}},
        ]})
    );
    session
        .port("draft.delete", &agent(), json!({"exhibitId":"shown"}))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), changes.next_event())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        event,
        json!({"exhibitId":"shown","revision":0,"deleted":true})
    );
    assert_eq!(
        code(
            session
                .harness
                .request("exhibit.open", json!({"exhibitId":"shown"}))
                .await
        ),
        "exhibits.not_found"
    );
    changes.close().await;
    session.harness.shutdown().await.unwrap();
}

#[tokio::test]
async fn exhibits_changed_carries_one_event_per_update_and_delete() {
    let mut session = start().await;
    let mut changes = session
        .harness
        .subscribe("exhibits.changed", json!({}))
        .await
        .unwrap();
    let page = session.upload(&agent(), b"<h1>one</h1>").await;
    let update = |id: &str, expected: Option<u64>| {
        let mut request = json!({"exhibitId":id,"title":"T","entry":"index.html",
            "files":[{"path":"index.html","sha256":page}]});
        if let Some(expected) = expected {
            request["expectedRevision"] = json!(expected);
        }
        request
    };
    session
        .port("draft.update", &agent(), update("a", Some(0)))
        .await
        .unwrap();
    session
        .port("draft.update", &person(), update("a", Some(1)))
        .await
        .unwrap();
    // Refusals change nothing and publish nothing.
    assert_eq!(
        code(
            session
                .port("draft.update", &agent(), update("a", Some(1)))
                .await
        ),
        "exhibits.revision_conflict"
    );
    assert_eq!(
        code(
            session
                .port(
                    "draft.delete",
                    &agent(),
                    json!({"exhibitId":"a","expectedRevision":1})
                )
                .await
        ),
        "exhibits.revision_conflict"
    );
    session
        .port(
            "draft.delete",
            &agent(),
            json!({"exhibitId":"a","expectedRevision":2}),
        )
        .await
        .unwrap();
    // Deleting a Draft that no longer exists is idempotent and publishes nothing.
    session
        .port("draft.delete", &agent(), json!({"exhibitId":"a"}))
        .await
        .unwrap();
    // A last update marks the end, so any extra event would arrive before it.
    session
        .port("draft.update", &agent(), update("b", None))
        .await
        .unwrap();
    let mut events = Vec::new();
    for _ in 0..4 {
        events.push(
            tokio::time::timeout(Duration::from_secs(5), changes.next_event())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(
        events,
        vec![
            json!({"exhibitId":"a","revision":1,"deleted":false}),
            json!({"exhibitId":"a","revision":2,"deleted":false}),
            json!({"exhibitId":"a","revision":0,"deleted":true}),
            json!({"exhibitId":"b","revision":1,"deleted":false}),
        ]
    );
    changes.close().await;
    session.harness.shutdown().await.unwrap();
}

/// Asserts that `new` is an additive minor over `old`: every earlier Port keeps its kind and
/// schemas, `added` Ports are new, and only the `amended` Ports change their description.
fn assert_additive(old: &[u8], new: &[u8], minors: (u16, u16), added: usize, amended: &[&str]) {
    use teamarness_package_contract::port_contract::check_additive_minor_documents;
    let old = compile_port_contracts(old).unwrap();
    let new = compile_port_contracts(new).unwrap();
    assert_eq!(
        (old.contracts[0].major, old.contracts[0].minor),
        (1, minors.0)
    );
    assert_eq!(
        (new.contracts[0].major, new.contracts[0].minor),
        (1, minors.1)
    );
    check_additive_minor_documents(&old, &new, CONTRACT).unwrap();
    assert_eq!(
        old.contracts[0].ports.len() + added,
        new.contracts[0].ports.len()
    );
    for before in &old.contracts[0].ports {
        let after = new.contracts[0]
            .ports
            .iter()
            .find(|port| port.name == before.name)
            .unwrap();
        assert_eq!(before.kind, after.kind);
        assert_eq!(before.request, after.request);
        assert_eq!(before.response, after.response);
        assert_eq!(before.event, after.event);
        if !amended.contains(&before.name.as_str()) {
            assert_eq!(before.description, after.description, "{}", before.name);
        }
    }
}

// Verifies: TEAM.SYS.PKG-R18 — 1.1 keeps every 1.0 Port, kind, schema and description, and
// adds file.write.
#[test]
fn port_contract_1_1_is_additive_over_1_0() {
    assert_additive(
        include_bytes!("../../tests/fixtures/ports-1.0.json"),
        include_bytes!("../../tests/fixtures/ports-1.1.json"),
        (0, 1),
        1,
        &[],
    );
}

// Verifies: TEAM.SYS.PKG-R18 — 1.2 keeps every 1.1 Port, kind and schema and adds file.text;
// only exhibit.read and file.read change their description, to point text reads at file.text,
// and file.write's names the control characters it refuses.
#[test]
fn port_contract_1_2_is_additive_over_1_1() {
    assert_additive(
        include_bytes!("../../tests/fixtures/ports-1.1.json"),
        include_bytes!("../../../ports.json"),
        (1, 2),
        1,
        &["exhibit.read", "file.read", "file.write"],
    );
}

// Verifies: TEAM.SYS.PKG-R18 — file.text reads a stored text file back as plain text in pages
// that end on character boundaries, for the Person and a bound Agent.
#[tokio::test]
async fn file_text_reads_stored_text_back_in_pages() {
    let mut session = start().await;
    for actor in [person(), agent()] {
        let page = format!(
            "<!doctype html>\n<h1 class=\"title\">Hello \u{1F600}</h1>\n{}",
            "<p>row</p>\n".repeat(2_000)
        );
        let written = session
            .port("file.write", &actor, json!({"text":page}))
            .await
            .unwrap();
        // A small file comes back whole, with no nextOffset.
        let small = session
            .port(
                "file.write",
                &actor,
                json!({"text":"body { color: #333; }"}),
            )
            .await
            .unwrap();
        assert_eq!(
            session
                .port("file.text", &actor, json!({"sha256":small["sha256"]}))
                .await
                .unwrap(),
            json!({"text":"body { color: #333; }","size":21})
        );
        // A larger file is read in default pages; joined, they are the file.
        let mut text = String::new();
        let mut offset = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"sha256":written["sha256"]});
            if let Some(offset) = offset {
                request["offset"] = json!(offset);
            }
            let answer = session.port("file.text", &actor, request).await.unwrap();
            assert_eq!(answer["size"], page.len());
            let chunk = answer["text"].as_str().unwrap();
            assert!(chunk.len() <= TEXT_PAGE_BYTES);
            assert!(answer.to_string().chars().count() <= 16_384);
            text.push_str(chunk);
            pages += 1;
            match answer.get("nextOffset") {
                Some(next) => offset = Some(next.as_u64().unwrap()),
                None => break,
            }
        }
        assert_eq!(text, page);
        assert_eq!(pages, page.len().div_ceil(TEXT_PAGE_BYTES));
        // A page that would cut the four-byte emoji ends before it instead.
        let cut = page.find('\u{1F600}').unwrap() + 2;
        let answer = session
            .port(
                "file.text",
                &actor,
                json!({"sha256":written["sha256"],"maxBytes":cut}),
            )
            .await
            .unwrap();
        assert_eq!(answer["text"], page[..cut - 2]);
        assert_eq!(answer["nextOffset"], cut - 2);
        // The whole file in one page.
        assert_eq!(
            session
                .port(
                    "file.text",
                    &actor,
                    json!({"sha256":written["sha256"],"maxBytes":MAX_WRITE_BYTES})
                )
                .await
                .unwrap(),
            json!({"text":page,"size":page.len()})
        );
    }
    session.harness.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R18 — file.text refuses binary content, unknown files, bad offsets and
// page sizes, and another Installation's Person.
#[tokio::test]
async fn file_text_refuses_binary_unknown_and_strangers() {
    let mut session = start().await;
    let binary = session
        .upload(&agent(), &[0x89, b'P', b'N', b'G', 0xff, 0xfe])
        .await;
    // Valid UTF-8, but a file of zeros is not text: JSON would write each byte as six.
    let nul = session.upload(&agent(), &[0; 64]).await;
    // An offset inside a character starts no text.
    let accented = session.upload(&agent(), "é".as_bytes()).await;
    let text = session
        .port("file.write", &agent(), json!({"text":"abc"}))
        .await
        .unwrap()["sha256"]
        .clone();
    for (request, expected) in [
        (json!({"sha256":binary}), "exhibits.not_text"),
        (json!({"sha256":sha(b"never stored")}), "exhibits.not_found"),
        (
            json!({"sha256":text,"offset":4}),
            "exhibits.invalid_request",
        ),
        (json!({"sha256":nul}), "exhibits.not_text"),
        (json!({"sha256":accented,"offset":1}), "exhibits.not_text"),
    ] {
        assert_eq!(
            code(session.port("file.text", &agent(), request).await),
            expected
        );
    }
    // Reading from the end answers empty text.
    assert_eq!(
        session
            .port("file.text", &agent(), json!({"sha256":text,"offset":3}))
            .await
            .unwrap(),
        json!({"text":"","size":3})
    );
    let stranger = json!({"kind":"person","identity":"9".repeat(64)});
    assert_eq!(
        code(
            session
                .port("file.text", &stranger, json!({"sha256":text}))
                .await
        ),
        "exhibits.actor_unauthorized"
    );
    session.harness.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R18 — file.write stores the UTF-8 text content-addressed, as a
// completed file.put would, for the Person and a bound Agent.
#[tokio::test]
async fn file_write_stores_text_as_an_uploaded_file() {
    let mut session = start().await;
    for (actor, id) in [(person(), "person-written"), (agent(), "agent-written")] {
        let page = format!("<!doctype html><h1>{id} \u{1F600}</h1><link href=\"style.css\">");
        let style = "body { color: #333; }\n";
        let written = session
            .port("file.write", &actor, json!({"text":page}))
            .await
            .unwrap();
        assert_eq!(
            written,
            json!({"sha256":sha(page.as_bytes()),"size":page.len()})
        );
        // Idempotent: the same text again answers the same hash and stores nothing new.
        assert_eq!(
            session
                .port("file.write", &actor, json!({"text":page}))
                .await
                .unwrap(),
            written
        );
        let style_sha = session
            .port("file.write", &actor, json!({"text":style}))
            .await
            .unwrap()["sha256"]
            .clone();
        // An empty file is a file too.
        let empty = session
            .port("file.write", &actor, json!({"text":""}))
            .await
            .unwrap();
        assert_eq!(empty, json!({"sha256":sha(b""),"size":0}));
        // A written file is stored completely, so files.missing leaves it out.
        assert_eq!(
            session
                .port(
                    "files.missing",
                    &actor,
                    json!({"sha256s":[written["sha256"], style_sha, empty["sha256"]]})
                )
                .await
                .unwrap(),
            json!({"missing":[]})
        );
        // A file written as text and one uploaded with file.put are the same stored content.
        assert_eq!(session.upload(&actor, style.as_bytes()).await, style_sha);
        let created = session
            .port(
                "draft.update",
                &actor,
                json!({"exhibitId":id,"title":"Written","entry":"index.html","expectedRevision":0,
                    "files":[{"path":"index.html","sha256":written["sha256"]},{"path":"style.css","sha256":style_sha}]}),
            )
            .await
            .unwrap();
        assert_eq!(created, json!({"revision":1}));
        let read = session
            .port("exhibit.read", &actor, json!({"exhibitId":id}))
            .await
            .unwrap();
        assert_eq!(
            read["files"],
            json!([
                {"path":"index.html","sha256":written["sha256"],"size":page.len(),"mediaType":"text/html; charset=utf-8"},
                {"path":"style.css","sha256":style_sha,"size":style.len(),"mediaType":"text/css; charset=utf-8"},
            ])
        );
        let chunk = session
            .port(
                "file.read",
                &actor,
                json!({"sha256":written["sha256"],"chunkIndex":0}),
            )
            .await
            .unwrap();
        assert_eq!(chunk["chunkCount"], 1);
        assert_eq!(
            STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap(),
            page.as_bytes()
        );
    }
    // The largest text is accepted.
    let largest = "x".repeat(MAX_WRITE_BYTES);
    assert_eq!(
        session
            .port("file.write", &agent(), json!({"text":largest}))
            .await
            .unwrap()["size"],
        MAX_WRITE_BYTES
    );
    session.harness.shutdown().await.unwrap();
}

// Verifies: TEAM.SYS.PKG-R18 — text over the bound is refused with exhibits.too_large and
// nothing is stored; control characters are refused with exhibits.not_text; another Installation's Person is refused as for file.put.
#[tokio::test]
async fn file_write_refuses_oversized_text_and_strangers() {
    let mut session = start().await;
    // 24577 four-byte characters pass the schema's character bound but not the byte bound.
    let wide = "\u{1F600}".repeat(MAX_WRITE_BYTES / 4 + 1);
    let error = session
        .port("file.write", &agent(), json!({"text":wide}))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "exhibits.too_large");
    assert_eq!(error.details()["limit"], "text");
    // Control characters other than tab, line feed, form feed and carriage return are not
    // text; those four are.
    for (text, expected) in [
        ("a\u{0}b", Some("exhibits.not_text")),
        ("\u{1b}[0m", Some("exhibits.not_text")),
        ("a\tb\r\n\u{c}", None),
    ] {
        let answer = session
            .port("file.write", &agent(), json!({"text":text}))
            .await;
        assert_eq!(
            answer.err().map(|error| error.code().to_owned()).as_deref(),
            expected,
            "{text:?}"
        );
    }
    assert_eq!(
        session
            .port(
                "files.missing",
                &agent(),
                json!({"sha256s":[sha(wide.as_bytes())]})
            )
            .await
            .unwrap(),
        json!({"missing":[{"sha256":sha(wide.as_bytes()),"chunks":"all"}]})
    );
    let stranger = json!({"kind":"person","identity":"9".repeat(64)});
    assert_eq!(
        code(
            session
                .port("file.write", &stranger, json!({"text":"x"}))
                .await
        ),
        "exhibits.actor_unauthorized"
    );
    session.harness.shutdown().await.unwrap();
}
