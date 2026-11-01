//! Backlog Port Contract 1.0 (`dev.teamarness.backlog`, `ports.json`): another Package, usually
//! an Agent, lists Epics and Tickets, reads a Ticket, creates one in an existing Epic, changes
//! its title, body, status or priority against its version, observes changes, and reads the
//! `runtime.guide`. This is the agent surface.
//!
//! Commands are written through the same [`append_as`] path, per-Ticket lock and `baseVersion`
//! check as the UI's requests. A bound Agent's record names the Agent's signing key as its
//! author and is appended with its Principal ID, so Core signs it with that key or refuses
//! (ADR 0030). Epic operations, assign, move, archive and restore are UI requests only: they
//! are absent from `ports.json`, and the closed request schemas refuse their fields.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use teamarness_package_sdk::ports::PortActor;
use teamarness_package_sdk::{BackendError, Context, EventSubscription, event_channel};

#[cfg(doc)]
use super::append_as;
use super::{
    Author, BacklogActive, UNTITLED, error, not_found, parse_priority, parse_status, storage_failed,
};

pub(super) const CONTRACT: &str = "dev.teamarness.backlog";

/// The most Epics, and the most Tickets, one `ticket.list` page holds.
pub(crate) const MAX_PAGE: usize = 64;
/// The Host's bound on one Port value, `MAX_UI_PAYLOAD_BYTES` in the Package contract crate,
/// which a test pins this to. A Package depends only on the SDK and the protocol (ARC-10), so
/// the bound is restated here rather than imported.
pub(crate) const PORT_PAYLOAD_BYTES: usize = 256 * 1024;
/// The longest Order Key a cursor carries.
const MAX_ORDER_KEY: usize = 128;

fn invalid_cursor() -> BackendError {
    error("backlog.invalid_cursor", false, json!({}))
}

/// Turns the Host-authored actor into the author a command is written as.
///
/// A Person must be this Installation. An Agent actor is Host-derived from the calling Package
/// Instance's Agent Binding (ADR 0030), so it is accepted as given: the record names the Agent's
/// key, and Core signs with that key or refuses.
pub(super) fn authorize_actor(
    context: &Context<BacklogActive>,
    actor: PortActor,
) -> Result<Author, BackendError> {
    match actor {
        PortActor::Person { identity } if identity == context.state().installation => Ok(Author {
            identity,
            principal: None,
        }),
        PortActor::Agent {
            principal,
            identity,
        } => Ok(Author {
            identity,
            principal: Some(principal),
        }),
        PortActor::Person { .. } => Err(error("backlog.actor_unauthorized", false, json!({}))),
    }
}

// ---------------------------------------------------------------- ticket.list

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListRequest {
    #[serde(default)]
    epic_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    include_archived: bool,
    #[serde(default)]
    cursor: Option<String>,
}

/// Where a listing stands: Epics come before Tickets, each in creation order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Position {
    Epic(String),
    Ticket(String),
}

impl Position {
    fn encode(&self) -> String {
        match self {
            Self::Epic(order) => format!("epic:{order}"),
            Self::Ticket(order) => format!("ticket:{order}"),
        }
    }

    /// Reads a cursor strictly; anything [`Position::encode`] cannot produce is `None`.
    fn decode(cursor: &str) -> Option<Self> {
        let (kind, order) = cursor.split_once(':')?;
        if order.is_empty()
            || order.len() > MAX_ORDER_KEY
            || !order
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        match kind {
            "epic" => Some(Self::Epic(order.to_owned())),
            "ticket" => Some(Self::Ticket(order.to_owned())),
            _ => None,
        }
    }
}

