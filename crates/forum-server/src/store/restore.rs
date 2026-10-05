//! Restores (design D.4.12): a command that names an entity that left memory restores it first.
//! In the writer job, before `apply` (D.8.1), the shell checks every id the command names and, for
//! each that is not resident but has a state row, decodes the row and calls the core's
//! `restore_tracked`, which emits `Restored { items }` as a transaction of its own (kind
//! `restore`) before the command's.
//!
//! [`named`] lists the ids of D.4.12's list: the requests of `settles`, `clarifies`, and `re`; the
//! questions of `answers`, `heard {answers}`, and `waits_for: {question}`; the decisions of
//! `supersedes` and of entry references in `basis`; the `said` entries of `said_id`, `corrects`,
//! and `unsent`; objections; the user entries and digest entries of `re`; the user entry of
//! `talk route`; and, through the `origins` table, the request of a `request`'s origin, so that Q1
//! holds across memory. [`items`] reads their rows. Beside D.4.12's list, it names the user entries
//! of a `node create`'s `user:e<n>` keys, which D.8.9 looks up among the resident user entries
//! (`not_found(UserEntry)` otherwise), by D.4.12's rule that a command naming an entity that left
//! memory restores it first.
//!
//! An entity whose leaving time has passed is pruned only at the next event (D.4.12, step 1 of
//! `evolve`). When no deadline fell due since, it is still resident when a command names it, and
//! the command's first event would prune it, so a later event of the command that names it would
//! fail its precondition. So the plan treats an entity that is resident with `leaving[x] <= now`
//! as departed ([`items_at`]): it restores it from its row, which is current (every change of a
//! resident entity rewrites its row), the `Restored` event's step 1 prunes it, and its arm
//! re-inserts it with a new leaving time; the writer then stores its final, resident value.

use std::collections::BTreeSet;

use forum_core::{
    BasisArg, Cmd, Eid, EntityRef, EntryMeta, Fact, HoldCmd, KeyStr, NodeCmd, Restored, State,
    TalkCmd, Time, WaitArg, leaving_due,
};
use rusqlite::Connection;
use serde::de::DeserializeOwned;

use super::shell_tables::origin_request;
use super::state::{fact_label, fact_of_entity, fact_row, key_json, kind};
use super::{Result, StoreError};

/// What a command names that may have left memory.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Named {
    /// The entities, typed by the argument that names them; an id that may be of several kinds
    /// (`re`, `waits_for: {question}`) is listed under each.
    pub entities: BTreeSet<EntityRef>,
    /// The origins whose request the `origins` table names.
    pub origins: BTreeSet<KeyStr>,
}

impl Named {
    fn requests(&mut self, ids: impl IntoIterator<Item = Eid>) {
        self.entities
            .extend(ids.into_iter().map(EntityRef::Request));
    }

    fn saids(&mut self, ids: impl IntoIterator<Item = Eid>) {
        self.entities.extend(ids.into_iter().map(EntityRef::Said));
    }
}

