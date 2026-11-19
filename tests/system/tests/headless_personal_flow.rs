//! Unknown Personal fixture uses existing Person Ports; removal leaves Team replication intact.
#[path = "support/headless.rs"]
mod headless;
use headless::*;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use teamarness_package_host::{PackageHostCommand, ReconcileWorkspaces};

// Execute the canonical runbook against disposable loopback storage. This is not Don's M4 observation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_runbook_fixture_settings_lifecycle_and_retention() {
    no_external_configuration();
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let application = stage(root.path());
    let desktop = Desktop::open(&application, &root.path().join("desktop")).await;
    let ws = desktop.core.create_workspace("Runbook").unwrap().id();
    desktop
        .host
        .execute(PackageHostCommand::ReconcileWorkspaces(
            ReconcileWorkspaces {},
        ))
        .await
        .unwrap();
    let a_path = root.path().join("a");
    let a = Headless::open(&application, &a_path, &desktop.core).await;
    a.join(&desktop.core, ws, "Runbook operator").await;
    let directory = root.path().join("runbook-fixture");
    let output = tokio::process::Command::new("node")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args([
            "scripts/prepare-headless-fixture.mjs",
            directory.to_str().unwrap(),
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let package = "org.unfamiliar.headless-proof";
    a.cli(
        &[
            "packages",
            "inspect",
            "--directory",
            directory.to_str().unwrap(),
        ],
        None,
    )
    .await;
    let install = [
        "packages",
        "install",
        "--directory",
        directory.to_str().unwrap(),
        "--package",
        package,
        "--input",
        "-",
    ];
    a.cli(
        &install,
        Some(json!({"acceptedRisks":["filesystem","network","process"]})),
    )
    .await;
    let ws = ws.to_string();
    let operations = a
        .cli(
            &[
                "package",
                "operations",
                "--workspace",
                &ws,
                "--package",
                package,
            ],
            None,
        )
        .await;
    assert!(operations.to_string().contains("proof.settings.set"));
    assert_eq!(
        a.request(
            &ws,
            package,
            "proof.settings.set",
            json!({"value":"manual qualification"})
        )
        .await,
        json!({"value":"manual qualification"})
    );
    for action in ["pause", "resume"] {
        a.cli(
            &["packages", action, "--workspace", &ws, "--package", package],
            None,
        )
        .await;
    }
    a.cli(&["packages", "reload", "--package", package], None)
        .await;
    assert_eq!(
        a.request(&ws, package, "proof.settings.get", json!({}))
            .await,
        json!({"value":"manual qualification"})
    );
    a.cli(&["packages", "remove", "--package", package], None)
        .await;
    a.cli(
        &install,
        Some(json!({"acceptedRisks":["filesystem","network","process"]})),
    )
    .await;
    assert_eq!(
        a.request(&ws, package, "proof.settings.get", json!({}))
            .await,
        json!({"value":"manual qualification"})
    );
    a.stop().await;
    let a = Headless::open(&application, &a_path, &desktop.core).await;
    assert_eq!(
        a.request(&ws, package, "proof.settings.get", json!({}))
            .await,
        json!({"value":"manual qualification"})
    );
    a.cli(&["packages", "remove", "--package", package], None)
        .await;
    a.stop().await;
    desktop.stop().await;
}

fn stage_driver(root: &Path) -> PathBuf {
    let directory = root.join("personal");
    fs::create_dir_all(directory.join("backend")).unwrap();
    fs::create_dir_all(directory.join("ui")).unwrap();
    let backend = fs::read(env!("CARGO_BIN_EXE_person-port-consumer-under-test")).unwrap();
    fs::write(directory.join("backend/driver"), &backend).unwrap();
    fs::set_permissions(
        directory.join("backend/driver"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let ui = b"throw new Error('UI_MUST_NOT_EXECUTE');";
    let contract = serde_json::to_vec(&json!({"schemaVersion":1,"$defs":{},"requests":{"proof.execute":{"request":{"type":"object"},"response":{"type":"object"}}},"subscriptions":{}})).unwrap();
    fs::write(directory.join("ui/index.js"), ui).unwrap();
    fs::write(directory.join("ui-contract.json"), &contract).unwrap();
    fs::write(directory.join("teamarness.package.json"),serde_json::to_vec(&json!({"schemaVersion":1,"identity":DRIVER,"version":"1.0.0","presentation":{"displayName":"Independent mesh fixture"},"protocol":{"major":1,"minMinor":0,"maxMinor":1},"activation":"eager",
        "backends":[{"target":teamarness_package_host::NATIVE_BACKEND_TARGET,"path":"backend/driver","size":backend.len(),"sha256":digest(&backend)}],
        "ui":{"entrypoint":{"path":"ui/index.js","size":ui.len(),"sha256":digest(ui)},"assets":[],"contract":{"path":"ui-contract.json","sha256":digest(&contract)}},
        "capabilities":[{"identity":"teamarness.package-ports","major":3,"minMinor":0,"maxMinor":0}]})).unwrap()).unwrap();
    directory
}
async fn execute(node: &Headless, ws: &str, input: Value) -> Value {
    let answer = node
        .request(ws, DRIVER, "proof.execute", input.clone())
        .await;
    assert_eq!(answer["ok"], true, "{input}: {answer}");
    answer["value"].clone()
}
fn route(row: &Value, action: &str, port: &str, request: Value) -> Value {
    json!({"action":action,"access":"person","contract":row["contract"],"major":row["major"],"routeRevision":row["routeRevision"],"port":port,"request":request})
}

// Verifies: TEAM.SYS.PKG-R09, TEAM.SYS.PKG-R13, TEAM.SYS.TRANS-R06.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn independent_personal_install_remove_preserves_team_generations_and_document_bodies() {
    no_external_configuration();
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let application = stage(root.path());
    let desktop = Desktop::open(&application, &root.path().join("desktop")).await;
    let workspaces = [
        desktop.core.create_workspace("One").unwrap().id(),
        desktop.core.create_workspace("Two").unwrap().id(),
    ];
    desktop
        .host
        .execute(PackageHostCommand::ReconcileWorkspaces(
            ReconcileWorkspaces {},
        ))
        .await
        .unwrap();
    let a_path = root.path().join("a");
    let a = Headless::open(&application, &a_path, &desktop.core).await;
    let b = Headless::open(&application, &root.path().join("b"), &desktop.core).await;
    for ws in workspaces {
        a.join(&desktop.core, ws, "A").await;
        b.join(&desktop.core, ws, "B").await;
    }
    let a_generations = team_generations(&a).await;
    let b_generations = team_generations(&b).await;
    let directory = stage_driver(root.path());
    a.cli(
        &[
            "packages",
            "inspect",
            "--directory",
            directory.to_str().unwrap(),
        ],
        None,
    )
    .await;
    a.cli(
        &[
            "packages",
            "install",
            "--directory",
            directory.to_str().unwrap(),
            "--package",
            DRIVER,
            "--input",
            "-",
        ],
        Some(json!({"acceptedRisks":["filesystem","network","process"]})),
    )
    .await;
    b.cli(
        &[
            "packages",
            "install",
            "--directory",
            directory.to_str().unwrap(),
            "--package",
            DRIVER,
            "--input",
            "-",
        ],
        Some(json!({"acceptedRisks":["filesystem","network","process"]})),
    )
    .await;
    let mut documents = Vec::new();
    for ws in workspaces {
        let ws = ws.to_string();
        let collection = desktop
            .request(
                &ws,
                DOCUMENTS,
                "documents.collection.create",
                json!({"name":"Body qualification"}),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_headless(&a, &ws, DOCUMENTS, "documents.snapshot", json!({}), |v| {
            contains(v, "name", "Body qualification")
        })
        .await;
        let page = execute(&a, &ws, json!({"action":"page","access":"person"})).await;
        let row = page["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["contract"] == DOCUMENTS)
            .unwrap();
        execute(&a, &ws, route(row, "enable", "", json!({}))).await;
        let created = execute(
            &a,
            &ws,
            route(
                row,
                "command",
                "document.create",
                json!({"collectionId":collection,"title":"Canonical body"}),
            ),
        )
        .await;
        let id = created["documentId"].as_str().unwrap().to_owned();
        execute(&a,&ws,route(row,"command","document.revise",json!({"documentId":id,"baseVersion":created["version"],"markdown":"# Native mesh\n\nWritten through declared Person Ports.\n"}))).await;
        wait_headless(
            &b,
            &ws,
            DOCUMENTS,
            "documents.document.markdown",
            json!({"documentId":id,"chunkIndex":0}),
            |v| v["chunk"] == "# Native mesh\n\nWritten through declared Person Ports.\n",
        )
        .await;
        wait_desktop(
            &desktop,
            &ws,
            DOCUMENTS,
            "documents.document.markdown",
            json!({"documentId":id,"chunkIndex":0}),
            |v| v["chunk"] == "# Native mesh\n\nWritten through declared Person Ports.\n",
        )
        .await;
        let page = execute(&b, &ws, json!({"action":"page","access":"person"})).await;
        let row = page["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["contract"] == DOCUMENTS)
            .unwrap();
        execute(&b, &ws, route(row, "enable", "", json!({}))).await;
        let base = execute(
            &b,
            &ws,
            route(row, "query", "document.markdown", json!({"documentId":id})),
        )
        .await;
        execute(&b,&ws,route(row,"command","document.revise",json!({"documentId":id,"baseVersion":base["version"],"markdown":"# Native mesh\n\nRevised independently by B.\n"}))).await;
        wait_headless(
            &a,
            &ws,
            DOCUMENTS,
            "documents.document.markdown",
            json!({"documentId":id,"chunkIndex":0}),
            |v| v["chunk"] == "# Native mesh\n\nRevised independently by B.\n",
        )
        .await;
        wait_desktop(
            &desktop,
            &ws,
            DOCUMENTS,
            "documents.document.markdown",
            json!({"documentId":id,"chunkIndex":0}),
            |v| v["chunk"] == "# Native mesh\n\nRevised independently by B.\n",
        )
        .await;
        documents.push((ws, id));
    }
    assert_eq!(team_generations(&a).await, a_generations);
    a.cli(&["packages", "remove", "--package", DRIVER], None)
        .await;
    b.cli(&["packages", "remove", "--package", DRIVER], None)
        .await;
    assert_eq!(team_generations(&a).await, a_generations);
    assert_eq!(team_generations(&b).await, b_generations);
    assert!(
        a.call(teamarness_headless::control::Operation::PackagesList {})
            .await
            .to_string()
            .find(DRIVER)
            .is_none()
    );
    for (ws, id) in &documents {
        a.request(
            ws,
            DOCUMENTS,
            "documents.comment.add",
            json!({"documentId":id,"markdown":"After Personal removal"}),
        )
        .await;
        wait_headless(
            &b,
            ws,
            DOCUMENTS,
            "documents.comments",
            json!({"documentId":id}),
            |v| contains(v, "markdown", "After Personal removal"),
        )
        .await;
        let epic = a
            .request(
                ws,
                BACKLOG,
                "backlog.epic.create",
                json!({"title":"After removal epic"}),
            )
            .await;
        let ticket = a.request(ws,BACKLOG,"backlog.ticket.create",json!({"epicId":epic["epicId"],"title":"After removal ticket","body":"Still synchronizing","priority":"low"})).await;
        wait_headless(
            &b,
            ws,
            BACKLOG,
            "backlog.ticket.get",
            json!({"ticketId":ticket["ticketId"]}),
            |v| v["body"] == "Still synchronizing",
        )
        .await;
        b.request(
            ws,
            CONVERSATIONS,
            "conversations.message.post",
            json!({"channelId":GENERAL,"markdown":"No Personal dependency"}),
        )
        .await;
        wait_headless(
            &a,
            ws,
            CONVERSATIONS,
            "conversations.channel.open",
            json!({"channelId":GENERAL,"limit":64}),
            |v| contains(v, "markdown", "No Personal dependency"),
        )
        .await;
        wait_desktop(
            &desktop,
            ws,
            CONVERSATIONS,
            "conversations.channel.open",
            json!({"channelId":GENERAL,"limit":64}),
            |v| contains(v, "markdown", "No Personal dependency"),
        )
        .await;
    }
    assert_eq!(team_generations(&a).await, a_generations);
    a.stop().await;
    let a = Headless::open(&application, &a_path, &desktop.core).await;
    for (ws, id) in &documents {
        wait_headless(
            &a,
            ws,
            DOCUMENTS,
            "documents.document.markdown",
            json!({"documentId":id,"chunkIndex":0}),
            |v| v["chunk"] == "# Native mesh\n\nRevised independently by B.\n",
        )
        .await;
    }
    team_generations(&a).await;
    a.stop().await;
    b.stop().await;
    desktop.stop().await;
}