/// One page of Epics and Ticket summaries, without bodies.
///
/// The cursor names the creation Order Key of the last entry listed, so a page continues
/// after it even if that entry has changed or been filtered out since.
pub(super) async fn list(
    context: &Context<BacklogActive>,
    request: ListRequest,
) -> Result<JsonValue, BackendError> {
    let status = parse_status(request.status.as_deref())?;
    let priority = parse_priority(request.priority.as_deref())?;
    let after = request
        .cursor
        .as_deref()
        .map(|cursor| Position::decode(cursor).ok_or_else(invalid_cursor))
        .transpose()?;
    let state = context.state().clone();
    let (epics, tickets) = context
        .blocking(move |_| {
            Ok((
                state.store.epics().map_err(storage_failed)?,
                state.store.tickets().map_err(storage_failed)?,
            ))
        })
        .await?;
    if let Some(epic_id) = &request.epic_id
        && !epics.iter().any(|epic| epic.id == *epic_id)
    {
        return Err(not_found(epic_id));
    }
    let archived_epics: BTreeSet<&str> = epics
        .iter()
        .filter(|epic| epic.archived)
        .map(|epic| epic.id.as_str())
        .collect();
    let in_scope = |epic_id: &str| request.epic_id.as_deref().is_none_or(|id| id == epic_id);
    let mut entries: BTreeMap<Position, JsonValue> = BTreeMap::new();
    for epic in &epics {
        if in_scope(&epic.id) && (request.include_archived || !epic.archived) {
            entries.insert(
                Position::Epic(epic.create_order.clone()),
                json!({
                    "epicId": epic.id,
                    "title": epic.title.as_deref().unwrap_or(UNTITLED),
                    "archived": epic.archived,
                }),
            );
        }
    }
    for ticket in &tickets {
        let visible = request.include_archived
            || !(ticket.archived || archived_epics.contains(ticket.epic_id.as_str()));
        if in_scope(&ticket.epic_id)
            && visible
            && status.is_none_or(|status| status == ticket.status)
            && priority.is_none_or(|priority| priority == ticket.priority)
        {
            entries.insert(
                Position::Ticket(ticket.create_order.clone()),
                json!({
                    "ticketId": ticket.id,
                    "epicId": ticket.epic_id,
                    "title": ticket.title.as_deref().unwrap_or(UNTITLED),
                    "status": ticket.status.as_str(),
                    "priority": ticket.priority.as_str(),
                    "assignee": ticket.assignee,
                    "archived": ticket.archived,
                    "version": ticket.version,
                }),
            );
        }
    }
    let remaining: Vec<(&Position, &JsonValue)> = match &after {
        None => entries.iter().collect(),
        Some(after) => entries
            .range((std::ops::Bound::Excluded(after), std::ops::Bound::Unbounded))
            .collect(),
    };
    let mut end = page_end(&remaining);
    loop {
        let page = &remaining[..end];
        let mut value = json!({
            "epics": page.iter().filter(|(position, _)| matches!(position, Position::Epic(_)))
                .map(|(_, value)| *value).collect::<Vec<_>>(),
            "tickets": page.iter().filter(|(position, _)| matches!(position, Position::Ticket(_)))
                .map(|(_, value)| *value).collect::<Vec<_>>(),
        });
        if end < remaining.len()
            && let Some((position, _)) = page.last()
        {
            value["nextCursor"] = json!(position.encode());
        }
        // A page shrinks rather than exceed the Port value bound; one entry always fits.
        if end <= 1 || value.to_string().len() <= PORT_PAYLOAD_BYTES {
            return Ok(value);
        }
        end -= 1;
    }
}

/// How many leading entries fit [`MAX_PAGE`] Epics and [`MAX_PAGE`] Tickets.
fn page_end(entries: &[(&Position, &JsonValue)]) -> usize {
    let (mut epics, mut tickets) = (0, 0);
    for (index, (position, _)) in entries.iter().enumerate() {
        let count = match position {
            Position::Epic(_) => &mut epics,
            Position::Ticket(_) => &mut tickets,
        };
        if *count == MAX_PAGE {
            return index;
        }
        *count += 1;
    }
    entries.len()
}

// ---------------------------------------------------------------- backlog.changed

