//! The shared helpers (design D.13.1): every helper that more than one package calls, with its
//! signature frozen at WP-D1 and a total body that emits nothing and returns an empty or `None`
//! value until the package that owns it fills it in. Each body has exactly one owner:
//!
//! | File | Helpers | Owner |
//! |---|---|---|
//! | `holds.rs` | `issue_hold`, `extend`, `bind_holder_notes`, `can_take` | WP-D3 |
//! | `close.rs` | `close_node`, `record_assumptions`, `assume_now` | WP-D3 |
//! | `principals.rs` | `intern`, `resolve_principal`, `same_person`, `aliases`, `gone` | WP-D3 |
//! | `tree.rs` | `reaches` (D.8.4, T5) | WP-D3 |
//! | `query::leave_better` | `leave_better` | WP-D3 |
//! | `crate::refusals` | `class_refused`, `backed_off`, `excluded`, `active`, `all_refused` | WP-D3 |
//! | `conversation.rs` | `conv_node`, `conv_key`, `record_request`, `try_settle`, `answer_pq`, `route_to` | WP-D4 |
//! | `restructure.rs` | `open_signal`, `reopen` | WP-D4 |
//! | `user.rs` | `create_uq`, `chain`, `requesters`, `owners`, `due_for`, `absent`, `lapse`, `add_digest`, `audience`, `e1_check`, `refusal_default`, `p90_gap` | WP-D5a |
//! | `agents.rs` | `recipients`, `inbox`, `conversation_holder` | WP-D5b |
//! | `people.rs` | `recheck`, the Recheck firing of a question to a person (D.8.2) | WP-D5b |
//! | `recurring.rs` | `maybe_missed`, `open_next_occurrence`, `coverage` | WP-D6 |
//!
//! The helpers that emit events take an `Exec`, the execution context of one command or one firing
//! (D.8.1 step 5, D.8.2): it emits each event at `now` and applies it with `evolve_mut` at once, so
//! later steps read the updated state and the next free ids from `s.counters` ("plan by doing").
//! Execution cannot fail: every helper skips a missing referent rather than failing.

pub mod agents;
pub mod close;
pub mod conversation;
pub mod holds;
pub mod people;
pub mod principals;
pub mod recurring;
pub mod restructure;
pub mod tree;
pub mod user;

use alloc::vec::Vec;

use crate::event::{EntryBody, EntryText, Ev, Event};
use crate::evolve::{evolve_mut, evolve_tracked};
use crate::facts::Tracked;
use crate::reply::Warning;
use crate::state::{Author, State};
use crate::types::{Eid, NodeId, Seq, SessionId, Time};

/// The execution context of one command or one firing.
#[derive(Debug)]
pub struct Exec<'a> {
    /// The state, changed only through `emit`.
    s: &'a mut State,
    /// The command's clamped `now`, or the deadline's own time.
    now: Time,
    /// The calling session; None in a firing (the server acts) and under `SessionCtx::System`.
    sess: Option<SessionId>,
    /// The events emitted so far, in order.
    events: Vec<Event>,
    /// The warnings the execution adds.
    warnings: Vec<Warning>,
    /// The facts the events change, and the entities that leave memory, for `apply_tracked` and
    /// `settle_tracked` (D.6.1); None for `apply` and `settle`, which discard them.
    dirty: Option<&'a mut Tracked>,
}

impl<'a> Exec<'a> {
    /// Starts executing at `now` in `sess`.
    pub fn new(s: &'a mut State, now: Time, sess: Option<SessionId>) -> Exec<'a> {
        Exec {
            s,
            now,
            sess,
            events: Vec::new(),
            warnings: Vec::new(),
            dirty: None,
        }
    }

    /// `new`, adding to `dirty` every fact the emitted events change and every entity that leaves
    /// memory (D.6.1), when given.
    pub fn tracked(
        s: &'a mut State,
        now: Time,
        sess: Option<SessionId>,
        dirty: Option<&'a mut Tracked>,
    ) -> Exec<'a> {
        Exec {
            s,
            now,
            sess,
            events: Vec::new(),
            warnings: Vec::new(),
            dirty,
        }
    }

    /// The state as the events so far left it.
    #[must_use]
    pub fn s(&self) -> &State {
        self.s
    }

    /// The time every event is stamped with.
    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    /// The calling session, if any.
    #[must_use]
    pub fn sess(&self) -> Option<SessionId> {
        self.sess
    }

    /// Who writes the entries this execution appends: the session, or the server.
    #[must_use]
    pub fn author(&self) -> Author {
        self.sess.map_or(Author::Server, Author::Session)
    }

    /// The id the next `Appended` gets.
    #[must_use]
    pub fn next_eid(&self) -> Eid {
        self.s.counters.next_eid
    }

    /// Emits `ev` at `now` and applies it at once.
    pub fn emit(&mut self, ev: Ev) {
        let event = Event { at: self.now, ev };
        match self.dirty.as_deref_mut() {
            Some(d) => evolve_tracked(self.s, &event, d),
            None => evolve_mut(self.s, &event),
        }
        self.events.push(event);
    }

    /// Appends an entry to `node` with the next eid and seq, and returns its eid.
    pub fn append(&mut self, node: NodeId, by: Author, body: EntryBody, text: EntryText) -> Eid {
        let eid = self.next_eid();
        let seq = self
            .s
            .nodes
            .get(&node)
            .map_or(Seq(1), |n| Seq(n.seq.0.saturating_add(1)));
        self.emit(Ev::Appended {
            eid,
            node,
            seq,
            by,
            body,
            text,
        });
        eid
    }

    /// Adds a warning to the outcome.
    pub fn warn(&mut self, w: Warning) {
        self.warnings.push(w);
    }

    /// The events emitted so far.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// The warnings added so far.
    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The events emitted, in order, and the warnings added.
    #[must_use]
    pub fn finish(self) -> (Vec<Event>, Vec<Warning>) {
        (self.events, self.warnings)
    }
}
