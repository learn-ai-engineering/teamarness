//! The pure Backlog projection: (records, order) to state, independent of arrival order.
//!
//! Every field converges on its own: a record changes a field only when its Order Key sorts
//! after the order that last set that field. A Ticket's version is the greatest Order Key of
//! any record applied to it, and its last editor and time come from that record (ADR 0039).
//! A record whose target is unknown waits, and is applied when the target's create record
//! arrives. A record whose author is not its signer is quarantined and never applied.
//!
//! Titles and bodies live only here, in memory, never in SQLite (ADR 0027).

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::{DomainRecord, Priority, Status};

/// The quarantine code of a record that waits for the record it targets.
pub(crate) const WAITING: &str = "waiting_for_target";
/// The quarantine code of a record whose `author` field is not its signer.
pub(crate) const SIGNER_MISMATCH: &str = "signer_mismatch";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicState {
    pub creator: String,
    pub create_order: String,
    pub create_at: u64,
    pub title_order: String,
    pub archived: bool,
    pub archive_order: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketState {
    pub epic: String,
    pub epic_order: String,
    pub status: Status,
    pub status_order: String,
    pub priority: Priority,
    pub priority_order: String,
    pub assignee: Option<String>,
    pub assignee_order: String,
    pub archived: bool,
    pub archive_order: String,
    pub title_order: String,
    pub body_order: String,
    /// The greatest Order Key of any record applied to this Ticket.
    pub version: String,
    pub creator: String,
    pub create_order: String,
    pub create_at: u64,
    /// The author of the record that set `version`.
    pub last_editor: String,
    /// The author-clock time of the record that set `version`.
    pub last_at: u64,
}

/// A remembered title or body, with the Order Key that set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub order: String,
    pub value: String,
}

/// Titles and bodies by Epic or Ticket id, each kept only when no later record set it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextMap {
    titles: BTreeMap<String, Text>,
    bodies: BTreeMap<String, Text>,
}

impl TextMap {
    pub fn title(&self, id: &str) -> Option<&str> {
        self.titles.get(id).map(|text| text.value.as_str())
    }

    pub fn body(&self, id: &str) -> Option<&str> {
        self.bodies.get(id).map(|text| text.value.as_str())
    }

    pub(crate) fn remember_title(&mut self, id: &str, order: &str, value: &str) {
        remember(&mut self.titles, id, order, value);
    }

    pub(crate) fn remember_body(&mut self, id: &str, order: &str, value: &str) {
        remember(&mut self.bodies, id, order, value);
    }
}

fn remember(texts: &mut BTreeMap<String, Text>, id: &str, order: &str, value: &str) {
    match texts.get(id) {
        Some(existing) if existing.order.as_str() >= order => {}
        _ => {
            texts.insert(
                id.to_owned(),
                Text {
                    order: order.to_owned(),
                    value: value.to_owned(),
                },
            );
        }
    }
}

/// A record held until its target arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Waiting {
    pub(crate) order: String,
    pub(crate) record: DomainRecord,
    pub(crate) target: String,
}

/// An Epic or Ticket whose state one apply changed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Touched {
    Epic(String),
    Ticket(String),
}

/// What one apply did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Applied, together with every waiting record the new target released.
    Applied {
        touched: BTreeSet<Touched>,
        /// Waiting records that applied because of this one.
        released: Vec<String>,
        /// Waiting records that now wait for another target.
        rewaiting: Vec<(String, String)>,
    },
    /// Waits for the Epic or Ticket named here.
    Waiting {
        target: String,
    },
    Quarantined(&'static str),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Projection {
    epics: BTreeMap<String, EpicState>,
    tickets: BTreeMap<String, TicketState>,
    texts: TextMap,
    waiting: BTreeMap<String, Waiting>,
}

/// Sets `field` and its order when `order` sorts after the order that last set it.
fn converge<T>(field: &mut T, field_order: &mut String, value: T, order: &str) {
    if order > field_order.as_str() {
        *field = value;
        order.clone_into(field_order);
    }
}

impl Projection {
    pub fn epic(&self, id: &str) -> Option<&EpicState> {
        self.epics.get(id)
    }

    pub fn ticket(&self, id: &str) -> Option<&TicketState> {
        self.tickets.get(id)
    }

    pub fn epics(&self) -> impl Iterator<Item = (&String, &EpicState)> {
        self.epics.iter()
    }

