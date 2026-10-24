//! The reply harness: one background task per active Instance that answers when the Agent is
//! addressed.
//!
//! The loop subscribes to Conversations' `messages.changed`; on start and on every event it runs
//! `channel.list` and pages each Channel forward from its persisted mark ([`crate::positions`]).
//! A Message triggers a reply when a Person other than the Agent wrote it and its markdown
//! mentions the Agent's current name ([`crate::mention`]). Each Channel has at most one reply in
//! flight and a one-slot pending flag, so a burst of events during a reply collapses into one
//! more scan of that Channel once the reply is done. The mark moves past the trigger, and is
//! written, before the model is asked, so a crash or an uncertain post never yields a second
//! reply. Endpoint failures reach the status as Package-owned codes and are never retried.
//!
//! The task ends on cancellation (suspend, dispose) and when the status leaves the ready family
//! (configuration or binding lost); `AgentActive::sync_harness` starts it again when both are
//! back. A new task reads the marks back from disk, so a restart never re-answers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use teamarness_package_sdk::CancellationToken;
use teamarness_package_sdk::capabilities::agent_binding::BoundAgent;
use teamarness_package_sdk::capabilities::principals::{Principal, PrincipalKind, Principals};
use teamarness_package_sdk::ports::PortClient;
use tokio::task::{JoinHandle, JoinSet};

use crate::mention::mentions;
use crate::model::{ChatMessage, ModelClient, ModelFailure};
use crate::positions::{Positions, is_after};
use crate::settings::SettingsStore;
use crate::status::StatusCell;

/// The Conversations Port Contract this Package consumes.
pub const CONVERSATIONS_CONTRACT: &str = "dev.teamarness.conversations";
/// `message.post` was refused or its outcome is uncertain; the mark had already moved, so the
/// trigger is never answered again.
pub const POST_FAILED: &str = "agent.post_failed";
/// The marks could not be read or written; nothing is posted without a persisted mark.
pub const STORAGE_FAILED: &str = "agent.storage_failed";

/// Messages per `message.page` and pages per Channel per pass: at most 256 Messages are read
/// per Channel per pass, so a Channel that moved further is answered from its newest 256.
const PAGE_LIMIT: usize = 64;
const MAX_PAGES_PER_CHANNEL: usize = 4;
/// The Conversations `message.post` bound on `markdown`.
const MAX_REPLY_CHARS: usize = 32_768;
/// How long the loop waits before reopening a subscription that failed or closed.
const RETRY: Duration = Duration::from_secs(1);
/// How long a stopping harness waits for its in-flight replies to notice the token.
const DRAIN: Duration = Duration::from_secs(1);

/// What the harness loop is given when it starts.
///
/// The settings store is read on each reply, so a saved change applies to the next reply; the
/// key is read from the store only while a request is being built, through
/// `SettingsStore::api_key`.
#[derive(Clone)]
pub struct HarnessInputs {
    pub ports: PortClient,
    pub principals: Principals,
    pub settings: Arc<std::sync::Mutex<SettingsStore>>,
    pub status: Arc<StatusCell>,
    /// The Package Data Directory, where `positions.json` lives.
    pub data_directory: PathBuf,
}

/// One running background task and the token that stops it.
pub struct TaskHandle {
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

impl TaskHandle {
    /// Runs `work` under a fresh child of `parent`, so cancelling the parent stops it too.
    pub fn spawn<F, Fut>(parent: &CancellationToken, work: F) -> Self
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cancellation = parent.child();
        let task = tokio::spawn(work(cancellation.clone()));
        Self { cancellation, task }
    }

    /// Whether the task has already returned on its own.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Cancels the task and waits for it to finish; a task that ignores the token is aborted.
    pub async fn stop(self) {
        self.cancellation.cancel();
        let abort = self.task.abort_handle();
        if tokio::time::timeout(Duration::from_secs(2), self.task)
            .await
            .is_err()
        {
            abort.abort();
        }
    }
}

/// Starts the reply harness under `parent`.
pub fn start(parent: &CancellationToken, inputs: HarnessInputs) -> TaskHandle {
    TaskHandle::spawn(parent, move |cancellation| async move {
        run(inputs, cancellation).await;
    })
}

/// One Message as `message.page` shows it.
#[derive(Debug, Clone)]
struct Message {
    position: String,
    author: String,
    markdown: String,
}

/// The per-Channel reply discipline.
#[derive(Debug, Default)]
struct Slot {
    in_flight: bool,
    pending: bool,
}