/// The ids a command names that D.4.12 restores. A `create {request}` names the request's origin,
/// defaulted as D.8.9 defaults it: `origin`, else `where`, else the first conversation key among
/// the node's keys, else its first key.
#[must_use]
pub fn named(s: &State, cmd: &Cmd) -> Named {
    let mut n = Named::default();
    match cmd {
        Cmd::Node(NodeCmd::Close {
            settles, unsent, ..
        }) => {
            n.requests(settles.iter().flatten().copied());
            n.saids(unsent.iter().copied());
        }
        Cmd::Node(NodeCmd::Create(a)) => {
            // The user entries of its `user:e<n>` keys, which D.8.9 step 1 looks up (a routed
            // entry gives `{existing}`, an unrouted one is routed by the create).
            n.entities.extend(
                a.keys
                    .iter()
                    .filter_map(|k| k.key.user_entry())
                    .map(EntityRef::UserEntry),
            );
            if let Some(r) = &a.request {
                let schemes = &s.params.conversation_schemes;
                let origin = r
                    .origin
                    .as_ref()
                    .or(r.where_.as_ref())
                    .or_else(|| a.keys.iter().find(|k| k.key.is_conversation(schemes)))
                    .or_else(|| a.keys.first());
                if let Some(o) = origin {
                    n.origins.insert(o.key.clone());
                }
            }
        }
        Cmd::Hold(HoldCmd::Release {
            waits_for,
            unsent,
            settles,
            ..
        }) => {
            // 1.12 (§10.3): a release's `settles`, as a close's and a decide's.
            n.requests(settles.iter().copied());
            n.saids(unsent.iter().copied());
            if let Some(WaitArg::Question(q)) = waits_for {
                n.entities.insert(EntityRef::PQuestion(*q));
                n.entities.insert(EntityRef::UQuestion(*q));
            }
        }
        Cmd::Talk(t) => match t {
            TalkCmd::FromUser {
                answers, clarifies, ..
            } => {
                n.entities
                    .extend(answers.iter().map(|q| EntityRef::UQuestion(*q)));
                n.requests(clarifies.iter().copied());
            }
            TalkCmd::ToUser { re, .. } => {
                for e in re {
                    n.entities.insert(EntityRef::Request(*e));
                    n.entities.insert(EntityRef::UserEntry(*e));
                    n.entities.insert(EntityRef::Digest(*e));
                }
            }
            TalkCmd::Route { entry, .. } => {
                n.entities.insert(EntityRef::UserEntry(*entry));
            }
            TalkCmd::Request { origin, .. } => {
                n.origins.insert(origin.key.clone());
            }
            TalkCmd::Said { corrects, .. } => n.saids(corrects.iter().copied()),
            TalkCmd::SaidConfirm { said, .. } => n.saids([*said]),
            TalkCmd::Heard { answers, .. } => {
                n.entities
                    .extend(answers.iter().map(|q| EntityRef::PQuestion(*q)));
            }
            TalkCmd::Decide {
                settles,
                supersedes,
                basis,
                ..
            } => {
                n.requests(settles.iter().copied());
                n.entities
                    .extend(supersedes.iter().map(|d| EntityRef::Decision(*d)));
                for b in basis {
                    if let BasisArg::Entry(e) = b {
                        n.entities.insert(EntityRef::Decision(*e));
                    }
                }
            }
            TalkCmd::Resolve { objection, .. } => {
                n.entities.insert(EntityRef::Objection(*objection));
            }
            TalkCmd::Ask { .. }
            | TalkCmd::Answer { .. }
            | TalkCmd::Say { .. }
            | TalkCmd::Approve { .. }
            | TalkCmd::Object { .. }
            | TalkCmd::Retract { .. }
            | TalkCmd::Wait { .. }
            | TalkCmd::Who { .. } => {}
        },
        Cmd::Session(_)
        | Cmd::View(_)
        | Cmd::Node(_)
        | Cmd::Hold(_)
        | Cmd::Control(_)
        | Cmd::Admin(_) => {}
    }
    n
}

/// Whether `s` holds the entity.
fn resident(s: &State, x: EntityRef) -> bool {
    match x {
        EntityRef::Request(i) => s.requests.contains_key(&i),
        EntityRef::PQuestion(i) => s.pquestions.contains_key(&i),
        EntityRef::UQuestion(i) => s.uquestions.contains_key(&i),
        EntityRef::Commitment(i) => s.commitments.contains_key(&i),
        EntityRef::Said(i) => s.saids.contains_key(&i),
        EntityRef::Decision(i) => s.decisions.contains_key(&i),
        EntityRef::Objection(i) => s.objections.contains_key(&i),
        EntityRef::UserEntry(i) => s.user_entries.contains_key(&i),
        EntityRef::Digest(i) => s.digest.contains_key(&i),
        EntityRef::Meta(i) => s.entries.contains_key(&i),
    }
}

