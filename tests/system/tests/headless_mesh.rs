//! Real loopback mesh, not production Iroh or a desktop GUI observation.
#[path = "support/headless.rs"]
mod headless;
use headless::*;
use serde_json::json;
use teamarness_package_host::{PackageHostCommand, ReconcileWorkspaces};

// Verifies: TEAM.SYS.TRANS-R06, TEAM.SYS.MEM-R10, TEAM.SYS.CONV-R03.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn sync_only_two_headless_and_desktop_backends_cover_two_workspaces_offline_restart_and_late_join()
 {
    no_external_configuration();
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let application = stage(root.path());
    let desktop = Desktop::open(&application, &root.path().join("desktop")).await;
    let workspaces = [
        desktop
            .core
            .create_workspace_with_person_name("Planning", "Desktop")
            .unwrap()
            .id(),
        desktop
            .core
            .create_workspace_with_person_name("Delivery", "Desktop")
            .unwrap()
            .id(),
    ];
    desktop
        .host
        .execute(PackageHostCommand::ReconcileWorkspaces(
            ReconcileWorkspaces {},
        ))
        .await
        .unwrap();
    let a = Headless::open(&application, &root.path().join("a"), &desktop.core).await;
    for workspace in workspaces {
        a.join(&desktop.core, workspace, "Headless A").await;
    }
    let a_identity = a.cli(&["identity", "show"], None).await;
    assert_ne!(
        a_identity["installation"],
        desktop.core.public_identity().to_string()
    );
    let mut records = Vec::new();
    for workspace in workspaces {
        let ws = workspace.to_string();
        let collection = desktop
            .request(
                &ws,
                DOCUMENTS,
                "documents.collection.create",
                json!({"name":"Mesh collection"}),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let document = desktop
            .request(
                &ws,
                DOCUMENTS,
                "documents.document.create",
                json!({"collectionId":collection,"title":"Desktop document"}),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        desktop
            .request(
                &ws,
                DOCUMENTS,
                "documents.comment.add",
                json!({"documentId":document,"markdown":"Desktop document content"}),
            )
            .await;
        let epic = desktop
            .request(
                &ws,
                BACKLOG,
                "backlog.epic.create",
                json!({"title":"Mesh epic"}),
            )
            .await["epicId"]
            .as_str()
            .unwrap()
            .to_owned();
        let ticket = desktop.request(&ws, BACKLOG, "backlog.ticket.create", json!({"epicId":epic,"title":"Desktop ticket","body":"Desktop ticket body","priority":"low"})).await["ticketId"].as_str().unwrap().to_owned();
        desktop
            .request(
                &ws,
                CONVERSATIONS,
                "conversations.message.post",
                json!({"channelId":GENERAL,"markdown":"Desktop message"}),
            )
            .await;
        wait_headless(
            &a,
            &ws,
            DOCUMENTS,
            "documents.comments",
            json!({"documentId":document}),
            |v| contains(v, "markdown", "Desktop document content"),
        )
        .await;
        let original = wait_headless(
            &a,
            &ws,
            BACKLOG,
            "backlog.ticket.get",
            json!({"ticketId":ticket}),
            |v| v["body"] == "Desktop ticket body",
        )
        .await;
        wait_headless(
            &a,
            &ws,
            CONVERSATIONS,
            "conversations.channel.open",
            json!({"channelId":GENERAL,"limit":64}),
            |v| contains(v, "markdown", "Desktop message"),
        )
        .await;
        a.request(
            &ws,
            DOCUMENTS,
            "documents.document.rename",
            json!({"documentId":document,"title":"Headless A document"}),
        )
        .await;
        a.request(
            &ws,
            DOCUMENTS,
            "documents.comment.add",
            json!({"documentId":document,"markdown":"Headless A content"}),
        )
        .await;
        a.request(&ws, BACKLOG, "backlog.ticket.update", json!({"ticketId":ticket,"baseVersion":original["version"],"body":"Headless A ticket body"})).await;
        a.request(
            &ws,
            CONVERSATIONS,
            "conversations.message.post",
            json!({"channelId":GENERAL,"markdown":"Headless A message"}),
        )
        .await;
        wait_desktop(
            &desktop,
            &ws,
            DOCUMENTS,
            "documents.snapshot",
            json!({}),
            |v| contains(v, "title", "Headless A document"),
        )
        .await;
        wait_desktop(
            &desktop,
            &ws,
            DOCUMENTS,
            "documents.comments",
            json!({"documentId":document}),
            |v| contains(v, "markdown", "Headless A content"),
        )
        .await;
        wait_desktop(
            &desktop,
            &ws,
            BACKLOG,
            "backlog.ticket.get",
            json!({"ticketId":ticket}),
            |v| v["body"] == "Headless A ticket body",
        )
        .await;
        wait_desktop(
            &desktop,
            &ws,
            CONVERSATIONS,
            "conversations.channel.open",
            json!({"channelId":GENERAL,"limit":64}),
            |v| contains(v, "markdown", "Headless A message"),
        )
        .await;
        records.push((ws, document, ticket));
    }
    let generations = team_generations(&a).await;
    // B is a genuinely new Installation, admitted only after A's existing records converge.
    let b_path = root.path().join("b");
    let b = Headless::open(&application, &b_path, &desktop.core).await;
    for workspace in workspaces {
        b.join(&desktop.core, workspace, "Headless B").await;
    }
    let b_identity = b.cli(&["identity", "show"], None).await;
    assert_ne!(a_identity["installation"], b_identity["installation"]);
    for (ws, document, ticket) in &records {
        wait_headless(
            &b,
            ws,
            DOCUMENTS,
            "documents.comments",
            json!({"documentId":document}),
            |v| contains(v, "markdown", "Headless A content"),
        )
        .await;
        let current = wait_headless(
            &b,
            ws,
            BACKLOG,
            "backlog.ticket.get",
            json!({"ticketId":ticket}),
            |v| v["body"] == "Headless A ticket body",
        )
        .await;
        wait_headless(
            &b,
            ws,
            CONVERSATIONS,
            "conversations.channel.open",
            json!({"channelId":GENERAL,"limit":64}),
            |v| contains(v, "markdown", "Headless A message"),
        )
        .await;
        b.request(
            ws,
            DOCUMENTS,
            "documents.comment.add",
            json!({"documentId":document,"markdown":"Late join B content"}),
        )
        .await;
        b.request(
            ws,
            BACKLOG,
            "backlog.ticket.update",
            json!({"ticketId":ticket,"baseVersion":current["version"],"priority":"high"}),
        )
        .await;
        b.request(
            ws,
            CONVERSATIONS,
            "conversations.message.post",
            json!({"channelId":GENERAL,"markdown":"Late join B message"}),
        )
        .await;
        wait_headless(
            &a,
            ws,
            DOCUMENTS,
            "documents.comments",
            json!({"documentId":document}),
            |v| contains(v, "markdown", "Late join B content"),
        )
        .await;
        wait_headless(
            &a,
            ws,
            BACKLOG,
            "backlog.ticket.get",
            json!({"ticketId":ticket}),
            |v| v["priority"] == "high",
        )
        .await;
        wait_desktop(
            &desktop,
            ws,
            CONVERSATIONS,
            "conversations.channel.open",
            json!({"channelId":GENERAL,"limit":64}),
            |v| contains(v, "markdown", "Late join B message"),
        )
        .await;
    }
    b.stop().await;
    // A continues writing while B is offline. No observer lease keeps any Backend alive.
    for (ws, document, ticket) in &records {
        a.request(
            ws,
            DOCUMENTS,
            "documents.document.rename",
            json!({"documentId":document,"title":"Offline edit"}),
        )
        .await;
        let current = a
            .request(
                ws,
                BACKLOG,
                "backlog.ticket.get",
                json!({"ticketId":ticket}),
            )
            .await;
        a.request(ws,BACKLOG,"backlog.ticket.update",json!({"ticketId":ticket,"baseVersion":current["version"],"body":"Offline ticket body"})).await;
        a.request(
            ws,
            CONVERSATIONS,
            "conversations.message.post",
            json!({"channelId":GENERAL,"markdown":"Offline message"}),
        )
        .await;
    }
    assert_eq!(team_generations(&a).await, generations);
    let b = Headless::open(&application, &b_path, &desktop.core).await;
    assert_eq!(
        b.cli(&["identity", "show"], None).await["installation"],
        b_identity["installation"]
    );
    assert_eq!(
        b.cli(&["workspace", "list"], None)
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for (ws, document, ticket) in &records {
        wait_headless(&b, ws, DOCUMENTS, "documents.snapshot", json!({}), |v| {
            contains(v, "title", "Offline edit")
        })
        .await;
        wait_headless(
            &b,
            ws,
            BACKLOG,
            "backlog.ticket.get",
            json!({"ticketId":ticket}),
            |v| v["body"] == "Offline ticket body" && v["priority"] == "high",
        )
        .await;
        wait_headless(
            &b,
            ws,
            CONVERSATIONS,
            "conversations.channel.open",
            json!({"channelId":GENERAL,"limit":64}),
            |v| contains(v, "markdown", "Offline message"),
        )
        .await;
        // Durable late-join content survived, not just a successful startup/status.
        wait_headless(
            &b,
            ws,
            DOCUMENTS,
            "documents.comments",
            json!({"documentId":document}),
            |v| contains(v, "markdown", "Late join B content"),
        )
        .await;
    }
    team_generations(&b).await;
    assert!(
        desktop
            .host
            .snapshot()
            .await
            .instances
            .iter()
            .all(|instance| instance.mount_state.is_none())
    );
    b.stop().await;
    a.stop().await;
    desktop.stop().await;
}