    pub fn tickets(&self) -> impl Iterator<Item = (&String, &TicketState)> {
        self.tickets.iter()
    }

    pub fn texts(&self) -> &TextMap {
        &self.texts
    }

    pub(crate) fn texts_mut(&mut self) -> &mut TextMap {
        &mut self.texts
    }

    /// The record ids waiting for a target, with that target.
    pub fn waiting(&self) -> impl Iterator<Item = (&String, &str)> {
        self.waiting
            .iter()
            .map(|(id, waiting)| (id, waiting.target.as_str()))
    }

    /// Restores persisted state; the store loads SQLite rows through these.
    pub(crate) fn restore_epic(&mut self, id: String, epic: EpicState) {
        self.epics.insert(id, epic);
    }

    pub(crate) fn restore_ticket(&mut self, id: String, ticket: TicketState) {
        self.tickets.insert(id, ticket);
    }

    /// Applies one record with its Order Key, signed by `signer`.
    ///
    /// The result depends only on the set of records applied, never on their arrival order.
    /// Applying the same record twice changes nothing.
    pub fn apply(
        &mut self,
        record_id: &str,
        order: &str,
        signer: &str,
        record: &DomainRecord,
    ) -> Outcome {
        if record.author() != signer {
            return Outcome::Quarantined(SIGNER_MISMATCH);
        }
        let mut touched = BTreeSet::new();
        if let Some(target) = self.apply_one(record_id, order, record, &mut touched) {
            self.waiting.insert(
                record_id.to_owned(),
                Waiting {
                    order: order.to_owned(),
                    record: record.clone(),
                    target: target.clone(),
                },
            );
            return Outcome::Waiting { target };
        }
        let mut released = Vec::new();
        let mut rewaiting = Vec::new();
        let mut created = created_id(record_id, record)
            .into_iter()
            .collect::<Vec<_>>();
        while let Some(target) = created.pop() {
            let mut ready = self
                .waiting
                .iter()
                .filter(|(_, waiting)| waiting.target == target)
                .map(|(id, waiting)| (waiting.order.clone(), id.clone()))
                .collect::<Vec<_>>();
            ready.sort();
            for (_, id) in ready {
                let Some(waiting) = self.waiting.remove(&id) else {
                    continue;
                };
                match self.apply_one(&id, &waiting.order, &waiting.record, &mut touched) {
                    Some(next) => {
                        rewaiting.push((id.clone(), next.clone()));
                        self.waiting.insert(
                            id,
                            Waiting {
                                target: next,
                                ..waiting
                            },
                        );
                    }
                    None => {
                        created.extend(created_id(&id, &waiting.record));
                        released.push(id);
                    }
                }
            }
        }
        rewaiting.retain(|(id, _)| !released.contains(id));
        Outcome::Applied {
            touched,
            released,
            rewaiting,
        }
    }