fn id_of(x: EntityRef) -> Eid {
    match x {
        EntityRef::Request(i)
        | EntityRef::PQuestion(i)
        | EntityRef::UQuestion(i)
        | EntityRef::Commitment(i)
        | EntityRef::Said(i)
        | EntityRef::Decision(i)
        | EntityRef::Objection(i)
        | EntityRef::UserEntry(i)
        | EntityRef::Digest(i)
        | EntityRef::Meta(i) => i,
    }
}

fn decode<T: DeserializeOwned>(f: &Fact, v: &str) -> Result<T> {
    serde_json::from_str(v).map_err(|e| StoreError::Decode {
        table: "st_facts",
        key: fact_label(kind(f), &key_json(f).unwrap_or_default()),
        msg: e.to_string(),
    })
}

/// The metadata of the entry an entity was born from: resident in `s`, or from its row.
fn meta_of(conn: &Connection, s: &State, e: Eid) -> Result<Option<EntryMeta>> {
    if let Some(m) = s.entries.get(&e) {
        return Ok(Some(*m));
    }
    let f = Fact::Entry(e);
    fact_row(conn, &f)?.map(|(v, _)| decode(&f, &v)).transpose()
}

/// The restorable items of what a command names, on a state settled at `s.last_now` with no event
/// to come: `items_at` at `s.last_now`, where no leaving time has come (T3).
///
/// # Errors
///
/// As `items_at`.
pub fn items(conn: &Connection, s: &State, named: &Named) -> Result<Vec<Restored>> {
    items_at(conn, s, named, s.last_now)
}

/// The restorable items of what a command at `now` names (D.4.12): for each named entity with an
/// id below `next_eid` and the metadata of its entry that is either not resident in `s` and has a
/// row marked not resident, or resident with its leaving time at or before `now` (its row is then
/// current, and marked resident), the item built from its row. An origin names the request the
/// `origins` table gives for it. The core drops whatever is resident and not due by the time it
/// restores.
///
/// # Errors
///
/// A failed query, or a row that does not decode.
pub fn items_at(conn: &Connection, s: &State, named: &Named, now: Time) -> Result<Vec<Restored>> {
    let mut wanted = named.entities.clone();
    for o in &named.origins {
        if let Some(r) = origin_request(conn, o.as_str())? {
            wanted.insert(EntityRef::Request(r));
        }
    }
    let mut out = Vec::new();
    for x in wanted {
        let id = id_of(x);
        let due = leaving_due(s, x, now);
        if id >= s.counters.next_eid || (resident(s, x) && !due) {
            continue;
        }
        let f = fact_of_entity(x);
        let Some((v, row_resident)) = fact_row(conn, &f)? else {
            continue;
        };
        if row_resident && !due {
            continue;
        }
        let Some(m) = meta_of(conn, s, id)? else {
            continue;
        };
        out.push(match x {
            EntityRef::Request(i) => Restored::Request(i, decode(&f, &v)?, m),
            EntityRef::PQuestion(i) => Restored::PQuestion(i, decode(&f, &v)?, m),
            EntityRef::UQuestion(i) => Restored::UQuestion(i, decode(&f, &v)?, m),
            EntityRef::Commitment(i) => Restored::Commitment(i, decode(&f, &v)?, m),
            EntityRef::Said(i) => Restored::Said(i, decode(&f, &v)?, m),
            EntityRef::Decision(i) => Restored::Decision(i, decode(&f, &v)?, m),
            EntityRef::Objection(i) => Restored::Objection(i, decode(&f, &v)?, m),
            EntityRef::UserEntry(i) => Restored::UserEntry(i, decode(&f, &v)?, m),
            EntityRef::Digest(i) => Restored::Digest(i, decode(&f, &v)?, m),
            EntityRef::Meta(i) => Restored::Meta(i, decode(&f, &v)?),
        });
    }
    Ok(out)
}