/// How one reply task ended.
enum ReplyOutcome {
    Posted,
    Cancelled,
    Failed(&'static str, bool),
}

struct Loop {
    inputs: HarnessInputs,
    client: ModelClient,
    positions: Positions,
    slots: HashMap<String, Slot>,
    replies: JoinSet<(String, ReplyOutcome)>,
    /// Cancels every reply task when the loop ends for any reason.
    work: CancellationToken,
}

async fn run(inputs: HarnessInputs, cancellation: CancellationToken) {
    let client = match ModelClient::new() {
        Ok(client) => client,
        Err(failure) => {
            inputs.status.set_error(failure.code, failure.retryable);
            return;
        }
    };
    let positions = match Positions::open(&inputs.data_directory) {
        Ok(positions) => positions,
        Err(_) => {
            inputs.status.set_error(STORAGE_FAILED, true);
            return;
        }
    };
    let mut status = inputs.status.subscribe();
    let mut state = Loop {
        work: cancellation.child(),
        inputs,
        client,
        positions,
        slots: HashMap::new(),
        replies: JoinSet::new(),
    };
    'subscription: loop {
        if cancellation.is_cancelled() {
            break;
        }
        let mut events = match state
            .inputs
            .ports
            .subscribe_event(CONVERSATIONS_CONTRACT, "messages.changed")
            .await
        {
            Ok(events) => events,
            Err(_) => {
                if wait_or_cancel(&cancellation, RETRY).await {
                    break;
                }
                continue;
            }
        };
        state.pass().await;
        loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    events.close().await;
                    break 'subscription;
                }
                changed = status.changed() => {
                    if changed.is_err() || !state.inputs.status.is_ready_to_run() {
                        events.close().await;
                        break 'subscription;
                    }
                }
                joined = state.replies.join_next(), if !state.replies.is_empty() => {
                    if let Some(joined) = joined {
                        state.finished(joined).await;
                    }
                }
                next = events.recv() => match next {
                    Some(Ok(_)) => state.pass().await,
                    Some(Err(_)) | None => {
                        events.close().await;
                        break;
                    }
                },
            }
        }
        if wait_or_cancel(&cancellation, RETRY).await {
            break;
        }
    }
    state.shutdown().await;
}

/// Sleeps for `duration` unless cancelled first; `true` means cancelled.
async fn wait_or_cancel(cancellation: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        () = cancellation.cancelled() => true,
        () = tokio::time::sleep(duration) => false,
    }
}