    /// Applies `record` when its targets exist; otherwise returns the first missing target.
    fn apply_one(
        &mut self,
        record_id: &str,
        order: &str,
        record: &DomainRecord,
        touched: &mut BTreeSet<Touched>,
    ) -> Option<String> {
        let at = record.at();
        let author = record.author();
        match record {
            DomainRecord::EpicCreate { title, .. } => {
                self.epics
                    .entry(record_id.to_owned())
                    .or_insert_with(|| EpicState {
                        creator: author.to_owned(),
                        create_order: order.to_owned(),
                        create_at: at,
                        title_order: order.to_owned(),
                        archived: false,
                        archive_order: order.to_owned(),
                    });
                self.texts.remember_title(record_id, order, title);
                touched.insert(Touched::Epic(record_id.to_owned()));
            }
            DomainRecord::EpicRename { epic, title, .. } => {
                let Some(state) = self.epics.get_mut(epic) else {
                    return Some(epic.clone());
                };
                if order > state.title_order.as_str() {
                    order.clone_into(&mut state.title_order);
                }
                self.texts.remember_title(epic, order, title);
                touched.insert(Touched::Epic(epic.clone()));
            }
            DomainRecord::EpicArchive { epic, .. } | DomainRecord::EpicRestore { epic, .. } => {
                let Some(state) = self.epics.get_mut(epic) else {
                    return Some(epic.clone());
                };
                let archived = matches!(record, DomainRecord::EpicArchive { .. });
                converge(
                    &mut state.archived,
                    &mut state.archive_order,
                    archived,
                    order,
                );
                touched.insert(Touched::Epic(epic.clone()));
            }
            DomainRecord::TicketCreate {
                epic,
                title,
                body,
                status,
                priority,
                ..
            } => {
                if !self.epics.contains_key(epic) {
                    return Some(epic.clone());
                }
                self.tickets
                    .entry(record_id.to_owned())
                    .or_insert_with(|| TicketState {
                        epic: epic.clone(),
                        epic_order: order.to_owned(),
                        status: *status,
                        status_order: order.to_owned(),
                        priority: *priority,
                        priority_order: order.to_owned(),
                        assignee: None,
                        assignee_order: order.to_owned(),
                        archived: false,
                        archive_order: order.to_owned(),
                        title_order: order.to_owned(),
                        body_order: order.to_owned(),
                        version: order.to_owned(),
                        creator: author.to_owned(),
                        create_order: order.to_owned(),
                        create_at: at,
                        last_editor: author.to_owned(),
                        last_at: at,
                    });
                self.texts.remember_title(record_id, order, title);
                self.texts.remember_body(record_id, order, body);
                touched.insert(Touched::Ticket(record_id.to_owned()));
            }
            DomainRecord::TicketUpdate {
                ticket, changes, ..
            } => {
                let Some(state) = self.tickets.get_mut(ticket) else {
                    return Some(ticket.clone());
                };
                if let Some(title) = &changes.title {
                    if order > state.title_order.as_str() {
                        order.clone_into(&mut state.title_order);
                    }
                    self.texts.remember_title(ticket, order, title);
                }
                if let Some(body) = &changes.body {
                    if order > state.body_order.as_str() {
                        order.clone_into(&mut state.body_order);
                    }
                    self.texts.remember_body(ticket, order, body);
                }
                if let Some(status) = changes.status {
                    converge(&mut state.status, &mut state.status_order, status, order);
                }
                if let Some(priority) = changes.priority {
                    converge(
                        &mut state.priority,
                        &mut state.priority_order,
                        priority,
                        order,
                    );
                }
                bump_version(state, order, author, at);
                touched.insert(Touched::Ticket(ticket.clone()));
            }
            DomainRecord::TicketAssign {
                ticket, assignee, ..
            } => {
                let Some(state) = self.tickets.get_mut(ticket) else {
                    return Some(ticket.clone());
                };
                converge(
                    &mut state.assignee,
                    &mut state.assignee_order,
                    assignee.clone(),
                    order,
                );
                bump_version(state, order, author, at);
                touched.insert(Touched::Ticket(ticket.clone()));
            }
            DomainRecord::TicketMove { ticket, epic, .. } => {
                if !self.tickets.contains_key(ticket) {
                    return Some(ticket.clone());
                }
                if !self.epics.contains_key(epic) {
                    return Some(epic.clone());
                }
                let Some(state) = self.tickets.get_mut(ticket) else {
                    return Some(ticket.clone());
                };
                converge(&mut state.epic, &mut state.epic_order, epic.clone(), order);
                bump_version(state, order, author, at);
                touched.insert(Touched::Ticket(ticket.clone()));
            }
            DomainRecord::TicketArchive { ticket, .. }
            | DomainRecord::TicketRestore { ticket, .. } => {
                let Some(state) = self.tickets.get_mut(ticket) else {
                    return Some(ticket.clone());
                };
                let archived = matches!(record, DomainRecord::TicketArchive { .. });
                converge(
                    &mut state.archived,
                    &mut state.archive_order,
                    archived,
                    order,
                );
                bump_version(state, order, author, at);
                touched.insert(Touched::Ticket(ticket.clone()));
            }
        }
        None
    }
}

/// Raises the version to `order` and takes the attribution from that record.
fn bump_version(state: &mut TicketState, order: &str, author: &str, at: u64) {
    if order > state.version.as_str() {
        order.clone_into(&mut state.version);
        author.clone_into(&mut state.last_editor);
        state.last_at = at;
    }
}