/// Live-only and coalescing, as Conversations `messages.changed`: one empty event waits at a
/// time, and every change meanwhile folds into it. There is no initial event.
pub(super) fn changed(context: &Context<BacklogActive>) -> Result<EventSubscription, BackendError> {
    let (sender, subscription) = event_channel(1);
    let mut revisions = context.state().changes.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = revisions.changed() => {
                    if result.is_err() || sender.send(json!({})).await.is_err() { break; }
                }
                () = sender.closed() => break,
            }
        }
    });
    Ok(subscription)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use teamarness_package_contract::port_contract::{
        CompiledPortContracts, compile_port_contracts,
    };
    use teamarness_package_sdk::testing::Harness;

    use super::super::tests::{
        ALICE, Ledger, Session, agent, agent_key, epic, get, hex, next_snapshot, request, start,
        ticket,
    };
    use super::*;
    use crate::domain::{DomainRecord, MAX_BODY, MAX_TITLE, Priority, Status};

    /// The digest of the 1.0 contract projection, pinned so 1.0 never changes by accident.
    const CONTRACT_1_0_SHA256: &str =
        "e34faf66ab93eb4a0abb134371a67b09d9b43a6435802221d48295043a6748ff";

    fn contract() -> CompiledPortContracts {
        compile_port_contracts(include_bytes!("../../ports.json")).unwrap()
    }

    fn agent_actor() -> JsonValue {
        json!({ "kind": "agent", "principal": agent(), "identity": agent_key() })
    }

    fn person_actor() -> JsonValue {
        json!({ "kind": "person", "identity": ALICE })
    }

    /// Calls one provided Port as `actor` and unwraps the Port value.
    async fn port_as(
        harness: &mut Harness,
        kind: &str,
        port: &str,
        actor: JsonValue,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        harness
            .request(
                &format!("teamarness.port.{kind}.v1"),
                json!({
                    "contract": CONTRACT, "major": 1, "minor": 0,
                    "port": port, "actor": actor, "request": request,
                }),
            )
            .await
            .map(|value| value["$teamarnessPortValue"].clone())
    }

    async fn agent_query(session: &mut Session, port: &str, request: JsonValue) -> JsonValue {
        port_as(&mut session.harness, "query", port, agent_actor(), request)
            .await
            .unwrap_or_else(|failure| panic!("{port}: {}", failure.code()))
    }

    async fn agent_command(
        session: &mut Session,
        port: &str,
        request: JsonValue,
    ) -> Result<JsonValue, BackendError> {
        port_as(
            &mut session.harness,
            "command",
            port,
            agent_actor(),
            request,
        )
        .await
    }

    fn appends(session: &Session) -> Vec<JsonValue> {
        session
            .log
            .recorded_calls()
            .into_iter()
            .filter(|call| call.method == "append")
            .map(|call| call.payload)
            .collect()
    }

    fn appended_author(payload: &JsonValue) -> String {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let canonical = URL_SAFE_NO_PAD
            .decode(payload["canonical"].as_str().unwrap())
            .unwrap();
        DomainRecord::decode(&canonical)
            .unwrap()
            .author()
            .to_owned()
    }

    // Verifies: TEAM.SYS.BACKLOG-R09 — the 1.0 contract is exactly the Ticket content Ports, the
    // guide and the change event; no Epic, assign, move, archive or restore Port exists.
    #[test]
    fn port_contract_1_0_is_pinned_and_offers_no_structural_ports() {
        let fixture =
            compile_port_contracts(include_bytes!("../tests/fixtures/ports-1.0.json")).unwrap();
        let current = contract();
        let pinned = fixture.project_metadata(CONTRACT, None).unwrap();
        assert_eq!(pinned.sha256, CONTRACT_1_0_SHA256);
        assert_eq!(
            current.project_metadata(CONTRACT, None).unwrap().bytes,
            pinned.bytes,
            "ports.json is the shipped 1.0 contract"
        );
        let provided = &current.contracts[0];
        assert_eq!(provided.identity, CONTRACT);
        assert_eq!((provided.major, provided.minor), (1, 0));
        let ports: BTreeSet<(String, String)> = provided
            .ports
            .iter()
            .map(|port| {
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
        let expected: BTreeSet<(String, String)> = [
            ("runtime.guide", "query"),
            ("ticket.list", "query"),
            ("ticket.get", "query"),
            ("ticket.create", "command"),
            ("ticket.update", "command"),
            ("backlog.changed", "event"),
        ]
        .into_iter()
        .map(|(name, kind)| (name.to_owned(), kind.to_owned()))
        .collect();
        assert_eq!(ports, expected);
        for (name, _) in &ports {
            for forbidden in ["epic.", "assign", "move", "archive", "restore"] {
                assert!(!name.contains(forbidden), "{name} offers {forbidden}");
            }
        }
        assert!(current.warnings.is_empty(), "{:?}", current.warnings);
        // The contract and every Port open with a sentence that stands on its own.
        for description in std::iter::once(&provided.description)
            .chain(provided.ports.iter().map(|port| &port.description))
        {
            let first = description.split(". ").next().unwrap();
            assert!(first.len() >= 24, "{description}");
            assert!(first.starts_with(char::is_uppercase), "{description}");
        }
    }

    // Verifies: TEAM.SYS.BACKLOG-R09 — closed schemas refuse an assignee, a new Epic and any
    // unknown field before dispatch.
    #[test]
    fn closed_schemas_refuse_assignee_epic_and_unknown_fields() {
        let contract = contract();
        let check = |port: &str, value: JsonValue| {
            contract.validate(CONTRACT, port, "request", &value).is_ok()
        };
        let ticket_id = hex(64, 1);
        let base = hex(82, 1);
        assert!(check(
            "ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": base, "status": "done" })
        ));
        for refused in [
            json!({ "ticketId": ticket_id, "baseVersion": base, "assignee": hex(64, 2) }),
            json!({ "ticketId": ticket_id, "baseVersion": base, "assignee": null }),
            json!({ "ticketId": ticket_id, "baseVersion": base, "epicId": hex(64, 2) }),
            json!({ "ticketId": ticket_id, "baseVersion": base, "archived": true }),
            json!({ "ticketId": ticket_id, "baseVersion": base, "author": "forged" }),
            json!({ "ticketId": ticket_id, "status": "done" }),
            json!({ "ticketId": ticket_id, "baseVersion": base, "status": "blocked" }),
        ] {
            assert!(!check("ticket.update", refused.clone()), "{refused}");
        }
        assert!(check(
            "ticket.create",
            json!({ "epicId": hex(64, 2), "title": "Plan", "priority": "high" })
        ));
        for refused in [
            json!({ "epicId": hex(64, 2), "title": "Plan", "assignee": hex(64, 3) }),
            json!({ "epicId": hex(64, 2), "title": "Plan", "archived": false }),
            json!({ "epicId": hex(64, 2), "title": "" }),
            json!({ "title": "Plan" }),
        ] {
            assert!(!check("ticket.create", refused.clone()), "{refused}");
        }
        assert!(check("ticket.list", json!({})));
        assert!(!check("ticket.list", json!({ "limit": 10 })));
        assert!(!check(
            "ticket.get",
            json!({ "ticketId": ticket_id, "body": true })
        ));
        assert!(!check("runtime.guide", json!({ "extra": 0 })));
        assert!(
            contract
                .validate(CONTRACT, "backlog.changed", "event", &json!({}))
                .is_ok()
        );
        assert!(
            contract
                .validate(
                    CONTRACT,
                    "backlog.changed",
                    "event",
                    &json!({ "ticketId": ticket_id })
                )
                .is_err()
        );
    }

    // Verifies: TEAM.SYS.BACKLOG-R09 — the guide conforms to `runtime.guide`, has its six
    // sections and states the enforced statuses, priorities, defaults and limits.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_guide_conforms_and_states_the_enforced_constants() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let guide = agent_query(&mut session, "runtime.guide", json!({})).await;
        let contract = contract();
        assert!(contract.has_guide(CONTRACT));
        assert!(
            contract
                .validate(CONTRACT, "runtime.guide", "response", &guide)
                .is_ok()
        );
        assert!(serde_json::to_vec(&guide).unwrap().len() <= 32 * 1024);
        let markdown = guide["markdown"].as_str().unwrap();
        let sections: Vec<&str> = markdown
            .lines()
            .filter_map(|line| line.strip_prefix("## "))
            .collect();
        assert_eq!(
            sections,
            [
                "What this is",
                "Statuses",
                "Priorities",
                "What you can do",
                "Saving",
                "Limits"
            ]
        );
        let mut advice = vec![
            format!("1 to {MAX_TITLE} characters"),
            format!("at most {MAX_BODY} bytes"),
            format!("at most {MAX_PAGE} of each per page"),
            format!("{} KiB", PORT_PAYLOAD_BYTES / 1024),
            "default to `not_ready` / `medium`".into(),
            "`high`, `medium`, `low`".into(),
            "pass its `version` as `baseVersion`".into(),
            "retry once".into(),
            "You cannot create, rename, archive or restore Epics".into(),
            "assign, move, archive or restore Tickets".into(),
            "- [ ]".into(),
        ];
        for status in Status::ALL {
            advice.push(format!(
                "`{}`: {}.",
                status.as_str(),
                super::super::guide::status_meaning(status)
            ));
        }
        for priority in Priority::ALL {
            advice.push(format!("`{}`", priority.as_str()));
        }
        for code in [
            "backlog.stale_version",
            "backlog.not_found",
            "backlog.epic_archived",
            "backlog.invalid",
            "backlog.too_large",
            "backlog.invalid_cursor",
            "backlog.actor_unauthorized",
            "backlog.unavailable",
            "outcomeUncertain",
        ] {
            advice.push(code.into());
        }
        for needle in advice {
            assert!(markdown.contains(&needle), "missing guidance: {needle}");
        }
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R09, TEAM.SYS.BACKLOG-R04 — an Agent creates and updates a
    // Ticket as itself, through the same version check as the UI, and the change is observed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn agent_creates_and_updates_tickets_as_itself_against_the_version() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let epic_id = epic(&mut session, "Launch").await;
        let mut events = session
            .harness
            .subscribe(
                "teamarness.port.event.v1",
                json!({
                    "contract": CONTRACT, "major": 1, "minor": 0, "port": "backlog.changed",
                    "actor": agent_actor(), "request": {},
                }),
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.next_event())
                .await
                .is_err(),
            "no initial event"
        );

        let created = agent_command(
            &mut session,
            "ticket.create",
            json!({ "epicId": epic_id, "title": "Write the plan", "body": "- [ ] outline" }),
        )
        .await
        .unwrap();
        let ticket_id = created["ticketId"].as_str().unwrap().to_owned();
        let calls = appends(&session);
        let call = calls.last().unwrap();
        assert_eq!(call["principal"], agent());
        assert_eq!(appended_author(call), agent_key());
        let read = agent_query(&mut session, "ticket.get", json!({ "ticketId": ticket_id })).await;
        assert!(
            contract()
                .validate(CONTRACT, "ticket.get", "response", &read)
                .is_ok(),
            "{read}"
        );
        assert_eq!(read["creator"], agent_key());
        assert_eq!(read["status"], "not_ready");
        assert_eq!(read["priority"], "medium");
        assert_eq!(read["version"], created["version"]);
        let event = tokio::time::timeout(Duration::from_secs(2), events.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(event["$teamarnessPortValue"], json!({}));

        // A Person saves first; the Agent's save on the old version is refused with the
        // current Ticket and writes nothing.
        let person_save = request(
            &mut session,
            "backlog.ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": read["version"], "priority": "high" }),
        )
        .await;
        let before = appends(&session).len();
        let stale = agent_command(
            &mut session,
            "ticket.update",
            json!({ "ticketId": ticket_id, "baseVersion": read["version"], "status": "ready" }),
        )
        .await
        .unwrap_err();
        assert_eq!(stale.code(), "backlog.stale_version");
        assert!(!stale.outcome_uncertain());
        assert_eq!(stale.details()["ticket"]["version"], person_save["version"]);
        assert_eq!(stale.details()["ticket"]["priority"], "high");
        assert_eq!(appends(&session).len(), before);

        let updated = agent_command(
            &mut session,
            "ticket.update",
            json!({
                "ticketId": ticket_id, "baseVersion": person_save["version"],
                "status": "in_progress", "body": "- [x] outline",
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            appended_author(appends(&session).last().unwrap()),
            agent_key()
        );
        let read = get(&mut session, &ticket_id).await;
        assert_eq!(read["version"], updated["version"]);
        assert_eq!(read["lastEditor"], agent_key());
        assert_eq!(read["creator"], agent_key());
        assert_eq!(read["status"], "in_progress");
        assert_eq!(read["priority"], "high", "the Person's change is kept");
        assert_eq!(read["body"], "- [x] outline");
        events.close().await;
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R09 — Port refusals use the Backlog error codes and write
    // nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn port_refusals_use_the_backlog_codes_and_write_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let epic_id = epic(&mut session, "Launch").await;
        let archived = epic(&mut session, "Old").await;
        request(
            &mut session,
            "backlog.epic.archive",
            json!({ "epicId": archived }),
        )
        .await;
        let (ticket_id, version) = ticket(&mut session, &epic_id, "Plan").await;
        let before = appends(&session).len();
        let unknown = hex(64, 7);
        let cases = [
            (
                "ticket.create",
                json!({ "epicId": unknown, "title": "Lost" }),
                "backlog.not_found",
            ),
            (
                "ticket.create",
                json!({ "epicId": archived, "title": "Late" }),
                "backlog.epic_archived",
            ),
            (
                "ticket.create",
                json!({ "epicId": epic_id, "title": "x".repeat(MAX_TITLE + 1) }),
                "backlog.too_large",
            ),
            (
                "ticket.create",
                json!({ "epicId": epic_id, "title": " padded" }),
                "backlog.invalid",
            ),
            (
                "ticket.create",
                json!({ "epicId": epic_id, "title": "Plan", "body": "é".repeat(MAX_BODY / 2 + 1) }),
                "backlog.too_large",
            ),
            (
                "ticket.update",
                json!({ "ticketId": unknown, "baseVersion": version, "status": "done" }),
                "backlog.not_found",
            ),
            (
                "ticket.update",
                json!({ "ticketId": ticket_id, "baseVersion": version }),
                "backlog.invalid",
            ),
            (
                "ticket.update",
                json!({ "ticketId": ticket_id, "baseVersion": hex(82, 1), "status": "done" }),
                "backlog.stale_version",
            ),
        ];
        for (port, payload, code) in cases {
            let failure = agent_command(&mut session, port, payload.clone())
                .await
                .unwrap_err();
            assert_eq!(failure.code(), code, "{port} {payload}");
            assert!(!failure.outcome_uncertain(), "{port} {payload}");
        }
        // The Backend refuses structural fields itself, too.
        for payload in [
            json!({ "ticketId": ticket_id, "baseVersion": version, "assignee": null }),
            json!({ "ticketId": ticket_id, "baseVersion": version, "epicId": archived }),
        ] {
            assert!(
                agent_command(&mut session, "ticket.update", payload)
                    .await
                    .is_err()
            );
        }
        for (port, payload, code) in [
            (
                "ticket.get",
                json!({ "ticketId": unknown }),
                "backlog.not_found",
            ),
            (
                "ticket.list",
                json!({ "epicId": unknown }),
                "backlog.not_found",
            ),
            (
                "ticket.list",
                json!({ "cursor": "elsewhere" }),
                "backlog.invalid_cursor",
            ),
            (
                "ticket.list",
                json!({ "cursor": "ticket:XYZ" }),
                "backlog.invalid_cursor",
            ),
        ] {
            let failure = port_as(&mut session.harness, "query", port, agent_actor(), payload)
                .await
                .unwrap_err();
            assert_eq!(failure.code(), code, "{port}");
        }
        // A Person must be this Installation; queries accept any Host-derived actor.
        let stranger = json!({ "kind": "person", "identity": "stranger" });
        let failure = port_as(
            &mut session.harness,
            "command",
            "ticket.create",
            stranger.clone(),
            json!({ "epicId": epic_id, "title": "Forged" }),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.code(), "backlog.actor_unauthorized");
        assert!(
            port_as(
                &mut session.harness,
                "query",
                "ticket.get",
                stranger,
                json!({ "ticketId": ticket_id })
            )
            .await
            .is_ok()
        );
        assert_eq!(appends(&session).len(), before);
        // The local Person writes as the Installation.
        let created = port_as(
            &mut session.harness,
            "command",
            "ticket.create",
            person_actor(),
            json!({ "epicId": epic_id, "title": "Mine" }),
        )
        .await
        .unwrap();
        let call = appends(&session).pop().unwrap();
        assert!(call.get("principal").is_none_or(JsonValue::is_null));
        assert_eq!(appended_author(&call), ALICE);
        assert_eq!(
            get(&mut session, created["ticketId"].as_str().unwrap()).await["creator"],
            ALICE
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R09 — `ticket.list` lists Epics and Ticket summaries without
    // bodies, filters, omits archived items by default, and pages with a cursor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ticket_list_filters_omits_archived_and_pages() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let launch = epic(&mut session, "Launch").await;
        let old = epic(&mut session, "Old").await;
        let (old_ticket, _) = ticket(&mut session, &old, "Old work").await;
        request(
            &mut session,
            "backlog.epic.archive",
            json!({ "epicId": old }),
        )
        .await;
        let mut created = Vec::new();
        for index in 0..(MAX_PAGE + 6) {
            let response = request(
                &mut session,
                "backlog.ticket.create",
                json!({
                    "epicId": launch, "title": format!("Task {index}"), "body": "secret body",
                    "priority": if index % 2 == 0 { "high" } else { "low" },
                }),
            )
            .await;
            created.push(response["ticketId"].as_str().unwrap().to_owned());
        }
        let (archived_ticket, version) = ticket(&mut session, &launch, "Dropped").await;
        request(
            &mut session,
            "backlog.ticket.archive",
            json!({ "ticketId": archived_ticket, "baseVersion": version }),
        )
        .await;
        let contract = contract();
        let first = agent_query(&mut session, "ticket.list", json!({})).await;
        assert!(
            contract
                .validate(CONTRACT, "ticket.list", "response", &first)
                .is_ok()
        );
        assert_eq!(
            first["epics"],
            json!([{ "epicId": launch, "title": "Launch", "archived": false }])
        );
        let tickets = first["tickets"].as_array().unwrap();
        assert_eq!(tickets.len(), MAX_PAGE);
        assert_eq!(tickets[0]["ticketId"], created[0]);
        assert_eq!(tickets[0]["title"], "Task 0");
        assert_eq!(tickets[0]["assignee"], JsonValue::Null);
        assert!(tickets.iter().all(|ticket| ticket.get("body").is_none()));
        assert!(!first.to_string().contains("secret body"));
        let second = agent_query(
            &mut session,
            "ticket.list",
            json!({ "cursor": first["nextCursor"] }),
        )
        .await;
        assert_eq!(second["epics"], json!([]));
        let rest: Vec<&JsonValue> = second["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ticket| &ticket["ticketId"])
            .collect();
        assert_eq!(rest, created[MAX_PAGE..].iter().collect::<Vec<_>>());
        assert!(second.get("nextCursor").is_none());

        let high = agent_query(&mut session, "ticket.list", json!({ "priority": "high" })).await;
        assert_eq!(
            high["tickets"].as_array().unwrap().len(),
            (MAX_PAGE + 6) / 2
        );
        assert_eq!(
            high["epics"].as_array().unwrap().len(),
            1,
            "Epics stay listed"
        );
        let none = agent_query(&mut session, "ticket.list", json!({ "status": "done" })).await;
        assert_eq!(none["tickets"], json!([]));

        let all = agent_query(
            &mut session,
            "ticket.list",
            json!({ "epicId": old, "includeArchived": true }),
        )
        .await;
        assert_eq!(
            all["epics"],
            json!([{ "epicId": old, "title": "Old", "archived": true }])
        );
        assert_eq!(all["tickets"][0]["ticketId"], old_ticket);
        assert_eq!(all["tickets"][0]["archived"], false);
        let hidden = agent_query(&mut session, "ticket.list", json!({ "epicId": old })).await;
        assert_eq!(hidden, json!({ "epics": [], "tickets": [] }));
        let archived = agent_query(
            &mut session,
            "ticket.list",
            json!({ "epicId": launch, "includeArchived": true, "cursor": first["nextCursor"] }),
        )
        .await;
        assert_eq!(
            archived["tickets"].as_array().unwrap().last().unwrap()["ticketId"],
            archived_ticket
        );
        session.harness.shutdown().await.unwrap();
    }

    // Verifies: TEAM.SYS.BACKLOG-R09 — every change sends at most one empty event, never more
    // events than changes, while the UI snapshot follows the same changes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn backlog_changed_is_empty_and_bounded_by_the_changes() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = start(directory.path(), &Ledger::default()).await;
        let mut events = session
            .harness
            .subscribe(
                "teamarness.port.event.v1",
                json!({
                    "contract": CONTRACT, "major": 1, "minor": 0, "port": "backlog.changed",
                    "actor": person_actor(), "request": {},
                }),
            )
            .await
            .unwrap();
        let mut changes = session
            .harness
            .subscribe("backlog.changes", json!({}))
            .await
            .unwrap();
        let epic_id = epic(&mut session, "Launch").await;
        let (ticket_id, _) = ticket(&mut session, &epic_id, "One").await;
        for index in 0..8 {
            ticket(&mut session, &epic_id, &format!("More {index}")).await;
        }
        let mut received = 0;
        while let Ok(event) =
            tokio::time::timeout(Duration::from_millis(200), events.next_event()).await
        {
            assert_eq!(event.unwrap().unwrap()["$teamarnessPortValue"], json!({}));
            received += 1;
        }
        assert!(
            (1..=10).contains(&received),
            "{received} events for ten changes"
        );
        let snapshot = next_snapshot(&mut changes, |event| {
            event["tickets"]
                .as_array()
                .is_some_and(|tickets| tickets.len() == 9)
        })
        .await;
        assert!(
            snapshot["tickets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["ticketId"] == ticket_id)
        );
        changes.close().await;
        events.close().await;
        session.harness.shutdown().await.unwrap();
    }

    #[test]
    fn cursors_round_trip_and_refuse_anything_else() {
        for position in [
            Position::Epic("0a".repeat(41)),
            Position::Ticket("f".into()),
        ] {
            assert_eq!(Position::decode(&position.encode()), Some(position));
        }
        for cursor in [
            "",
            "epic",
            "epic:",
            "ticket:XYZ",
            "ticket:ABCDEF",
            "note:00",
            " epic:00",
            &format!("epic:{}", "0".repeat(MAX_ORDER_KEY + 1)),
        ] {
            assert_eq!(Position::decode(cursor), None, "{cursor}");
        }
        assert!(Position::Epic("ff".into()) < Position::Ticket("00".into()));
    }

    #[test]
    fn the_port_payload_bound_is_the_hosts() {
        const {
            assert!(
                PORT_PAYLOAD_BYTES
                    == teamarness_package_contract::ui_contract::MAX_UI_PAYLOAD_BYTES
            )
        };
    }
}