impl Loop {
    /// One pass: list the Channels and scan each one that has no reply in flight.
    async fn pass(&mut self) {
        let Some(agent) = self.inputs.status.agent() else {
            return;
        };
        let Ok(list) = self
            .inputs
            .ports
            .query(CONVERSATIONS_CONTRACT, "channel.list", json!({}))
            .await
        else {
            return;
        };
        let channels = list["channels"]
            .as_array()
            .map(|channels| {
                channels
                    .iter()
                    .filter_map(|channel| channel["id"].as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let (context_messages, system_prompt) = {
            let store = self
                .inputs
                .settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let settings = store.settings();
            (
                settings.context_messages as usize,
                settings.system_prompt.clone(),
            )
        };
        let mut lookups = HashMap::new();
        for channel in channels {
            if self.slots.get(&channel).is_some_and(|slot| slot.in_flight) {
                self.slots.entry(channel).or_default().pending = true;
                continue;
            }
            self.scan(
                &agent,
                &channel,
                context_messages,
                &system_prompt,
                &mut lookups,
            )
            .await;
        }
    }

    /// Pages one Channel newest-first until the mark is passed and enough context is in hand.
    async fn scan(
        &mut self,
        agent: &BoundAgent,
        channel: &str,
        context_messages: usize,
        system_prompt: &str,
        lookups: &mut HashMap<String, Option<Principal>>,
    ) {
        let mark = self.positions.mark(channel).map(str::to_owned);
        let mut rows: Vec<Message> = Vec::new();
        let mut before: Option<String> = None;
        for _ in 0..MAX_PAGES_PER_CHANNEL {
            let mut request = json!({ "channelId": channel, "limit": PAGE_LIMIT });
            if let Some(before) = &before {
                request["before"] = json!(before);
            }
            let Ok(page) = self
                .inputs
                .ports
                .query(CONVERSATIONS_CONTRACT, "message.page", request)
                .await
            else {
                return;
            };
            rows.extend(
                page["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(parse_message),
            );
            let Some(mark) = &mark else {
                // First sight of this Channel: the head becomes the mark; history is never answered.
                let head = rows.first().map_or("", |row| row.position.as_str());
                if self.positions.advance(channel, head).is_err() {
                    self.inputs.status.set_error(STORAGE_FAILED, true);
                }
                return;
            };
            let older = rows
                .iter()
                .filter(|row| !is_after(&row.position, mark))
                .count();
            let next = page["nextBefore"].as_str().map(str::to_owned);
            if older >= context_messages.max(1) || next.is_none() {
                break;
            }
            before = next;
        }
        let Some(mark) = mark else {
            return;
        };
        let new = rows
            .iter()
            .filter(|row| is_after(&row.position, &mark))
            .collect::<Vec<_>>();
        let Some(head) = new.first().map(|row| row.position.clone()) else {
            return;
        };
        let mut trigger = None;
        for candidate in new.iter().rev() {
            if candidate.author == agent.identity {
                continue;
            }
            let Some(name) = agent.name.as_deref() else {
                continue;
            };
            if !mentions(&candidate.markdown, name) {
                continue;
            }
            let Ok(principal) = self.lookup(&candidate.author, lookups).await else {
                // The roster is unavailable: leave the mark so the next pass decides.
                return;
            };
            if principal.is_some_and(|principal| principal.kind == PrincipalKind::Person) {
                trigger = Some((*candidate).clone());
                break;
            }
        }
        let Some(trigger) = trigger else {
            if self.positions.advance(channel, &head).is_err() {
                self.inputs.status.set_error(STORAGE_FAILED, true);
            }
            return;
        };
        // Mark before anything else: from here on this Message is never answered twice.
        if self.positions.advance(channel, &trigger.position).is_err() {
            self.inputs.status.set_error(STORAGE_FAILED, true);
            return;
        }
        let mut prior = rows
            .iter()
            .filter(|row| is_after(&trigger.position, &row.position))
            .take(context_messages)
            .cloned()
            .collect::<Vec<_>>();
        prior.reverse();
        let mut messages = Vec::with_capacity(prior.len() + 2);
        messages.push(ChatMessage::system(system_message(agent, system_prompt)));
        for row in prior.iter().chain(std::iter::once(&trigger)) {
            if row.author == agent.identity {
                messages.push(ChatMessage::assistant(row.markdown.clone()));
                continue;
            }
            let label = match self.lookup(&row.author, lookups).await {
                Ok(principal) => label(principal.as_ref(), &row.author),
                Err(_) => label(None, &row.author),
            };
            messages.push(ChatMessage::user(format!("{label}: {}", row.markdown)));
        }
        self.spawn_reply(channel.to_owned(), messages);
    }

    /// Resolves an author key once per pass.
    async fn lookup(
        &self,
        author: &str,
        lookups: &mut HashMap<String, Option<Principal>>,
    ) -> Result<Option<Principal>, ()> {
        if let Some(found) = lookups.get(author) {
            return Ok(found.clone());
        }
        let found = self
            .inputs
            .principals
            .lookup(author)
            .await
            .map_err(|_| ())?;
        lookups.insert(author.to_owned(), found.clone());
        Ok(found)
    }

    fn spawn_reply(&mut self, channel: String, messages: Vec<ChatMessage>) {
        let slot = self.slots.entry(channel.clone()).or_default();
        slot.in_flight = true;
        // Whatever arrives while this reply is in flight is looked at once it is done.
        slot.pending = true;
        if self.replies.is_empty() {
            self.inputs.status.set_replying();
        }
        let client = self.client.clone();
        let ports = self.inputs.ports.clone();
        let settings = self.inputs.settings.clone();
        let cancellation = self.work.clone();
        self.replies.spawn(async move {
            let outcome = reply(
                &client,
                &ports,
                &settings,
                &channel,
                messages,
                &cancellation,
            )
            .await;
            (channel, outcome)
        });
    }

    /// A reply task ended: publish its outcome and rescan its Channel if events were pending.
    async fn finished(&mut self, joined: Result<(String, ReplyOutcome), tokio::task::JoinError>) {
        let Ok((channel, outcome)) = joined else {
            return;
        };
        let pending = {
            let slot = self.slots.entry(channel.clone()).or_default();
            slot.in_flight = false;
            std::mem::take(&mut slot.pending)
        };
        match outcome {
            ReplyOutcome::Posted => {
                if self.replies.is_empty() {
                    self.inputs.status.set_idle();
                }
            }
            ReplyOutcome::Cancelled => {
                if self.replies.is_empty() {
                    self.inputs.status.set_idle_if_replying();
                }
            }
            ReplyOutcome::Failed(code, retryable) => self.inputs.status.set_error(code, retryable),
        }
        if pending && !self.work.is_cancelled() {
            let Some(agent) = self.inputs.status.agent() else {
                return;
            };
            let (context_messages, system_prompt) = {
                let store = self
                    .inputs
                    .settings
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let settings = store.settings();
                (
                    settings.context_messages as usize,
                    settings.system_prompt.clone(),
                )
            };
            let mut lookups = HashMap::new();
            self.scan(
                &agent,
                &channel,
                context_messages,
                &system_prompt,
                &mut lookups,
            )
            .await;
        }
    }

    /// Cancels the reply tasks and gives them a moment to notice before aborting.
    async fn shutdown(&mut self) {
        self.work.cancel();
        let drain = async {
            while let Some(joined) = self.replies.join_next().await {
                self.finished(joined).await;
            }
        };
        if tokio::time::timeout(DRAIN, drain).await.is_err() {
            self.replies.shutdown().await;
        }
        self.inputs.status.set_idle_if_replying();
    }
}

fn parse_message(row: &Value) -> Option<Message> {
    Some(Message {
        position: row["position"].as_str()?.to_owned(),
        author: row["author"].as_str()?.to_owned(),
        markdown: row["markdown"].as_str()?.to_owned(),
    })
}

/// The system message: who the Agent is, how the transcript is labelled, then the Steward's
/// prompt.
fn system_message(agent: &BoundAgent, system_prompt: &str) -> String {
    let name = agent.name.as_deref().unwrap_or("the Agent");
    let mut text = format!(
        "You are {name}, an Agent in a team Workspace. A Person addressed you by name in a \
         Channel; reply to them. Earlier messages are shown oldest first, each prefixed with \
         its author's name and kind in the form `Name (person): text`. Answer with the reply \
         text only, in Markdown."
    );
    let prompt = system_prompt.trim();
    if !prompt.is_empty() {
        text.push_str("\n\n");
        text.push_str(prompt);
    }
    text
}

/// `Name (kind)` for a resolved Principal; a key prefix for one the roster does not know.
fn label(principal: Option<&Principal>, author: &str) -> String {
    match principal {
        Some(principal) => {
            let kind = match principal.kind {
                PrincipalKind::Person => "person",
                PrincipalKind::Agent => "agent",
            };
            let name = principal
                .name
                .as_deref()
                .unwrap_or(principal.fingerprint.as_str());
            format!("{name} ({kind})")
        }
        None => format!("{} (unknown)", &author[..author.len().min(8)]),
    }
}

/// One reply: read the key, ask the model once, post once. The key lives in this task only.
async fn reply(
    client: &ModelClient,
    ports: &PortClient,
    settings: &std::sync::Mutex<SettingsStore>,
    channel: &str,
    messages: Vec<ChatMessage>,
    cancellation: &CancellationToken,
) -> ReplyOutcome {
    let (endpoint, model, key) = {
        let store = settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let settings = store.settings();
        (
            settings.endpoint_base_url.clone(),
            settings.model.clone(),
            store.api_key(),
        )
    };
    let Ok(Some(key)) = key else {
        // The key went away between the status check and this reply.
        return ReplyOutcome::Failed(crate::model::ENDPOINT_REJECTED, false);
    };
    let completion = tokio::select! {
        () = cancellation.cancelled() => return ReplyOutcome::Cancelled,
        completion = client.complete(&endpoint, &model, &messages, &key) => completion,
    };
    drop(key);
    let markdown = match completion {
        Ok(text) => truncate_chars(text, MAX_REPLY_CHARS),
        Err(ModelFailure { code, retryable }) => return ReplyOutcome::Failed(code, retryable),
    };
    if markdown.trim().is_empty() {
        return ReplyOutcome::Failed(crate::model::MODEL_ERROR, false);
    }
    let post = tokio::select! {
        () = cancellation.cancelled() => return ReplyOutcome::Cancelled,
        post = ports.command(
            CONVERSATIONS_CONTRACT,
            "message.post",
            json!({ "channelId": channel, "markdown": markdown }),
        ) => post,
    };
    match post {
        Ok(_) => ReplyOutcome::Posted,
        // Refused or uncertain alike: the mark has moved, so this trigger is settled.
        Err(_) => ReplyOutcome::Failed(POST_FAILED, false),
    }
}

fn truncate_chars(text: String, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => text[..index].to_owned(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(kind: PrincipalKind, name: Option<&str>) -> Principal {
        Principal {
            principal_id: "a".repeat(64),
            kind,
            steward_principal_id: None,
            keys: vec!["b".repeat(64)],
            name: name.map(str::to_owned),
            fingerprint: "bbbbbbbb".into(),
        }
    }

    #[test]
    fn labels_and_system_message() {
        assert_eq!(
            label(Some(&principal(PrincipalKind::Person, Some("Alice"))), "x"),
            "Alice (person)"
        );
        assert_eq!(
            label(Some(&principal(PrincipalKind::Agent, None)), "x"),
            "bbbbbbbb (agent)"
        );
        assert_eq!(label(None, &"c".repeat(64)), "cccccccc (unknown)");
        let agent = BoundAgent {
            principal_id: "a".repeat(64),
            identity: "b".repeat(64),
            name: Some("Scout".into()),
        };
        let text = system_message(&agent, "  Be brief.  ");
        assert!(text.starts_with("You are Scout, an Agent"));
        assert!(text.ends_with("\n\nBe brief."));
        assert_eq!(truncate_chars("héllo".into(), 2), "hé");
        assert_eq!(truncate_chars("hi".into(), 5), "hi");
    }
}