/// The Epic or Ticket a create record establishes.
fn created_id(record_id: &str, record: &DomainRecord) -> Option<String> {
    matches!(
        record,
        DomainRecord::EpicCreate { .. } | DomainRecord::TicketCreate { .. }
    )
    .then(|| record_id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TicketChanges;

    const ALICE: &str = "alice";
    const BOB: &str = "bob";
    const PERSON: &str = "9090909090909090909090909090909090909090909090909090909090909090";

    fn id(seed: u64) -> String {
        format!("{seed:064x}")
    }

    fn order(seed: u64) -> String {
        format!("{seed:082x}")
    }

    /// One record of a fixed set: its id seed, order seed, signer and content.
    struct Entry {
        id: String,
        order: String,
        signer: &'static str,
        record: DomainRecord,
    }

    fn entry(seed: u64, signer: &'static str, record: DomainRecord) -> Entry {
        Entry {
            id: id(seed),
            order: order(seed),
            signer,
            record,
        }
    }

    fn update(author: &str, seed_at: u64, ticket: u64, changes: TicketChanges) -> DomainRecord {
        DomainRecord::ticket_update(author, seed_at, &id(ticket), changes).unwrap()
    }

    fn epic(seed: u64, title: &str) -> Entry {
        entry(
            seed,
            ALICE,
            DomainRecord::epic_create(ALICE, seed, title).unwrap(),
        )
    }

    fn ticket(seed: u64, epic: u64, title: &str) -> Entry {
        entry(
            seed,
            ALICE,
            DomainRecord::ticket_create(
                ALICE,
                seed,
                &id(epic),
                title,
                "Body",
                Status::NotReady,
                Priority::Medium,
            )
            .unwrap(),
        )
    }

    fn permutations(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }
        let mut out = Vec::new();
        for rest in permutations(n - 1) {
            for at in 0..=rest.len() {
                let mut next = rest.clone();
                next.insert(at, n - 1);
                out.push(next);
            }
        }
        out
    }

    fn project(entries: &[Entry], sequence: &[usize]) -> Projection {
        let mut projection = Projection::default();
        for &index in sequence {
            let entry = &entries[index];
            projection.apply(&entry.id, &entry.order, entry.signer, &entry.record);
        }
        projection
    }

    /// Applies every permutation and asserts each converges to the in-order state, with no
    /// record left waiting. Returns that state.
    fn converges(entries: &[Entry]) -> Projection {
        let in_order = project(entries, &(0..entries.len()).collect::<Vec<_>>());
        assert_eq!(in_order.waiting().count(), 0);
        let all = permutations(entries.len());
        assert!(all.len() > 1);
        for sequence in all {
            let projection = project(entries, &sequence);
            assert_eq!(projection, in_order, "arrival order {sequence:?}");
            // Applying everything again changes nothing.
            let mut again = projection.clone();
            for entry in entries {
                again.apply(&entry.id, &entry.order, entry.signer, &entry.record);
            }
            assert_eq!(again, in_order);
        }
        in_order
    }

    // Verifies: TEAM.SYS.BACKLOG-R03 — concurrent changes to one field converge on the later
    // record in log order, and the version and attribution come from that record.
    #[test]
    fn concurrent_same_field_updates_converge_in_every_arrival_order() {
        let entries = [
            epic(1, "Launch"),
            ticket(2, 1, "Draft"),
            entry(
                3,
                ALICE,
                update(
                    ALICE,
                    30,
                    2,
                    TicketChanges {
                        title: Some("Alice's title".into()),
                        ..TicketChanges::default()
                    },
                ),
            ),
            entry(
                4,
                BOB,
                update(
                    BOB,
                    40,
                    2,
                    TicketChanges {
                        title: Some("Bob's title".into()),
                        ..TicketChanges::default()
                    },
                ),
            ),
        ];
        let state = converges(&entries);
        let ticket = state.ticket(&id(2)).unwrap();
        assert_eq!(state.texts().title(&id(2)), Some("Bob's title"));
        assert_eq!(ticket.title_order, order(4));
        assert_eq!(ticket.version, order(4));
        assert_eq!(ticket.last_editor, BOB);
        assert_eq!(ticket.last_at, 40);
        assert_eq!(ticket.creator, ALICE);
        assert_eq!(ticket.create_at, 2);
    }

    // Verifies: TEAM.SYS.BACKLOG-R03 — concurrent changes to different fields are all kept.
    #[test]
    fn concurrent_different_field_updates_are_all_kept() {
        let entries = [
            epic(1, "Launch"),
            ticket(2, 1, "Draft"),
            entry(
                3,
                ALICE,
                update(
                    ALICE,
                    30,
                    2,
                    TicketChanges {
                        body: Some("New body".into()),
                        ..TicketChanges::default()
                    },
                ),
            ),
            entry(
                4,
                BOB,
                update(
                    BOB,
                    40,
                    2,
                    TicketChanges {
                        status: Some(Status::InProgress),
                        ..TicketChanges::default()
                    },
                ),
            ),
            entry(
                5,
                ALICE,
                update(
                    ALICE,
                    50,
                    2,
                    TicketChanges {
                        priority: Some(Priority::High),
                        ..TicketChanges::default()
                    },
                ),
            ),
            entry(
                6,
                ALICE,
                DomainRecord::ticket_assign(ALICE, 60, &id(2), Some(PERSON)).unwrap(),
            ),
        ];
        let state = converges(&entries);
        let ticket = state.ticket(&id(2)).unwrap();
        assert_eq!(state.texts().title(&id(2)), Some("Draft"));
        assert_eq!(state.texts().body(&id(2)), Some("New body"));
        assert_eq!(ticket.status, Status::InProgress);
        assert_eq!(ticket.priority, Priority::High);
        assert_eq!(ticket.assignee.as_deref(), Some(PERSON));
        assert_eq!(ticket.version, order(6));
        assert_eq!(ticket.last_at, 60);
    }

    // Verifies: TEAM.SYS.BACKLOG-R02, R03 — a move and an update both survive, and a Ticket
    // always has exactly one Epic.
    #[test]
    fn a_move_and_an_update_both_survive() {
        let entries = [
            epic(1, "Launch"),
            epic(2, "Later"),
            ticket(3, 1, "Draft"),
            entry(
                4,
                BOB,
                DomainRecord::ticket_move(BOB, 40, &id(3), &id(2)).unwrap(),
            ),
            entry(
                5,
                ALICE,
                update(
                    ALICE,
                    50,
                    3,
                    TicketChanges {
                        title: Some("Renamed".into()),
                        ..TicketChanges::default()
                    },
                ),
            ),
        ];
        let state = converges(&entries);
        let ticket = state.ticket(&id(3)).unwrap();
        assert_eq!(ticket.epic, id(2));
        assert_eq!(ticket.epic_order, order(4));
        assert_eq!(state.texts().title(&id(3)), Some("Renamed"));
        assert_eq!(ticket.version, order(5));
        assert_eq!(ticket.last_editor, ALICE);
    }

    // Verifies: TEAM.SYS.BACKLOG-R01, R02 — archive and restore converge on the later record,
    // for Epics and Tickets alike.
    #[test]
    fn archive_and_restore_converge_on_the_later_record() {
        let entries = [
            epic(1, "Launch"),
            ticket(2, 1, "Draft"),
            entry(
                3,
                ALICE,
                DomainRecord::ticket_archive(ALICE, 30, &id(2)).unwrap(),
            ),
            entry(
                4,
                BOB,
                DomainRecord::ticket_restore(BOB, 40, &id(2)).unwrap(),
            ),
            entry(5, BOB, DomainRecord::epic_restore(BOB, 50, &id(1)).unwrap()),
            entry(
                6,
                ALICE,
                DomainRecord::epic_archive(ALICE, 60, &id(1)).unwrap(),
            ),
        ];
        let state = converges(&entries);
        let ticket = state.ticket(&id(2)).unwrap();
        assert!(!ticket.archived);
        assert_eq!(ticket.version, order(4));
        assert_eq!(ticket.last_editor, BOB);
        let epic = state.epic(&id(1)).unwrap();
        assert!(epic.archived);
        assert_eq!(epic.archive_order, order(6));
    }

    // Verifies: TEAM.SYS.BACKLOG-R03 — records that arrive before their target wait, then apply.
    #[test]
    fn records_before_their_target_wait_and_then_apply() {
        let entries = [
            epic(1, "Launch"),
            ticket(2, 1, "Draft"),
            entry(
                3,
                BOB,
                DomainRecord::epic_rename(BOB, 30, &id(1), "Launch v2").unwrap(),
            ),
            entry(
                4,
                BOB,
                update(
                    BOB,
                    40,
                    2,
                    TicketChanges {
                        body: Some("Later".into()),
                        ..TicketChanges::default()
                    },
                ),
            ),
            epic(5, "Other"),
            entry(
                6,
                ALICE,
                DomainRecord::ticket_move(ALICE, 60, &id(2), &id(5)).unwrap(),
            ),
        ];
        let state = converges(&entries);
        assert_eq!(state.texts().title(&id(1)), Some("Launch v2"));
        assert_eq!(state.texts().body(&id(2)), Some("Later"));
        assert_eq!(state.ticket(&id(2)).unwrap().epic, id(5));

        // Arriving alone, each dependant waits for its first missing target.
        let mut projection = Projection::default();
        let update = &entries[3];
        assert_eq!(
            projection.apply(&update.id, &update.order, update.signer, &update.record),
            Outcome::Waiting { target: id(2) }
        );
        let create = &entries[1];
        assert_eq!(
            projection.apply(&create.id, &create.order, create.signer, &create.record),
            Outcome::Waiting { target: id(1) }
        );
        let epic = &entries[0];
        let Outcome::Applied {
            touched, released, ..
        } = projection.apply(&epic.id, &epic.order, epic.signer, &epic.record)
        else {
            panic!("the Epic applies");
        };
        // The Epic released the Ticket create, which released the update.
        assert_eq!(released, vec![id(2), id(4)]);
        assert!(touched.contains(&Touched::Ticket(id(2))));
        assert_eq!(projection.waiting().count(), 0);
        assert_eq!(projection.texts().body(&id(2)), Some("Later"));
    }

    #[test]
    fn a_released_move_waits_again_for_its_destination() {
        let mut projection = Projection::default();
        let entries = [
            epic(1, "Launch"),
            ticket(2, 1, "Draft"),
            entry(
                3,
                ALICE,
                DomainRecord::ticket_move(ALICE, 30, &id(2), &id(9)).unwrap(),
            ),
        ];
        let movement = &entries[2];
        assert_eq!(
            projection.apply(&movement.id, &movement.order, ALICE, &movement.record),
            Outcome::Waiting { target: id(2) }
        );
        projection.apply(&entries[0].id, &entries[0].order, ALICE, &entries[0].record);
        let Outcome::Applied { rewaiting, .. } =
            projection.apply(&entries[1].id, &entries[1].order, ALICE, &entries[1].record)
        else {
            panic!("the Ticket applies");
        };
        assert_eq!(rewaiting, vec![(id(3), id(9))]);
        assert_eq!(
            projection.waiting().collect::<Vec<_>>(),
            vec![(&id(3), id(9).as_str())]
        );
        assert_eq!(projection.ticket(&id(2)).unwrap().epic, id(1));
    }

    // Verifies: TEAM.SYS.BACKLOG-R03 — a record whose author is not its signer is never applied.
    #[test]
    fn a_record_whose_author_is_not_its_signer_is_quarantined() {
        let mut projection = Projection::default();
        let create = epic(1, "Launch");
        assert_eq!(
            projection.apply(&create.id, &create.order, BOB, &create.record),
            Outcome::Quarantined(SIGNER_MISMATCH)
        );
        assert_eq!(projection, Projection::default());
        projection.apply(&create.id, &create.order, ALICE, &create.record);
        let forged = DomainRecord::epic_archive(ALICE, 2, &id(1)).unwrap();
        assert_eq!(
            projection.apply(&id(2), &order(2), BOB, &forged),
            Outcome::Quarantined(SIGNER_MISMATCH)
        );
        assert!(!projection.epic(&id(1)).unwrap().archived);
        assert_eq!(projection.waiting().count(), 0);
    }

    #[test]
    fn an_older_record_never_overrides_a_newer_one() {
        let mut projection = Projection::default();
        for entry in [epic(1, "Launch"), ticket(2, 1, "Draft")] {
            projection.apply(&entry.id, &entry.order, entry.signer, &entry.record);
        }
        let newer = update(
            BOB,
            90,
            2,
            TicketChanges {
                status: Some(Status::Done),
                ..TicketChanges::default()
            },
        );
        let older = update(
            ALICE,
            80,
            2,
            TicketChanges {
                status: Some(Status::Ready),
                title: Some("Old".into()),
                ..TicketChanges::default()
            },
        );
        projection.apply(&id(9), &order(9), BOB, &newer);
        projection.apply(&id(8), &order(8), ALICE, &older);
        let ticket = projection.ticket(&id(2)).unwrap();
        assert_eq!(ticket.status, Status::Done);
        // The title had not been set after the older record, so the older title applies.
        assert_eq!(projection.texts().title(&id(2)), Some("Old"));
        assert_eq!(ticket.version, order(9));
        assert_eq!(ticket.last_editor, BOB);
        assert_eq!(ticket.last_at, 90);
    }
}