/// The restorable items of a list of entities, read from their rows: `items` without a command.
///
/// # Errors
///
/// As `items`.
pub fn items_of(conn: &Connection, s: &State, refs: &[EntityRef]) -> Result<Vec<Restored>> {
    let named = Named {
        entities: refs.iter().copied().collect(),
        origins: BTreeSet::new(),
    };
    items(conn, s, &named)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test code"
    )]

    use forum_core::{
        ClassName, Ctx, DAY, Dur, NodeId, NodeRef, SessionCtx, Text, Tracked, apply_tracked,
    };

    use super::super::history::TxKind;
    use super::super::load::load_state;
    use super::super::replay::differences;
    use super::super::testkit::{World, scenario, test_params};
    use super::*;

    /// An entity whose leaving time has passed with no event since is still resident when a
    /// command names it (D.4.12). The plan treats it as departed: it restores it from its row,
    /// which is current and marked resident; the `Restored` event's step 1 prunes it and its arm
    /// re-inserts it, so the writer stores its final, resident value although the event recorded
    /// it as departed; and the command then runs with no anomaly and finds it resident.
    #[test]
    fn an_entity_due_to_leave_with_no_event_since_is_restored_from_its_current_row() {
        let mut w = World::memory(test_params());
        scenario(&mut w);
        let request = Eid(1);
        let x = EntityRef::Request(request);
        let leaves =
            *w.s.leaving
                .get(&x)
                .expect("the scenario's request is settled and resident");
        // Idle past its leaving time: every deadline due by then fires, none after it.
        let later = leaves.plus(Dur(DAY.0));
        assert!(w.catch_up(later));
        w.at(later);
        assert!(w.s.last_now < leaves, "an event after the leaving time");
        assert!(w.s.requests.contains_key(&request));
        let decide = Cmd::Talk(TalkCmd::Decide {
            at: NodeRef::Id(NodeId(8)),
            text: Text("again".into()),
            settles: [request].into(),
            supersedes: None,
            basis: Vec::new(),
            evidence: None,
        });
        let n = named(&w.s, &decide);
        // At the state's own time nothing is due (T3); at the command's `now` the request is.
        assert!(items(&w.conn, &w.s, &n).unwrap().is_empty());
        let got = items_at(&w.conn, &w.s, &n, w.clock).unwrap();
        let row = fact_row(&w.conn, &Fact::Request(request)).unwrap().unwrap();
        assert!(row.1, "the row of a resident entity is marked resident");
        match got.as_slice() {
            [Restored::Request(id, v, m)] => {
                assert_eq!(*id, request);
                assert_eq!(v, &w.s.requests[&request], "the row is current");
                assert_eq!(*m, w.s.entries[&request]);
            }
            other => panic!("{other:?}"),
        }
        // The restore transaction: pruned and re-inserted by one event, written resident.
        let before = w.s.clone();
        let mut dirty = Tracked::new();
        let evs = forum_core::restore_tracked(&mut w.s, w.clock, got, &mut dirty);
        assert_eq!(evs.len(), 1);
        assert!(
            dirty.departed.contains_key(&Fact::Request(request)),
            "the event's prune recorded it as departed"
        );
        w.write_applied(TxKind::Restore, &before, &evs, &dirty);
        let row = fact_row(&w.conn, &Fact::Request(request)).unwrap().unwrap();
        assert_eq!(
            row,
            (
                serde_json::to_string(&w.s.requests[&request]).unwrap(),
                true
            ),
            "the final, resident value"
        );
        assert!(!leaving_due(&w.s, x, w.clock));
        // The command finds it, and applies with no anomaly.
        let ctx = Ctx {
            now: w.clock,
            class: ClassName::new("coder").unwrap(),
            caps: None,
            session: SessionCtx::New,
            op: None,
        };
        let before = w.s.clone();
        let mut dirty = Tracked::new();
        let (evs, out) = apply_tracked(&mut w.s, &ctx, &decide, &mut dirty);
        assert!(out.is_ok(), "{out:?}");
        assert_eq!(w.s.anomalies, before.anomalies);
        assert!(w.s.requests.contains_key(&request));
        w.write_applied(TxKind::Cmd, &before, &evs, &dirty);
        let loaded = load_state(&w.conn).unwrap();
        let d = differences(&loaded, &w.s);
        assert!(d.is_empty() || d == ["ix"], "{d:?}");
    }
}
