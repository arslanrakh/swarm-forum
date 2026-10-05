//! The storage tests of WP-E1 (design D.13.2, D.12.6, D.12.1's `forum-server` row): persist and
//! reload, history replay, the two-pass writing and the constraints, the append-only history, the
//! shell's tables and their rebuild, migrations, and restores.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    reason = "test code"
)]

use std::collections::{BTreeMap, BTreeSet};

use forum_core::{
    BasisTarget, Done, EntityRef, EntryBody, Ev, Event, Fact, Fence, HoldEnd, Key, LinkRel, MINUTE,
    Name, NodeId, NodePatch, NormVer, OpId, Outcome, PrincipalKey, ROOT, ReqDigest, Requester,
    SaidKind, Seq, SessionId, SignalKey, SignalKind, State, SystemNote, Text, Time, Token,
};
use rusqlite::Connection;

use super::history::{TxKind, decode_event, encode_event, events_after, last_seq, node_history};
use super::load::{Checks, check_loaded, load_state};
use super::replay::{
    differences, replay, replay_into, replay_into_checked, verify, verify_checked,
};
use super::schema::{HISTORY_TABLES, SHELL_TABLES, STATE_TABLES};
use super::shell_tables::rebuild_shell_tables;
use super::state::{
    FACT_NAMES, Which, fact_of, facts_of, key_json, kind, read_rows, value_json, write_dirty,
};
use super::testkit::{
    Draft, Settling, TempDb, World, appended, class, create_node, created, fresh_file, init,
    issued, keystr, label, random_history, random_history_with, session_opened, test_params,
    with_session,
};

// ------------------------------------------------------------------------------------------------
// Helpers

/// Every row of a table, each column rendered, in a fixed order: for comparing tables.
fn dump_table(conn: &Connection, table: &str) -> Vec<Vec<String>> {
    let mut st = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
    let n = st.column_count();
    let mut rows: Vec<Vec<String>> = st
        .query_map([], |r| {
            let mut v = Vec::with_capacity(n);
            for i in 0..n {
                let x: rusqlite::types::Value = r.get(i)?;
                v.push(format!("{x:?}"));
            }
            Ok(v)
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.sort();
    rows
}

fn dump(conn: &Connection, tables: &[&str]) -> BTreeMap<String, Vec<Vec<String>>> {
    tables
        .iter()
        .map(|t| ((*t).to_owned(), dump_table(conn, t)))
        .collect()
}

/// The persist-and-reload check (D.12.6): loading the resident rows into a fresh state equals the
/// in-memory state, base and history parts field by field, and its recount equals the live
/// indices; every entity that left memory has a row marked not resident whose value is its last
/// value.
fn assert_reloads(w: &World, what: &str) {
    let loaded = load_state(&w.conn).unwrap_or_else(|e| panic!("{what}: load: {e}"));
    let d = differences(&loaded, &w.s);
    if !d.is_empty() {
        // The base and history parts must be equal; the indices are the recount's, which equals
        // the live indices wherever R1 holds (the core's own tests hold it there).
        let r1 = forum_core::invariants::r1_differences(&w.s, w.s.last_now);
        assert!(
            d == ["ix"] && !r1.is_empty(),
            "{what}: the loaded state differs in {d:?} (live R1 differences: {r1:?})"
        );
        assert_eq!(loaded.ix, forum_core::recount(&w.s, w.s.last_now), "{what}");
    }
    for (f, v) in &w.grave {
        let row = super::state::fact_row(&w.conn, f).unwrap();
        assert_eq!(
            row,
            Some((v.clone(), false)),
            "{what}: the row of {f:?}, which left memory"
        );
    }
}

// ------------------------------------------------------------------------------------------------
// The codec

/// One instance of every `Fact` variant.
fn every_fact() -> Vec<Fact> {
    let n = NodeId(9);
    let e = forum_core::Eid(4);
    vec![
        Fact::Scalars,
        Fact::NodeHot(n),
        Fact::NodeContent(n),
        Fact::Key(n, keystr("chat:a/1")),
        Fact::Link(n, LinkRel::Continues, NodeId(10)),
        Fact::Name(n, Name::new("x").unwrap()),
        Fact::Alias(n, Name::new("y").unwrap()),
        Fact::Hold(Token {
            node: n,
            fence: Fence(2),
        }),
        Fact::Entry(e),
        Fact::Request(e),
        Fact::PQuestion(e),
        Fact::AQuestion(e),
        Fact::UQuestion(e),
        Fact::Commitment(e),
        Fact::Said(e),
        Fact::Decision(e),
        Fact::Objection(e),
        Fact::UserEntry(e),
        Fact::Digest(e),
        Fact::Note(e),
        Fact::Approval(n, forum_core::Version(3)),
        Fact::Retracted(BasisTarget::Entry(e)),
        Fact::Refusal(n, class("a")),
        Fact::ClassProfile(class("a"), [label("x"), label("y")].into()),
        Fact::Signal(SignalKey {
            kind: SignalKind::Conflict,
            a: n,
            b: NodeId(12),
        }),
        Fact::Pause(n),
        Fact::Session(SessionId(3)),
        Fact::Principal(PrincipalKey::new("mail:a@x").unwrap()),
        Fact::Op(class("a"), OpId::new("op-1").unwrap()),
        Fact::Day(20_000),
        Fact::Start(29_000_000),
        Fact::Profile([label("x")].into(), 6),
        Fact::DemandCounted(n),
        Fact::Since(forum_core::Cand::Status(n)),
        Fact::UserMsgs(forum_core::PrincipalId(2)),
        Fact::Leaving(EntityRef::Said(e)),
        Fact::DigestLeft(forum_core::PrincipalId(2)),
    ]
}

#[test]
fn the_fact_column_is_the_variant_index_and_keys_round_trip() {
    let all = every_fact();
    assert_eq!(all.len(), FACT_NAMES.len());
    for (i, f) in all.iter().enumerate() {
        assert_eq!(usize::from(kind(f)), i, "{f:?}");
        let name = match serde_json::to_value(f).unwrap() {
            serde_json::Value::String(s) => s,
            serde_json::Value::Object(m) => m.keys().next().unwrap().clone(),
            other => panic!("{other}"),
        };
        assert_eq!(name, FACT_NAMES[i]);
        let key = key_json(f).unwrap();
        assert_eq!(&fact_of(kind(f), &key).unwrap(), f, "{key}");
    }
    // A tuple key is a JSON array; a map key's label set is a sorted array (D.7.6).
    assert_eq!(key_json(&all[3]).unwrap(), r#"[9,"chat:a/1"]"#);
    assert_eq!(key_json(&all[23]).unwrap(), r#"["a",["x","y"]]"#);
    assert_eq!(key_json(&all[0]).unwrap(), "null");
    assert!(fact_of(37, "1").is_err());
    assert!(fact_of(9, "\"not an eid\"").is_err());
    // A struct key keeps its fields in declaration order, as the canonical JSON of D.7.6 does,
    // whatever serde_json's features: the payload's own serialisation.
    let signal = SignalKey {
        kind: SignalKind::Duplicate,
        a: NodeId(3),
        b: NodeId(4),
    };
    assert_eq!(
        key_json(&Fact::Signal(signal)).unwrap(),
        serde_json::to_string(&signal).unwrap()
    );
    assert_eq!(
        key_json(&Fact::Signal(signal)).unwrap(),
        r#"{"kind":"Duplicate","a":3,"b":4}"#
    );
    let token = Token {
        node: NodeId(9),
        fence: Fence(2),
    };
    assert_eq!(
        key_json(&Fact::Hold(token)).unwrap(),
        serde_json::to_string(&token).unwrap()
    );
    for f in &all[1..] {
        let whole = serde_json::to_string(f).unwrap();
        let key = key_json(f).unwrap();
        assert!(whole.ends_with(&format!(":{key}}}")), "{whole} and {key}");
    }
}

#[test]
fn every_event_body_round_trips_through_zstd() {
    let w = random_history(3, 120, |_, _| {});
    let events = events_after(&w.conn, 0, 100_000).unwrap();
    assert_eq!(events.len() as u64, w.s.counters.events);
    for x in &events {
        let z = encode_event(&x.event).unwrap();
        assert_eq!(decode_event(1, &z).unwrap(), x.event);
    }
    assert!(decode_event(2, &encode_event(&events[0].event).unwrap()).is_err());
    assert!(decode_event(1, b"not zstd").is_err());
    // Each text is stored once, compressed: a long body shrinks.
    let long = Event {
        at: Time(1),
        ev: Ev::SessionStatus {
            session: SessionId(1),
            text: Text("the same words again ".repeat(500)),
        },
    };
    let raw = serde_json::to_vec(&long).unwrap().len();
    assert!(encode_event(&long).unwrap().len() * 10 < raw);
}

// ------------------------------------------------------------------------------------------------
// Genesis and the load

#[test]
fn genesis_writes_its_transaction_rows_and_meta() {
    let db = TempDb::new("genesis");
    let o = fresh_file(&db, test_params());
    assert!(o.created);
    assert!(o.migrated.is_empty());
    let conn = o.store.conn();
    assert_eq!(last_seq(conn).unwrap(), 1);
    let txs = super::history::txs_after(conn, 0, 10).unwrap();
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0].meta.kind, TxKind::Genesis);
    assert_eq!(o.state.counters.events, 1);
    assert_eq!(o.state.nodes.len(), 8);
    let mut s = State::default();
    forum_core::evolve_mut(
        &mut s,
        &forum_core::genesis(test_params(), super::testkit::START),
    );
    assert_eq!(differences(&o.state, &s), Vec::<&str>::new());
    for (k, v) in [
        ("state_version", "1"),
        ("shell_version", "1"),
        ("keys_version", "1"),
        ("masking_version", "1"),
    ] {
        assert_eq!(
            super::schema::get_meta(conn, k).unwrap().as_deref(),
            Some(v)
        );
    }
    assert_eq!(
        super::schema::get_meta(conn, "core_semver")
            .unwrap()
            .as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    // The reserved nodes' event_nodes rows serve view history.
    assert_eq!(
        node_history(conn, ROOT, None, 10).unwrap(),
        Vec::<u64>::new()
    );
    check_loaded(&o.state, Checks::Full).unwrap();
    // A second open loads what the first wrote.
    drop(o);
    let again = fresh_file(&db, test_params());
    assert!(!again.created);
    assert_eq!(differences(&again.state, &s), Vec::<&str>::new());
}

#[test]
fn a_transaction_without_events_writes_nothing() {
    let w = World::memory(test_params());
    let n = dump(&w.conn, &HISTORY_TABLES);
    let wrote = super::write(
        &w.conn,
        &super::TxMeta::new(TxKind::Cmd, w.clock),
        &[],
        &w.s,
        &forum_core::Tracked::new(),
    )
    .unwrap();
    assert!(wrote.is_none());
    assert_eq!(dump(&w.conn, &HISTORY_TABLES), n);
}

// ------------------------------------------------------------------------------------------------
// Persist and reload, replay, rebuild (D.12.6)

#[test]
fn persist_and_reload_equal_the_state_after_every_transaction() {
    let mut not_resident = 0_usize;
    let mut left = 0_usize;
    let mut restored = 0_usize;
    for seed in 1..=16 {
        let w = random_history(seed, 220, |w, step| {
            assert_reloads(w, &format!("seed {seed} step {step}"));
        });
        left += w.grave.len();
        restored += w.restored;
        not_resident += read_rows(&w.conn, Which::All)
            .unwrap()
            .iter()
            .filter(|r| !r.resident)
            .count();
        // The history replays to the state tables, and `replay --into` writes the same tables.
        let replayed = verify(&w.conn).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert_eq!(differences(&replayed, &w.s), Vec::<&str>::new());
        let dst = Connection::open_in_memory().unwrap();
        let into = replay_into(&w.conn, &dst).unwrap();
        assert_eq!(differences(&into, &w.s), Vec::<&str>::new(), "seed {seed}");
        let all: Vec<&str> = STATE_TABLES
            .iter()
            .chain(HISTORY_TABLES.iter())
            .chain(SHELL_TABLES.iter())
            .copied()
            .collect();
        assert_eq!(
            dump(&dst, &all),
            dump(&w.conn, &all),
            "seed {seed}: replay --into"
        );
        // Rebuilding the shell's tables from the history equals writing them incrementally.
        let before = dump(&w.conn, &SHELL_TABLES);
        let tx = w.conn.unchecked_transaction().unwrap();
        rebuild_shell_tables(&tx).unwrap();
        assert_eq!(dump(&tx, &SHELL_TABLES), before, "seed {seed}: rebuild");
        tx.rollback().unwrap();
    }
    // The histories reach the memory prune and restores through the rows (D.4.12).
    assert!(left > 0, "no entity left memory");
    assert!(not_resident > 0, "no row is marked not resident");
    assert!(restored > 0, "no entity was restored from its row");
}

// Review E1-spec-1: a settle batch may span a leaving time (D.8.1, D.8.2), so one write can change
// an entity, or create it, and then prune it. `evolve_tracked`, `settle_tracked`, `apply_tracked`,
// and `restore_tracked` hand the writer, beside the dirty set, the last value of every entity that
// leaves memory during the job (`Tracked::departed`, recorded in `leave_memory` and `drop_meta`
// before the entity is removed), and `write_dirty` writes that value with `resident = 0`.

/// A user entry, appended at the clock.
fn from_user(w: &mut World, by: SessionId) -> forum_core::Eid {
    let e = w.s.counters.next_eid;
    w.run(TxKind::Cmd, |d| {
        let ev = appended(
            d,
            forum_core::USER,
            by,
            EntryBody::FromUser {
                principal: forum_core::USER_PRINCIPAL,
            },
            "please look at the parser",
        );
        d.push(ev);
    });
    e
}

#[test]
fn one_write_that_changes_an_entity_and_prunes_it_keeps_its_last_value() {
    let (mut w, s1) = with_session();
    // (a) A stored entity changes, and a later event of the same write prunes it: its row must
    // hold the changed value, not the stored one.
    let e = from_user(&mut w, s1);
    let t1 = w.clock.plus(MINUTE);
    let t2 = t1.plus(forum_core::FINISHED_RETAIN).plus(MINUTE);
    w.tx_events(
        TxKind::Settle,
        vec![
            Event {
                at: t1,
                ev: Ev::EntryRouted {
                    entry: e,
                    routing: forum_core::Routing::NoAction,
                    reason: Some(Text("nothing to do".into())),
                },
            },
            Event {
                at: t2,
                ev: Ev::SessionStatus {
                    session: s1,
                    text: Text("later".into()),
                },
            },
        ],
    );
    w.at(t2);
    assert!(!w.s.user_entries.contains_key(&e));
    assert_reloads(&w, "changed and pruned in one write");
}

#[test]
fn an_entity_born_and_pruned_in_one_write_has_a_row() {
    let (mut w, s1) = with_session();
    // `write_applied` asserts that no entity left memory without a row.
    let e2 = w.s.counters.next_eid;
    let ev = appended(
        &w,
        forum_core::USER,
        s1,
        EntryBody::FromUser {
            principal: forum_core::USER_PRINCIPAL,
        },
        "and the lexer",
    );
    let t3 = w.clock.plus(forum_core::FINISHED_RETAIN).plus(MINUTE);
    w.tx_events(
        TxKind::Settle,
        vec![
            Event { at: w.clock, ev },
            Event {
                at: w.clock,
                ev: Ev::EntryRouted {
                    entry: e2,
                    routing: forum_core::Routing::NoAction,
                    reason: Some(Text("nothing to do".into())),
                },
            },
            Event {
                at: t3,
                ev: Ev::SessionStatus {
                    session: s1,
                    text: Text("later still".into()),
                },
            },
        ],
    );
    w.at(t3);
    assert!(
        super::state::fact_row(&w.conn, &Fact::UserEntry(e2))
            .unwrap()
            .is_some_and(|(_, resident)| !resident)
    );
    assert_reloads(&w, "born and pruned in one write");
}

#[test]
fn persist_and_reload_with_settle_batches_that_span_the_retain_times() {
    for seed in 1..=16 {
        let _ = random_history_with(seed, 220, Settling::Batched(64), |w, step| {
            assert_reloads(w, &format!("seed {seed} step {step}"));
        });
    }
}

/// An entity that left memory with neither its last value nor a row to mark is a core bug, which a
/// debug or test build refuses at once (review E1-spec-1); with its value, the row is written not
/// resident.
#[test]
fn an_entity_that_left_with_no_value_and_no_row_is_refused() {
    let (mut w, s1) = with_session();
    let e = from_user(&mut w, s1);
    let gone = Fact::UserEntry(forum_core::Eid(e.0 + 1_000));
    let tx = w.conn.unchecked_transaction().unwrap();
    let err = write_dirty(&tx, &w.s, &[gone.clone()].into(), &BTreeMap::new()).unwrap_err();
    assert!(err.to_string().contains("left memory"), "{err}");
    // With the value the core recorded, the row is written, not resident.
    let mut v = w.s.user_entries[&e];
    v.routing = forum_core::Routing::NoAction;
    let departed = BTreeMap::from([(gone.clone(), forum_core::Departed::UserEntry(v))]);
    let written = write_dirty(&tx, &w.s, &[gone.clone()].into(), &departed).unwrap();
    assert_eq!(written.left, 1);
    assert!(written.left_without_row.is_empty());
    let (value, resident) = super::state::fact_row(&tx, &gone).unwrap().unwrap();
    assert!(!resident);
    assert_eq!(value, serde_json::to_string(&v).unwrap());
    tx.rollback().unwrap();
}

#[test]
fn the_history_numbers_events_by_the_states_count() {
    let w = random_history(7, 80, |w, _| {
        assert_eq!(last_seq(&w.conn).unwrap(), w.s.counters.events);
    });
    let seqs: Vec<u64> = events_after(&w.conn, 0, 100_000)
        .unwrap()
        .iter()
        .map(|x| x.seq)
        .collect();
    assert_eq!(seqs, (1..=w.s.counters.events).collect::<Vec<_>>());
}

#[test]
fn every_fact_of_a_state_round_trips_through_its_row() {
    let w = random_history(11, 150, |_, _| {});
    // Write the whole state into empty tables, as dirty facts, and load it back.
    let dst = Connection::open_in_memory().unwrap();
    super::schema::create_tables(&dst).unwrap();
    let facts = facts_of(&w.s);
    write_dirty(&dst, &w.s, &facts, &BTreeMap::new()).unwrap();
    for (class_name, op) in w.s.ops.keys() {
        super::state::put_op(&dst, &w.s, class_name, op, 1, b"r").unwrap();
    }
    let loaded = load_state(&dst).unwrap();
    assert_eq!(differences(&loaded, &w.s), Vec::<&str>::new());
    // Every fact has a value exactly when the state holds it.
    for f in &facts {
        assert!(value_json(&w.s, f).unwrap().is_some(), "{f:?}");
    }
    assert_eq!(
        replay(&w.conn).unwrap().counters.events,
        w.s.counters.events
    );
}

#[test]
#[ignore = "a coverage report, run by hand"]
fn coverage_report() {
    let mut fallback = 0_u32;
    let mut restored = 0_usize;
    let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
    for seed in 1..=16 {
        let w = random_history(seed, 220, |w, _| {
            if !forum_core::invariants::r1_differences(&w.s, w.s.last_now).is_empty() {
                fallback += 1;
            }
        });
        for x in events_after(&w.conn, 0, 1_000_000).unwrap() {
            let mut name = format!("{:?}", x.event.ev);
            name.truncate(name.find([' ', '{', '(']).unwrap_or(name.len()));
            if let Ev::Restored { items } = &x.event.ev {
                restored += items.len();
            }
            *kinds.entry(name).or_insert(0) += 1;
        }
        for t in STATE_TABLES.iter().chain(SHELL_TABLES.iter()) {
            let n: i64 = w
                .conn
                .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                .unwrap();
            *kinds.entry(format!("~rows {t}")).or_insert(0) += n as u64;
        }
        let gone: i64 = w
            .conn
            .query_row(
                "SELECT count(*) FROM st_facts WHERE resident = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        *kinds.entry("~not resident".to_owned()).or_insert(0) += gone as u64;
        let aliases: i64 = w
            .conn
            .query_row("SELECT count(*) FROM st_names WHERE alias = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        *kinds.entry("~aliases".to_owned()).or_insert(0) += aliases as u64;
        *kinds.entry("~retired".to_owned()).or_insert(0) +=
            w.s.counters.next_session.0 - 1 - w.s.sessions.len() as u64;
    }
    for (k, v) in &kinds {
        eprintln!("{k:30} {v}");
    }
    eprintln!("R1 fallbacks {fallback}, restored items {restored}");
}

// ------------------------------------------------------------------------------------------------
// A valid forum

#[test]
fn a_valid_forum_reloads_and_passes_every_check() {
    let mut w = World::memory(test_params());
    super::testkit::scenario(&mut w);
    assert_reloads(&w, "scenario");
    let loaded = load_state(&w.conn).unwrap();
    if let Err(e) = check_loaded(&loaded, Checks::Full) {
        panic!("{e}");
    }
    verify_checked(&w.conn).unwrap();
    let gone: i64 = w
        .conn
        .query_row(
            "SELECT count(*) FROM st_facts WHERE resident = 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(gone > 0, "the scenario leaves rows that are not resident");
}

// ------------------------------------------------------------------------------------------------
// The two-pass writing and the constraints (D.11.6, §24)

/// Opens a node with a key under ROOT and returns it.
fn keyed(w: &mut World, name: &str, by: SessionId, cap: u32, keys: &[&str]) -> NodeId {
    create_node(w, ROOT, name, by, init(cap, keys))
}

#[test]
fn a_close_and_a_create_that_pass_one_key_on_in_one_group() {
    let (mut w, s1) = with_session();
    let a = keyed(&mut w, "a", s1, 1, &["chat:k/1"]);
    let c = w.s.counters.next_node;
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::NodeClosed {
            node: a,
            outcome: Outcome::Done,
            merged_into: None,
            by: None,
        });
        let ev = created(d, ROOT, "c", s1, init(1, &["chat:k/1"]));
        d.push(ev);
    });
    assert_reloads(&w, "close and create");
    let open: Vec<(i64, bool)> = w
        .conn
        .prepare("SELECT node, open FROM st_keys WHERE key = 'chat:k/1' ORDER BY node")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(open, vec![(a.0 as i64, false), (c.0 as i64, true)]);
}

#[test]
fn a_reopen_that_drops_keys_in_one_group() {
    let (mut w, s1) = with_session();
    let a = keyed(&mut w, "a", s1, 1, &["chat:k/1", "lean:t9"]);
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::NodeClosed {
            node: a,
            outcome: Outcome::Done,
            merged_into: None,
            by: None,
        });
    });
    let b = keyed(&mut w, "b", s1, 1, &["chat:k/1"]);
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::KeyRemoved {
            node: a,
            key: keystr("chat:k/1"),
        });
        d.push(Ev::NodeReopened { node: a });
        let ev = appended(
            d,
            a,
            s1,
            EntryBody::System(SystemNote::KeysDropped {
                keys: vec![(keystr("chat:k/1"), b)],
            }),
            "",
        );
        d.push(ev);
        let ev = appended(d, a, s1, EntryBody::System(SystemNote::Reopened), "again");
        d.push(ev);
    });
    assert_reloads(&w, "reopen");
    let n: i64 = w
        .conn
        .query_row(
            "SELECT count(*) FROM st_keys WHERE open = 1 AND key = 'lean:t9'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn a_key_that_changes_hands_in_one_group_is_written_closed_before_open() {
    let (mut w, s1) = with_session();
    let a = keyed(&mut w, "a", s1, 1, &["chat:k/1"]);
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::NodeClosed {
            node: a,
            outcome: Outcome::Done,
            merged_into: None,
            by: None,
        });
    });
    let b = keyed(&mut w, "b", s1, 1, &["chat:k/1"]);
    assert!(a < b);
    // b gives the key up by closing, and a, the smaller id, reopens with it, in one group.
    let before = w.s.clone();
    let mut d = w.draft();
    d.push(Ev::NodeClosed {
        node: b,
        outcome: Outcome::Dropped,
        merged_into: None,
        by: None,
    });
    d.push(Ev::NodeReopened { node: a });
    let events: Vec<Event> = d
        .evs
        .iter()
        .map(|ev| Event {
            at: w.clock,
            ev: ev.clone(),
        })
        .collect();
    // Written in plain fact order, a's open row would meet b's before b's turns 0.
    let mut naive = before.clone();
    let mut dirty = forum_core::Tracked::new();
    for e in &events {
        forum_core::evolve_tracked(&mut naive, e, &mut dirty);
    }
    let tx = w.conn.unchecked_transaction().unwrap();
    let keys: Vec<_> = dirty
        .facts
        .iter()
        .filter(|f| matches!(f, Fact::Key(..)))
        .map(|f| super::state::row_of(&naive, f).unwrap().unwrap())
        .collect();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].extra, super::state::Extra::Open(true));
    let e = keys
        .iter()
        .try_for_each(|r| super::state::put_row(&tx, r))
        .unwrap_err();
    assert!(e.is_constraint(), "{e}");
    assert!(e.to_string().contains("st_keys.key"), "{e}");
    tx.rollback().unwrap();
    // The two passes write it without a transient violation.
    w.commit(TxKind::Cmd, d);
    assert_reloads(&w, "a key changes hands");
}

#[test]
fn a_merge_in_one_group() {
    let (mut w, s1) = with_session();
    let from = keyed(&mut w, "from", s1, 1, &["chat:k/1"]);
    let into = keyed(&mut w, "into", s1, 1, &["issue:p#2"]);
    let r = w.s.counters.next_eid;
    w.run(TxKind::Cmd, |d| {
        let ev = issued(d, from, s1);
        d.push(ev);
        let ev = appended(
            d,
            from,
            s1,
            EntryBody::Request {
                origin: Key {
                    key: keystr("chat:k/1/m1"),
                    nv: NormVer(1),
                },
                conv: keystr("chat:k/1"),
                from_user: None,
                requester: Requester::External,
            },
            "a request",
        );
        d.push(ev);
    });
    let token = Token {
        node: from,
        fence: Fence(1),
    };
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::KeyRemoved {
            node: from,
            key: keystr("chat:k/1"),
        });
        d.push(Ev::KeyAdded {
            node: into,
            key: Key {
                key: keystr("chat:k/1"),
                nv: NormVer(1),
            },
        });
        d.push(Ev::RequestMoved {
            request: r,
            to: into,
        });
        let ev = appended(
            d,
            into,
            s1,
            EntryBody::System(SystemNote::MergedFrom {
                from,
                last_checkpoint: None,
            }),
            "",
        );
        d.push(ev);
        d.push(Ev::HoldEnded {
            token,
            end: HoldEnd::EndedByClose(forum_core::EndCause::Merge),
        });
        d.push(Ev::NodeClosed {
            node: from,
            outcome: Outcome::Superseded,
            merged_into: Some(into),
            by: None,
        });
    });
    assert_reloads(&w, "merge");
    assert_eq!(w.s.requests[&r].node, into);
}

#[test]
fn capacity_changes_with_holds_in_one_group() {
    let (mut w, s1) = with_session();
    let s2 = w.s.counters.next_session;
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    let n = keyed(&mut w, "n", s1, 2, &[]);
    w.run(TxKind::Cmd, |d| {
        let ev = issued(d, n, s1);
        d.push(ev);
        let ev = issued(d, n, s2);
        d.push(ev);
    });
    // A hold ends and the capacity drops to what is left.
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::HoldEnded {
            token: Token {
                node: n,
                fence: Fence(1),
            },
            end: HoldEnd::Released,
        });
        d.push(Ev::NodeUpdated {
            node: n,
            patch: NodePatch {
                capacity: Some(1),
                ..NodePatch::default()
            },
        });
    });
    assert_reloads(&w, "capacity down");
    // The capacity rises and a new hold takes the slot.
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::NodeUpdated {
            node: n,
            patch: NodePatch {
                capacity: Some(2),
                ..NodePatch::default()
            },
        });
        let ev = issued(d, n, s1);
        d.push(ev);
    });
    assert_reloads(&w, "capacity up");
    assert_eq!(w.s.holds.len(), 2);
    // Written the other way round, the new hold would have met the old capacity: the order of
    // the passes is what keeps the trigger quiet.
    let conn = Connection::open_in_memory().unwrap();
    super::schema::create_tables(&conn).unwrap();
    let mut rows = read_rows(&w.conn, Which::All).unwrap();
    rows.retain(|r| r.table() != super::state::Table::Ops);
    let node_row = rows
        .iter()
        .find(|r| r.kind == kind(&Fact::NodeContent(n)) && r.key == n.0.to_string())
        .unwrap()
        .clone();
    let mut small = node_row.clone();
    small.value = small.value.replace("\"capacity\":2", "\"capacity\":1");
    super::state::put_row(&conn, &small).unwrap();
    let holds: Vec<_> = rows
        .iter()
        .filter(|r| r.table() == super::state::Table::Holds)
        .collect();
    super::state::put_row(&conn, holds[0]).unwrap();
    let e = super::state::put_row(&conn, holds[1]).unwrap_err();
    assert!(e.is_constraint(), "{e}");
    assert!(e.to_string().contains("holders exceed capacity"), "{e}");
}

#[test]
fn a_hold_that_ends_and_another_that_starts_on_one_node_in_one_group() {
    let (mut w, s1) = with_session();
    let n = keyed(&mut w, "n", s1, 1, &[]);
    w.run(TxKind::Cmd, |d| {
        let ev = issued(d, n, s1);
        d.push(ev);
    });
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::HoldEnded {
            token: Token {
                node: n,
                fence: Fence(1),
            },
            end: HoldEnd::Released,
        });
        let ev = issued(d, n, s1);
        d.push(ev);
    });
    assert_reloads(&w, "hold handed on");
    let fences: Vec<i64> = w
        .conn
        .prepare("SELECT fence FROM st_holds")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(fences, vec![2]);
}

#[test]
fn the_op_row_is_written_whole_in_step_four_and_leaves_with_its_record() {
    let (mut w, s1) = with_session();
    let class_a = class("coder");
    let op = OpId::new("op-7").unwrap();
    let mut d = w.draft();
    let p = d.s.params.clone();
    d.push(Ev::OpRecorded {
        class: class_a.clone(),
        op: op.clone(),
        digest: ReqDigest([9; 32]),
        session: s1,
        expires_at: d.clock.plus(p.op_retain),
        outcome: Ok(Done {
            reply: forum_core::Reply::Status,
            warnings: Vec::new(),
            opened: None,
            stale_session: false,
            delivered: Vec::new(),
        }),
    });
    // Steps 1 to 3 alone write no st_ops row: pass 2 skips it.
    let mut dirty = forum_core::Tracked::new();
    let events: Vec<Event> = d
        .evs
        .iter()
        .map(|ev| Event {
            at: w.clock,
            ev: ev.clone(),
        })
        .collect();
    for e in &events {
        forum_core::evolve_tracked(&mut w.s, e, &mut dirty);
    }
    let tx = w.conn.transaction().unwrap();
    super::write(
        &tx,
        &super::TxMeta::new(TxKind::Cmd, w.clock),
        &events,
        &w.s,
        &dirty,
    )
    .unwrap();
    assert!(
        super::state::stored_op(&tx, "coder", "op-7")
            .unwrap()
            .is_none()
    );
    // Step 4 writes the record, its masking version, and the rendered response.
    super::state::put_op(&tx, &w.s, &class_a, &op, 3, b"the response bytes").unwrap();
    tx.commit().unwrap();
    let stored = super::state::stored_op(&w.conn, "coder", "op-7")
        .unwrap()
        .unwrap();
    assert_eq!(stored.masking_version, 3);
    assert_eq!(stored.response, b"the response bytes");
    assert_eq!(
        stored.value,
        serde_json::to_string(&w.s.ops[&(class_a.clone(), op.clone())]).unwrap()
    );
    assert_reloads(&w, "op");
    // A record that no longer exists has no row to write.
    assert!(
        super::state::put_op(&w.conn, &w.s, &class_a, &OpId::new("nope").unwrap(), 1, b"").is_err()
    );
    // After op.retain the next event drops the record, and its row goes with it.
    let later = w.clock.plus(p.op_retain);
    assert!(w.catch_up(later));
    w.at(later);
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    assert!(w.s.ops.is_empty());
    assert!(
        super::state::stored_op(&w.conn, "coder", "op-7")
            .unwrap()
            .is_none()
    );
    assert_reloads(&w, "op expired");
}

/// One transaction's events, drafted from the state at its start.
type Step = Box<dyn Fn(&Draft) -> Vec<Ev>>;

#[test]
fn a_node_event_rewrites_only_the_nodes_hot_row() {
    use std::sync::{Arc, Mutex};
    let (mut w, s1) = with_session();
    let n = keyed(&mut w, "n", s1, 1, &["chat:k/1"]);
    let seen: Arc<Mutex<Vec<(String, i64)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    w.conn
        .update_hook(Some(
            move |_: rusqlite::hooks::Action, _: &str, table: &str, rowid: i64| {
                sink.lock().unwrap().push((table.to_owned(), rowid));
            },
        ))
        .unwrap();
    let token = Token {
        node: n,
        fence: Fence(1),
    };
    let steps: Vec<Step> = vec![
        Box::new(move |d| vec![issued(d, n, s1)]),
        Box::new(move |d| {
            vec![appended(
                d,
                n,
                s1,
                EntryBody::Checkpoint {
                    token,
                    release: None,
                    pr_named: None,
                    handed_off_until: None,
                },
                "step",
            )]
        }),
        Box::new(move |d| {
            vec![Ev::HoldExtended {
                token,
                lease_until: d.clock.plus(d.s.params.lease_default),
                by: forum_core::Extension::Renew,
            }]
        }),
        Box::new(move |_| vec![Ev::OfferSet { node: n, on: true }]),
        Box::new(move |d| {
            vec![appended(
                d,
                n,
                s1,
                EntryBody::Note {
                    to: None,
                    until: None,
                    recipients: BTreeSet::new(),
                    pending_holder: false,
                    expects: None,
                    reply_to: None,
                },
                "a plain note",
            )]
        }),
        Box::new(move |_| {
            vec![Ev::HoldEnded {
                token,
                end: HoldEnd::Released,
            }]
        }),
    ];
    for step in steps {
        seen.lock().unwrap().clear();
        let mut d = w.draft();
        let evs = step(&d);
        // A lease extension changes the hold's row only.
        let hot = !evs.iter().all(|e| matches!(e, Ev::HoldExtended { .. }));
        for ev in evs {
            d.push(ev);
        }
        w.commit(TxKind::Cmd, d);
        let touched = seen.lock().unwrap().clone();
        assert!(
            !touched.iter().any(|(t, _)| t == "st_nodes"),
            "the content row was rewritten: {touched:?}"
        );
        assert_eq!(
            touched
                .iter()
                .any(|(t, id)| t == "st_node_hot" && *id == n.0 as i64),
            hot,
            "the hot row: {touched:?}"
        );
    }
    // A content change rewrites the content row.
    seen.lock().unwrap().clear();
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::NodeUpdated {
            node: n,
            patch: NodePatch {
                title: Some(Text("a new title".into())),
                ..NodePatch::default()
            },
        });
    });
    assert!(seen.lock().unwrap().iter().any(|(t, _)| t == "st_nodes"));
    w.conn
        .update_hook(None::<fn(rusqlite::hooks::Action, &str, &str, i64)>)
        .unwrap();
    assert_reloads(&w, "hot rows");
}

/// Runs a statement that must fail on a constraint; returns the message.
fn refused(conn: &Connection, sql: &str) -> String {
    let e = conn
        .execute(sql, [])
        .map_err(super::StoreError::from)
        .unwrap_err();
    assert!(e.is_constraint(), "{sql}: {e}");
    e.to_string()
}

#[test]
fn a_corrupted_state_row_trips_its_constraint() {
    let (mut w, s1) = with_session();
    let a = keyed(&mut w, "a", s1, 1, &["chat:k/1"]);
    let b = keyed(&mut w, "b", s1, 1, &[]);
    w.run(TxKind::Cmd, |d| {
        let ev = issued(d, a, s1);
        d.push(ev);
        let p = d.s.params.clone();
        d.push(Ev::OpRecorded {
            class: class("coder"),
            op: OpId::new("x").unwrap(),
            digest: ReqDigest([1; 32]),
            session: s1,
            expires_at: d.clock.plus(p.op_retain),
            outcome: Ok(Done {
                reply: forum_core::Reply::Status,
                warnings: Vec::new(),
                opened: None,
                stale_session: false,
                delivered: Vec::new(),
            }),
        });
    });
    // A second open carrier of a key (K1's partial unique index); a closed one is fine.
    refused(
        &w.conn,
        &format!(
            "INSERT INTO st_keys (key, node, nv, open) VALUES ('chat:k/1', {}, 1, 1)",
            b.0
        ),
    );
    w.conn
        .execute(
            &format!(
                "INSERT INTO st_keys (key, node, nv, open) VALUES ('chat:k/1', {}, 1, 0)",
                b.0
            ),
            [],
        )
        .unwrap();
    w.conn
        .execute(&format!("DELETE FROM st_keys WHERE node = {}", b.0), [])
        .unwrap();
    // A hold over capacity, and a capacity below the holds.
    assert!(
        refused(
            &w.conn,
            &format!(
                "INSERT INTO st_holds (node, fence, v) VALUES ({}, 9, '{{}}')",
                a.0
            )
        )
        .contains("holders exceed capacity")
    );
    assert!(
        refused(
            &w.conn,
            &format!("UPDATE st_nodes SET capacity = 0 WHERE id = {}", a.0)
        )
        .contains("capacity below live holds")
    );
    // A second row of one name under one parent (K3), a name and an alias alike.
    refused(
        &w.conn,
        &format!(
            "INSERT INTO st_names (parent, name, node, alias) VALUES (0, 'a', {}, 1)",
            b.0
        ),
    );
    // A second record of one (class, op_id) (O1).
    refused(
        &w.conn,
        "INSERT INTO st_ops (class, op_id, v, masking_version, response) \
         VALUES ('coder', 'x', '{}', 1, x'')",
    );
    // The writer meets the same constraints when the state it writes is broken (a core bug).
    let mut bad = w.s.clone();
    bad.nodes
        .get_mut(&b)
        .unwrap()
        .keys
        .insert(keystr("chat:k/1"), NormVer(1));
    let tx = w.conn.unchecked_transaction().unwrap();
    let e = write_dirty(
        &tx,
        &bad,
        &[Fact::Key(b, keystr("chat:k/1"))].into(),
        &BTreeMap::new(),
    )
    .unwrap_err();
    assert!(e.is_constraint(), "{e}");
    tx.rollback().unwrap();
    let mut bad = w.s.clone();
    let extra = Token {
        node: a,
        fence: Fence(2),
    };
    let h = bad.holds.values().next().unwrap().clone();
    bad.holds.insert(extra, h);
    let tx = w.conn.unchecked_transaction().unwrap();
    let e = write_dirty(&tx, &bad, &[Fact::Hold(extra)].into(), &BTreeMap::new()).unwrap_err();
    assert!(e.is_constraint(), "{e}");
    tx.rollback().unwrap();
    // A name and an alias at one (parent, name) (K3): the alias's row must not overwrite the
    // name's.
    let mut bad = w.s.clone();
    let name_a = Name::new("a").unwrap();
    assert_eq!(bad.names.get(&(ROOT, name_a.clone())), Some(&a));
    bad.aliases.insert((ROOT, name_a.clone()), b);
    let tx = w.conn.unchecked_transaction().unwrap();
    let e = write_dirty(
        &tx,
        &bad,
        &[Fact::Alias(ROOT, name_a)].into(),
        &BTreeMap::new(),
    )
    .unwrap_err();
    assert!(e.is_constraint(), "{e}");
    tx.rollback().unwrap();
    // A second record of a live (class, op_id), which evolve refuses and so does not mark dirty
    // (O1): step 4's row must not replace the stored response.
    let tx = w.conn.unchecked_transaction().unwrap();
    let e = super::state::put_op(
        &tx,
        &w.s,
        &class("coder"),
        &OpId::new("x").unwrap(),
        1,
        b"a different response",
    )
    .unwrap_err();
    assert!(e.is_constraint(), "{e}");
    tx.rollback().unwrap();
    assert_eq!(
        super::state::stored_op(&w.conn, "coder", "x")
            .unwrap()
            .unwrap()
            .response,
        b"response x"
    );
    assert_reloads(&w, "after the refused writes");
}

#[test]
fn a_record_that_expires_and_is_recorded_again_in_one_job_is_written_whole() {
    let (mut w, s1) = with_session();
    let class_a = class("coder");
    let op = OpId::new("op-9").unwrap();
    let record = |d: &mut Draft, session: SessionId| {
        let p = d.s.params.clone();
        let ev = Ev::OpRecorded {
            class: class("coder"),
            op: OpId::new("op-9").unwrap(),
            digest: ReqDigest([7; 32]),
            session,
            expires_at: d.clock.plus(p.op_retain),
            outcome: Ok(Done {
                reply: forum_core::Reply::Status,
                warnings: Vec::new(),
                opened: None,
                stale_session: false,
                delivered: Vec::new(),
            }),
        };
        d.push(ev);
    };
    // A minute in, so that s-1's retirement fires before the record expires.
    w.at(w.clock.plus(MINUTE));
    w.run(TxKind::Cmd, |d| record(d, s1));
    let first = super::state::stored_op(&w.conn, "coder", "op-9")
        .unwrap()
        .unwrap();
    // At op.retain the job's first event prunes the record, and the call records it again: pass 1
    // deletes the old row and step 4 inserts the new one.
    let later = w.clock.plus(w.s.params.op_retain);
    assert!(w.catch_up(later));
    assert!(w.s.ops.contains_key(&(class_a.clone(), op.clone())));
    w.at(later);
    let s2 = w.s.counters.next_session;
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
        record(d, s2);
    });
    let second = super::state::stored_op(&w.conn, "coder", "op-9")
        .unwrap()
        .unwrap();
    assert_ne!(first.value, second.value);
    assert_eq!(
        second.value,
        serde_json::to_string(&w.s.ops[&(class_a, op)]).unwrap()
    );
    assert_reloads(&w, "recorded again");
}

#[test]
fn the_history_triggers_abort_updates_and_deletes() {
    let (mut w, s1) = with_session();
    let _ = keyed(&mut w, "a", s1, 1, &[]);
    for sql in [
        "UPDATE txs SET at = 0",
        "DELETE FROM txs",
        "UPDATE events SET at = 0",
        "DELETE FROM events",
        "UPDATE event_nodes SET seq = seq + 1000",
        "DELETE FROM event_nodes",
    ] {
        let e = w
            .conn
            .execute(sql, [])
            .map_err(super::StoreError::from)
            .unwrap_err();
        assert!(e.is_constraint(), "{sql}: {e}");
        assert!(
            e.to_string().contains("the history is append-only"),
            "{sql}: {e}"
        );
    }
    assert_reloads(&w, "after the refused statements");
    verify(&w.conn).unwrap();
}

#[test]
fn a_row_that_does_not_decode_stops_startup_with_its_table_and_key() {
    let db = TempDb::new("corrupt");
    let mut w = World::from_opened(fresh_file(&db, test_params()));
    let s1 = w.s.counters.next_session;
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    let n = keyed(&mut w, "a", s1, 1, &["chat:k/1"]);
    let want = w.s.clone();
    drop(w);
    let reopen = || {
        super::load::open(
            &db.path,
            &super::testkit::genesis_args(test_params()),
            &super::migrate::RELEASED,
        )
    };
    let session = i64::from(kind(&Fact::Session(s1)));
    let node = n.0 as i64;
    let scalars: String = Connection::open(&db.path)
        .unwrap()
        .query_row("SELECT v FROM st_scalars", [], |r| r.get(0))
        .unwrap();
    // (corrupt, repair, table, key): a row of st_facts is named by its fact's variant and key.
    let cases: Vec<(String, String, &str, String)> = vec![
        (
            format!("UPDATE st_facts SET v = '{{not json' WHERE fact = {session}"),
            String::new(),
            "st_facts",
            format!("Session {}", s1.0),
        ),
        // Integer and boolean columns that do not fit their types.
        (
            format!("UPDATE st_facts SET resident = 7 WHERE fact = {session}"),
            format!("UPDATE st_facts SET resident = 1 WHERE fact = {session}"),
            "st_facts",
            format!("Session {}", s1.0),
        ),
        (
            format!("UPDATE st_keys SET nv = 70000 WHERE node = {node}"),
            format!("UPDATE st_keys SET nv = 1 WHERE node = {node}"),
            "st_keys",
            format!("[{},\"chat:k/1\"]", n.0),
        ),
        (
            format!("UPDATE st_keys SET open = 2 WHERE node = {node}"),
            format!("UPDATE st_keys SET open = 1 WHERE node = {node}"),
            "st_keys",
            format!("[{},\"chat:k/1\"]", n.0),
        ),
        (
            format!("UPDATE st_keys SET nv = 'one' WHERE node = {node}"),
            format!("UPDATE st_keys SET nv = 1 WHERE node = {node}"),
            "st_keys",
            format!("[{},\"chat:k/1\"]", n.0),
        ),
        // No scalar row: the counters would start again, and the first write would reuse an
        // event's seq.
        (
            "DELETE FROM st_scalars".to_owned(),
            format!(
                "INSERT INTO st_scalars (one, v) VALUES (0, '{}')",
                scalars.replace('\'', "''")
            ),
            "st_scalars",
            "null".to_owned(),
        ),
        (
            format!("UPDATE st_node_hot SET v = '[1]' WHERE id = {node}"),
            String::new(),
            "st_node_hot",
            n.0.to_string(),
        ),
        (
            format!("UPDATE st_nodes SET capacity = 7 WHERE id = {node}"),
            format!("UPDATE st_nodes SET capacity = 1 WHERE id = {node}"),
            "st_nodes",
            n.0.to_string(),
        ),
        (
            format!("UPDATE st_keys SET open = 0 WHERE node = {node}"),
            format!("UPDATE st_keys SET open = 1 WHERE node = {node}"),
            "st_keys",
            format!("[{},\"chat:k/1\"]", n.0),
        ),
        (
            format!("DELETE FROM st_node_hot WHERE id = {node}"),
            String::new(),
            "st_node_hot",
            n.0.to_string(),
        ),
    ];
    for (i, (corrupt, repair, table, key)) in cases.iter().enumerate() {
        let conn = Connection::open(&db.path).unwrap();
        let saved = (
            dump_table(&conn, "st_facts"),
            dump_table(&conn, "st_node_hot"),
        );
        let facts: Vec<(i64, String, String, i64)> = conn
            .prepare("SELECT fact, k, v, resident FROM st_facts")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let hot: Vec<(i64, String)> = conn
            .prepare("SELECT id, v FROM st_node_hot")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        conn.execute_batch(corrupt).unwrap();
        drop(conn);
        match reopen() {
            Err(super::StoreError::Decode {
                table: t, key: k, ..
            }) => {
                assert_eq!((t, k.as_str()), (*table, key.as_str()), "case {i}");
            }
            other => panic!("case {i}: {other:?}"),
        }
        // Put the rows back as they were.
        let conn = Connection::open(&db.path).unwrap();
        if repair.is_empty() {
            conn.execute_batch("DELETE FROM st_facts; DELETE FROM st_node_hot;")
                .unwrap();
            for (f, k, v, r) in &facts {
                conn.execute(
                    "INSERT INTO st_facts (fact, k, v, resident) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![f, k, v, r],
                )
                .unwrap();
            }
            for (id, v) in &hot {
                conn.execute(
                    "INSERT INTO st_node_hot (id, v) VALUES (?1, ?2)",
                    rusqlite::params![id, v],
                )
                .unwrap();
            }
        } else {
            conn.execute_batch(repair).unwrap();
        }
        assert_eq!(
            (
                dump_table(&conn, "st_facts"),
                dump_table(&conn, "st_node_hot")
            ),
            saved
        );
        drop(conn);
        let o = reopen().unwrap();
        assert_eq!(differences(&o.state, &want), Vec::<&str>::new(), "case {i}");
    }
    // `replay --into` writes new state tables from the history, a last resort for damaged ones.
    let conn = Connection::open(&db.path).unwrap();
    conn.execute_batch(&format!(
        "UPDATE st_facts SET v = '{{not json' WHERE fact = {session}"
    ))
    .unwrap();
    let fixed = Connection::open_in_memory().unwrap();
    let tx = fixed.unchecked_transaction().unwrap();
    let s = replay_into(&conn, &tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(differences(&s, &want), Vec::<&str>::new());
    assert_eq!(
        differences(&load_state(&fixed).unwrap(), &want),
        Vec::<&str>::new()
    );
}

#[test]
fn the_shell_tables_follow_the_events() {
    let mut w = World::memory(test_params());
    super::testkit::scenario(&mut w);
    let c = &w.conn;
    let count = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() };
    // Two sessions retired; the third is live.
    assert_eq!(count("SELECT count(*) FROM sessions"), 1);
    assert_eq!(
        count("SELECT count(*) FROM sessions WHERE intent_ev IS NOT NULL"),
        1
    );
    // The confirmed reply's link, the request's origin, and no pause after its expiry.
    assert_eq!(count("SELECT count(*) FROM said_links"), 1);
    assert_eq!(
        count("SELECT count(*) FROM origins WHERE origin = 'chat:team/1/m1' AND nv = 1"),
        1
    );
    assert_eq!(count("SELECT count(*) FROM pauses"), 0);
    // Every entry has a row, checkpoints and plain notes included, each pointing at its event.
    let entries = count("SELECT count(*) FROM entries");
    assert_eq!(entries as u64, w.s.counters.next_eid.0 - 1);
    assert_eq!(
        count(
            "SELECT count(*) FROM entries e WHERE NOT EXISTS (SELECT 1 FROM events v WHERE v.seq = e.ev)"
        ),
        0
    );
    assert_eq!(
        count("SELECT count(*) FROM entries WHERE type = 'checkpoint'"),
        2
    );
    // A hidden entry and a hidden file point at their Hidden events.
    let (mut w, s1) = with_session();
    let n = keyed(&mut w, "n", s1, 1, &[]);
    w.run(TxKind::Cmd, |d| {
        let mut files = BTreeMap::new();
        files.insert(Text("secret.txt".into()), Text("https://x/secret".into()));
        d.push(Ev::NodeUpdated {
            node: n,
            patch: NodePatch {
                files: Some(files),
                ..NodePatch::default()
            },
        });
    });
    let e = w.s.counters.next_eid;
    w.run(TxKind::Cmd, |d| {
        let ev = appended(d, n, s1, EntryBody::Object, "the key is hunter2");
        d.push(ev);
    });
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::Hidden {
            target: forum_core::HideTarget::Entry(e),
            reason: Text("a secret".into()),
        });
        d.push(Ev::Hidden {
            target: forum_core::HideTarget::File {
                node: n,
                name: Text("secret.txt".into()),
            },
            reason: Text("a secret".into()),
        });
    });
    let last = w.s.counters.events as i64;
    let c = &w.conn;
    let hidden: Option<i64> = c
        .query_row(
            "SELECT hidden_ev FROM entries WHERE eid = ?1",
            [e.0 as i64],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hidden, Some(last - 1));
    let file: i64 = c
        .query_row(
            "SELECT ev FROM hidden_files WHERE name = 'secret.txt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(file, last);
    assert_reloads(&w, "hidden");
}

#[test]
fn view_history_pages_a_nodes_events_newest_first() {
    let mut w = World::memory(test_params());
    super::testkit::scenario(&mut w);
    let work = NodeId(8);
    let all = node_history(&w.conn, work, None, 1_000).unwrap();
    assert!(all.len() > 10, "{all:?}");
    assert!(all.windows(2).all(|p| p[0] > p[1]));
    // Every event in the page names the node.
    for seq in &all {
        let e = super::history::event_at(&w.conn, *seq).unwrap().unwrap();
        assert!(super::history::nodes_named(&e).contains(&work), "{e:?}");
    }
    let page = node_history(&w.conn, work, Some(all[2]), 3).unwrap();
    assert_eq!(page, all[3..6].to_vec());
}

#[test]
fn an_event_files_under_every_node_it_names() {
    use super::history::nodes_named;
    let ev = |ev| Event { at: Time(1), ev };
    let routed = ev(Ev::EntryRouted {
        entry: forum_core::Eid(5),
        routing: forum_core::Routing::Request {
            node: NodeId(42),
            request: forum_core::Eid(6),
        },
        reason: None,
    });
    assert_eq!(nodes_named(&routed), [NodeId(42)].into());
    let elsewhere = ev(Ev::EntryRouted {
        entry: forum_core::Eid(5),
        routing: forum_core::Routing::NoAction,
        reason: None,
    });
    assert_eq!(nodes_named(&elsewhere), BTreeSet::new());
    let updated = ev(Ev::NodeUpdated {
        node: NodeId(9),
        patch: NodePatch {
            depends_on: Some([NodeId(77)].into()),
            owners: Some([NodeId(88)].into()),
            ..NodePatch::default()
        },
    });
    assert_eq!(
        nodes_named(&updated),
        [NodeId(9), NodeId(77), NodeId(88)].into()
    );
    // view history lists them under each node.
    let (mut w, s1) = with_session();
    let a = keyed(&mut w, "a", s1, 1, &[]);
    let b = keyed(&mut w, "b", s1, 1, &[]);
    let owners = keyed(&mut w, "owners", s1, 1, &[]);
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::NodeUpdated {
            node: a,
            patch: NodePatch {
                depends_on: Some([b].into()),
                owners: Some([owners].into()),
                ..NodePatch::default()
            },
        });
    });
    let last = w.s.counters.events;
    for n in [a, b, owners] {
        assert_eq!(
            node_history(&w.conn, n, None, 1).unwrap(),
            vec![last],
            "{n:?}"
        );
    }
}

// ------------------------------------------------------------------------------------------------
// Restores (D.4.12)

#[test]
fn a_command_names_what_it_restores() {
    use forum_core::{BasisArg, Cmd, HoldCmd, NodeCmd, NodeRef, TalkCmd, WaitArg};
    let s = World::memory(test_params()).s;
    let at = NodeRef::Id(NodeId(8));
    let e = |n: u64| forum_core::Eid(n);
    let ents = |c: &Cmd| super::restore::named(&s, c).entities;
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::Decide {
            at: at.clone(),
            text: Text("d".into()),
            settles: [e(1)].into(),
            supersedes: Some(e(2)),
            basis: vec![
                BasisArg::Entry(e(3)),
                BasisArg::Node {
                    at: at.clone(),
                    version: None
                }
            ],
            evidence: None,
        })),
        [
            EntityRef::Request(e(1)),
            EntityRef::Decision(e(2)),
            EntityRef::Decision(e(3))
        ]
        .into()
    );
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::ToUser {
            text: Text("r".into()),
            kind: forum_core::ToUserKind::Result,
            verbatim: true,
            principal: None,
            re: [e(4)].into(),
            at: None,
        })),
        [
            EntityRef::Request(e(4)),
            EntityRef::UserEntry(e(4)),
            EntityRef::Digest(e(4))
        ]
        .into()
    );
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::FromUser {
            text: Text("t".into()),
            client: Text("c".into()),
            principal: None,
            answers: Some(e(5)),
            to: None,
            clarifies: Some(e(6)),
            reason: None,
        })),
        [EntityRef::UQuestion(e(5)), EntityRef::Request(e(6))].into()
    );
    assert_eq!(
        ents(&Cmd::Hold(HoldCmd::Release {
            token: Token {
                node: NodeId(8),
                fence: Fence(1)
            },
            done: Text("d".into()),
            next_step: Text("n".into()),
            reason: forum_core::ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Question(e(7))),
            intent_done: None,
            learnings: None,
            unsent: [e(8)].into(),
            settles: [e(12)].into(),
        })),
        [
            EntityRef::PQuestion(e(7)),
            EntityRef::UQuestion(e(7)),
            EntityRef::Said(e(8)),
            EntityRef::Request(e(12))
        ]
        .into()
    );
    assert_eq!(
        ents(&Cmd::Node(NodeCmd::Close {
            at: at.clone(),
            outcome: Outcome::Done,
            summary: Text("s".into()),
            verified: None,
            settles: Some([e(9)].into()),
            learnings: None,
            token: None,
            unsent: [e(10)].into(),
        })),
        [EntityRef::Request(e(9)), EntityRef::Said(e(10))].into()
    );
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::SaidConfirm {
            said: e(11),
            confirmed: Text("l".into()),
        })),
        [EntityRef::Said(e(11))].into()
    );
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::Said {
            at: at.clone(),
            where_: None,
            text: Text("t".into()),
            kind: SaidKind::Correction,
            corrects: Some(e(12)),
            seen: None,
        })),
        [EntityRef::Said(e(12))].into()
    );
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::Heard {
            at: at.clone(),
            where_: None,
            from: Text("Bob".into()),
            text: Text("t".into()),
            answers: Some(e(13)),
        })),
        [EntityRef::PQuestion(e(13))].into()
    );
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::Resolve {
            objection: e(14),
            note: Text("n".into()),
            upheld: false,
            defer: false,
        })),
        [EntityRef::Objection(e(14))].into()
    );
    assert_eq!(
        ents(&Cmd::Talk(TalkCmd::Route {
            entry: e(15),
            to: Some(forum_core::RouteTo::Nowhere),
            answers: None,
            clarifies: None,
            reason: Some(Text("r".into())),
        })),
        [EntityRef::UserEntry(e(15))].into()
    );
    // A request names its origin; a create's request defaults its origin as D.8.9 does.
    let origin = Key {
        key: keystr("chat:team/1/m1"),
        nv: NormVer(1),
    };
    let n = super::restore::named(
        &s,
        &Cmd::Talk(TalkCmd::Request {
            at: at.clone(),
            text: Text("t".into()),
            origin: origin.clone(),
            where_: None,
            requester: Text("Bob".into()),
        }),
    );
    assert_eq!(n.origins, [origin.key.clone()].into());
    let create = |request_origin: Option<Key>, where_: Option<Key>, keys: Vec<Key>| {
        Cmd::Node(NodeCmd::Create(forum_core::CreateArgs {
            path: forum_core::Path(vec![Name::new("x").unwrap()]),
            keys,
            kind: None,
            title: Text("t".into()),
            body: None,
            digest: None,
            fields: BTreeMap::new(),
            tags: BTreeSet::new(),
            capacity: None,
            needs: None,
            est: None,
            priority: None,
            due: None,
            depends_on: Vec::new(),
            owners: Vec::new(),
            repeat: None,
            agreements: None,
            request: Some(forum_core::RequestArg {
                text: Text("r".into()),
                origin: request_origin,
                where_,
                requester: Text("Bob".into()),
            }),
            take: None,
        }))
    };
    let k = |s: &str| Key {
        key: keystr(s),
        nv: NormVer(1),
    };
    let lean = k("lean:t1");
    let chat = k("chat:a/1");
    assert_eq!(
        super::restore::named(&s, &create(None, None, vec![lean.clone(), chat.clone()])).origins,
        [chat.key.clone()].into()
    );
    assert_eq!(
        super::restore::named(&s, &create(None, None, vec![lean.clone()])).origins,
        [lean.key.clone()].into()
    );
    assert_eq!(
        super::restore::named(&s, &create(None, Some(k("mail:m@x")), vec![chat.clone()])).origins,
        [keystr("mail:m@x")].into()
    );
    assert_eq!(
        super::restore::named(
            &s,
            &create(Some(origin.clone()), Some(chat.clone()), vec![])
        )
        .origins,
        [origin.key.clone()].into()
    );
    assert!(
        super::restore::named(&s, &Cmd::View(forum_core::ViewCmd::Constitution))
            .entities
            .is_empty()
    );
    // A create names the user entries of its `user:e<n>` keys (D.8.9 step 1).
    let user = Key {
        key: forum_core::KeyStr::of_user_entry(e(16)),
        nv: NormVer(1),
    };
    assert_eq!(
        super::restore::named(&s, &create(None, None, vec![chat.clone(), user])).entities,
        [EntityRef::UserEntry(e(16))].into()
    );
}

#[test]
fn a_create_whose_user_key_names_an_entry_that_left_memory_restores_it() {
    use forum_core::{Cmd, Ctx, NodeCmd, SessionCtx};
    let (mut w, s1) = with_session();
    let e = from_user(&mut w, s1);
    w.run(TxKind::Cmd, |d| {
        d.push(Ev::EntryRouted {
            entry: e,
            routing: forum_core::Routing::NoAction,
            reason: Some(Text("nothing to do".into())),
        });
    });
    // Routed, it leaves memory FINISHED_RETAIN later, at the next event.
    let later = w.clock.plus(forum_core::FINISHED_RETAIN).plus(MINUTE);
    assert!(w.catch_up(later));
    w.at(later);
    let s2 = w.s.counters.next_session;
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    assert!(!w.s.user_entries.contains_key(&e));
    let create = Cmd::Node(NodeCmd::Create(forum_core::CreateArgs {
        path: forum_core::Path(vec![Name::new("from-user").unwrap()]),
        keys: vec![Key {
            key: forum_core::KeyStr::of_user_entry(e),
            nv: NormVer(1),
        }],
        kind: None,
        title: Text("the user's ask".into()),
        body: None,
        digest: None,
        fields: BTreeMap::new(),
        tags: BTreeSet::new(),
        capacity: None,
        needs: None,
        est: None,
        priority: None,
        due: None,
        depends_on: Vec::new(),
        owners: Vec::new(),
        repeat: None,
        agreements: None,
        request: None,
        take: None,
    }));
    let ctx = Ctx {
        now: w.clock,
        class: class("coder"),
        caps: None,
        session: SessionCtx::Existing(s2),
        op: None,
    };
    // Without the restore, the core does not find the entry.
    let (_, out) = forum_core::apply(&mut w.s.clone(), &ctx, &create);
    assert_eq!(
        out.unwrap_err(),
        forum_core::Error::not_found(forum_core::What::UserEntry)
    );
    // The create names it, its row restores it, and then the create proceeds.
    let items =
        super::restore::items(&w.conn, &w.s, &super::restore::named(&w.s, &create)).unwrap();
    assert!(
        matches!(items.as_slice(), [forum_core::Restored::UserEntry(x, ..)] if *x == e),
        "{items:?}"
    );
    let at = w
        .grave
        .keys()
        .position(|f| *f == Fact::UserEntry(e))
        .unwrap();
    assert!(w.restore_some(|i| i == at) > 0);
    assert!(w.s.user_entries.contains_key(&e));
    let (_, out) = forum_core::apply(&mut w.s.clone(), &ctx, &create);
    assert!(out.is_ok(), "{out:?}");
    assert_reloads(&w, "restored for a create");
}

#[test]
fn a_command_that_names_an_entity_that_left_memory_restores_its_row() {
    use forum_core::{Cmd, NodeRef, TalkCmd};
    let mut w = World::memory(test_params());
    super::testkit::scenario(&mut w);
    let request = forum_core::Eid(1);
    // The scenario restored the request; let it leave again.
    let later = w.clock.plus(forum_core::Dur(8 * forum_core::DAY.0));
    assert!(w.catch_up(later));
    w.at(later);
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    assert!(!w.s.requests.contains_key(&request));
    let row = super::state::fact_row(&w.conn, &Fact::Request(request))
        .unwrap()
        .unwrap();
    assert!(!row.1);
    // `decide {settles}` names it.
    let decide = Cmd::Talk(TalkCmd::Decide {
        at: NodeRef::Id(NodeId(8)),
        text: Text("again".into()),
        settles: [request].into(),
        supersedes: None,
        basis: Vec::new(),
        evidence: None,
    });
    let items =
        super::restore::items(&w.conn, &w.s, &super::restore::named(&w.s, &decide)).unwrap();
    assert_eq!(items.len(), 1);
    match &items[0] {
        forum_core::Restored::Request(id, v, m) => {
            assert_eq!(*id, request);
            assert_eq!(serde_json::to_string(v).unwrap(), row.0);
            assert_eq!(m.node, NodeId(8));
        }
        other => panic!("{other:?}"),
    }
    // `request {origin}` names it through the origins table.
    let again = Cmd::Talk(TalkCmd::Request {
        at: NodeRef::Id(NodeId(8)),
        text: Text("again".into()),
        origin: Key {
            key: keystr("chat:team/1/m1"),
            nv: NormVer(1),
        },
        where_: None,
        requester: Text("Bob".into()),
    });
    let items = super::restore::items(&w.conn, &w.s, &super::restore::named(&w.s, &again)).unwrap();
    assert_eq!(items.len(), 1);
    // Ids at or above next_eid, resident ones, and unknown ones give nothing.
    let none = super::restore::items_of(
        &w.conn,
        &w.s,
        &[
            EntityRef::Request(w.s.counters.next_eid),
            EntityRef::Decision(forum_core::Eid(5)),
            EntityRef::Objection(forum_core::Eid(2)),
        ],
    )
    .unwrap();
    assert!(none.is_empty(), "{none:?}");
    // Restored, written, and reloaded.
    let n = w.restore_some(|_| true);
    assert!(n > 0);
    assert_reloads(&w, "restored");
}

#[test]
fn a_second_request_with_a_departed_requests_origin_trips_the_origins_key() {
    let mut w = World::memory(test_params());
    super::testkit::scenario(&mut w);
    let later = w.clock.plus(forum_core::Dur(8 * forum_core::DAY.0));
    assert!(w.catch_up(later));
    w.at(later);
    let s = w.s.counters.next_session;
    w.run(TxKind::Cmd, |d| {
        let ev = session_opened(d, "coder");
        d.push(ev);
    });
    assert!(!w.s.requests.contains_key(&forum_core::Eid(1)));
    // The core no longer sees the first request, so it would accept a second with its origin;
    // the shell's origins table refuses it (Q1 across memory, a second line of defence).
    let mut d = w.draft();
    let ev = appended(
        &d,
        NodeId(8),
        s,
        EntryBody::Request {
            origin: Key {
                key: keystr("chat:team/1/m1"),
                nv: NormVer(1),
            },
            conv: keystr("chat:team/1"),
            from_user: None,
            requester: Requester::External,
        },
        "the same message again",
    );
    d.push(ev);
    let mut s2 = w.s.clone();
    let mut dirty = forum_core::Tracked::new();
    let events: Vec<Event> = d
        .evs
        .into_iter()
        .map(|ev| Event { at: w.clock, ev })
        .collect();
    for e in &events {
        forum_core::evolve_tracked(&mut s2, e, &mut dirty);
    }
    let tx = w.conn.unchecked_transaction().unwrap();
    let e = super::write(
        &tx,
        &super::TxMeta::new(TxKind::Cmd, w.clock),
        &events,
        &s2,
        &dirty,
    )
    .unwrap_err();
    assert!(e.is_constraint(), "{e}");
    tx.rollback().unwrap();
}

#[test]
fn the_linked_sqlite_has_fts5_with_contentless_delete() {
    let p = super::schema::probe_fts5().unwrap();
    assert!(p.compiled, "{p:?}");
    assert!(p.contentless_delete, "{p:?}");
}

// ------------------------------------------------------------------------------------------------
// Migrations (D.7.6, D.11.8, D.12.6)

/// The database of `state_version` 1 and `shell_version` 1 that this release writes for the
/// scenario of `testkit::scenario`, frozen as SQL by `write_the_v1_fixture`. A later release keeps
/// it and adds one per version, so every released version is migrated and loaded in CI.
const FIXTURE_V1: &str = include_str!("fixtures/state_v1.sql");

/// Denies every read of a history table on `conn`, so that a statement that reads one fails.
fn deny_history_reads(conn: &Connection) {
    conn.authorizer(Some(|ctx: rusqlite::hooks::AuthContext<'_>| {
        match ctx.action {
            rusqlite::hooks::AuthAction::Read { table_name, .. }
                if HISTORY_TABLES.contains(&table_name) =>
            {
                rusqlite::hooks::Authorization::Deny
            }
            _ => rusqlite::hooks::Authorization::Allow,
        }
    }))
    .unwrap();
}

fn allow_everything(conn: &Connection) {
    conn.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
}

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(FIXTURE_V1).unwrap();
    conn
}

/// The state of the fixture, loaded with the released steps.
fn fixture_state() -> State {
    load_state(&fixture()).unwrap()
}

/// A state step that rewrites every row's value through a generic JSON value: a migration that
/// changes the stored text of every row and nothing it decodes to.
fn reserialise(r: super::state::Row) -> Result<Vec<super::state::Row>, String> {
    let v: serde_json::Value = serde_json::from_str(&r.value).map_err(|e| e.to_string())?;
    Ok(vec![super::state::Row {
        value: v.to_string(),
        ..r
    }])
}

/// A state step that refuses the first session row.
fn refuse_sessions(r: super::state::Row) -> Result<Vec<super::state::Row>, String> {
    if r.kind == kind(&Fact::Session(SessionId(1))) {
        Err("a session row this step cannot read".to_owned())
    } else {
        Ok(vec![r])
    }
}

/// A shell step that adds a column derived from the row itself: the type's length.
fn add_kind_code(conn: &Connection) -> super::Result<()> {
    conn.execute_batch(
        "ALTER TABLE entries ADD COLUMN kind_code INTEGER;
         UPDATE entries SET kind_code = length(type);",
    )?;
    Ok(())
}

/// What the shell's writer of version 2 adds to version 1's rows.
fn kind_code_v2(conn: &Connection) {
    conn.execute_batch("UPDATE entries SET kind_code = length(type) WHERE kind_code IS NULL")
        .unwrap();
}

const TO_V2: super::migrate::Steps = super::migrate::Steps {
    state: &[(1, reserialise)],
    shell: &[(1, add_kind_code)],
    state_version: 2,
    shell_version: 2,
};

#[test]
#[ignore = "writes the fixture of this release's state_version; run by hand when a version is released"]
fn write_the_v1_fixture() {
    let mut w = World::memory(test_params());
    super::testkit::scenario(&mut w);
    let sql = super::testkit::dump_sql(&w.conn);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store/fixtures/state_v1.sql");
    std::fs::write(path, sql).unwrap();
}

#[test]
fn the_v1_fixture_loads_without_reading_the_history_and_passes_every_check() {
    let conn = fixture();
    deny_history_reads(&conn);
    // The authorizer is effective: a read of the history fails.
    assert!(
        conn.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .is_err()
    );
    let o = super::load::open_connection(
        conn,
        &super::testkit::genesis_args(test_params()),
        &super::migrate::RELEASED,
    )
    .unwrap();
    assert!(!o.created);
    assert!(o.migrated.is_empty());
    check_loaded(&o.state, Checks::Full).unwrap();
    // The fixture keeps rows that left memory, and the history replays to its tables.
    let conn = o.store.into_connection();
    allow_everything(&conn);
    let gone: i64 = conn
        .query_row(
            "SELECT count(*) FROM st_facts WHERE resident = 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(gone > 0);
    let replayed = verify_checked(&conn).unwrap();
    assert_eq!(differences(&replayed, &o.state), Vec::<&str>::new());
    // Its shell's tables equal a rebuild from the history.
    let before = dump(&conn, &SHELL_TABLES);
    let tx = conn.unchecked_transaction().unwrap();
    rebuild_shell_tables(&tx).unwrap();
    assert_eq!(dump(&tx, &SHELL_TABLES), before);
    tx.rollback().unwrap();
}

/// Removes from a stored event's JSON the fields that 1.12 appended to an entry and an intent
/// (D.7.6), so that it reads as a log written before 1.12 holds it.
fn as_before_1_12(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(m) => {
            m.remove("pr_named");
            m.remove("handed_off_until");
            m.values_mut().for_each(as_before_1_12);
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(as_before_1_12),
        _ => {}
    }
}

/// 1.12 (D.7.6): a history written before 1.12 replays to the state tables it wrote. The state
/// tables are the source of truth and are loaded at startup, never rebuilt from the history; the
/// replay check of CI and `forum-server replay` compares the two. 1.12 keeps, on the node, whether
/// its last `intent_done` (or its open intent) names a pull request, and, on the session, the node
/// it handed off: both from flags its entries carry, never from their texts, which an older log
/// holds too. So a log of 1.11's shape, whose texts name a pull request and whose session handed
/// a node off and stays live, replays to tables that hold neither, as 1.11 wrote them.
#[test]
fn a_history_written_before_1_12_replays_to_the_tables_it_wrote() {
    use forum_core::event::{EntryText, TextField};
    let (mut w, s1) = with_session();
    let n = create_node(&mut w, ROOT, "flaky", s1, init(1, &[]));
    let token = Token {
        node: n,
        fence: Fence(1),
    };
    w.run(TxKind::Cmd, |d| {
        let ev = issued(d, n, s1);
        d.push(ev);
    });
    let pr = |main: &str, f: TextField, t: &str| EntryText {
        main: Text(main.into()),
        extra: vec![
            (TextField::NextStep, Text("go on".into())),
            (f, Text(t.into())),
        ],
    };
    let checkpoint = |release| EntryBody::Checkpoint {
        token,
        release,
        pr_named: None,
        handed_off_until: None,
    };
    let entry = |d: &Draft, body, text| Ev::Appended {
        eid: d.s.counters.next_eid,
        node: n,
        seq: Seq(d.s.nodes[&n].seq.0 + 1),
        by: forum_core::Author::Session(s1),
        body,
        text,
    };
    // An intent that names a pull request, closed by an `intent_done` that names one, then by a
    // `hold check` that names one, and a `handoff` release: each as 1.11 recorded it.
    for done in [true, false] {
        w.at(w.clock.plus(MINUTE));
        w.run(TxKind::Cmd, |d| {
            let eid = d.s.counters.next_eid;
            let ev = entry(
                d,
                checkpoint(None),
                pr("fixed it", TextField::Intent, "push the fix and open a PR"),
            );
            d.push(ev);
            d.push(Ev::IntentOpened {
                node: n,
                intent: forum_core::Intent {
                    eid,
                    session: s1,
                    pr_named: false,
                },
            });
        });
        w.at(w.clock.plus(MINUTE));
        w.run(TxKind::Cmd, |d| {
            let intent = d.s.nodes[&n].open_intent.unwrap().eid;
            let eid = d.s.counters.next_eid;
            let ev = if done {
                entry(
                    d,
                    checkpoint(None),
                    pr(
                        "opened it",
                        TextField::IntentDone,
                        "pushed and opened PR #3",
                    ),
                )
            } else {
                entry(
                    d,
                    EntryBody::IntentChecked {
                        intent,
                        pr_named: false,
                    },
                    EntryText {
                        main: Text("opened PR #4".into()),
                        extra: Vec::new(),
                    },
                )
            };
            d.push(ev);
            d.push(Ev::IntentClosed {
                node: n,
                end: forum_core::IntentEnd::Done { by: eid },
            });
        });
    }
    w.at(w.clock.plus(MINUTE));
    w.run(TxKind::Cmd, |d| {
        let ev = entry(
            d,
            checkpoint(Some(forum_core::ReleaseReason::Handoff)),
            pr("handing off", TextField::IntentDone, "opened PR #5"),
        );
        d.push(ev);
        d.push(Ev::HoldEnded {
            token,
            end: HoldEnd::Released,
        });
    });
    // The stored events, rewritten in the JSON of a log written before 1.12 (the test lifts the
    // history's append-only trigger to fake the older log).
    w.conn
        .execute_batch("DROP TRIGGER events_no_update")
        .unwrap();
    let rows: Vec<(i64, i64, Vec<u8>)> = w
        .conn
        .prepare("SELECT seq, format, z FROM events ORDER BY seq")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut rewritten = 0;
    for (seq, format, z) in rows {
        let e = decode_event(format, &z).unwrap();
        let mut v = serde_json::to_value(&e).unwrap();
        let before = v.clone();
        as_before_1_12(&mut v);
        if v == before {
            continue;
        }
        rewritten += 1;
        let old: Event = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(old, e, "a 1.11 event decodes to 1.12's defaults");
        let z = zstd::encode_all(v.to_string().as_bytes(), 3).unwrap();
        w.conn
            .execute(
                "UPDATE events SET z = ?1 WHERE seq = ?2",
                rusqlite::params![z, seq],
            )
            .unwrap();
    }
    assert!(rewritten >= 6, "{rewritten}");
    // The texts name pull requests, and the session that handed the node off is live; the tables
    // hold neither flag, and the replay gives the state they hold.
    assert!(forum_core::words::names_pull_request(
        "pushed and opened PR #3"
    ));
    let loaded = load_state(&w.conn).unwrap();
    assert!(!loaded.nodes[&n].pr_named);
    assert!(loaded.sessions[&s1].handed_off.is_none());
    let replayed = verify(&w.conn).unwrap();
    assert_eq!(differences(&replayed, &loaded), Vec::<&str>::new());
}

#[test]
fn a_migration_runs_in_one_transaction_without_reading_the_history() {
    let want = fixture_state();
    let conn = fixture();
    deny_history_reads(&conn);
    let o =
        super::load::open_connection(conn, &super::testkit::genesis_args(test_params()), &TO_V2)
            .unwrap();
    assert_eq!(
        o.migrated
            .iter()
            .map(|m| (m.what, m.from, m.to))
            .collect::<Vec<_>>(),
        vec![("state_version", 1, 2), ("shell_version", 1, 2)]
    );
    assert!(o.migrated[0].rows > 0);
    // The migrated tables load to the same state, and pass the checks.
    assert_eq!(differences(&o.state, &want), Vec::<&str>::new());
    check_loaded(&o.state, Checks::Full).unwrap();
    let conn = o.store.into_connection();
    allow_everything(&conn);
    assert_eq!(
        super::schema::get_version(&conn, "state_version").unwrap(),
        2
    );
    assert_eq!(
        super::schema::get_version(&conn, "shell_version").unwrap(),
        2
    );
    // Each migration is a history transaction of kind migrate that names both versions.
    let migrations: Vec<Option<String>> = super::history::txs_after(&conn, 0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|t| t.meta.kind == TxKind::Migrate)
        .map(|t| t.meta.detail)
        .collect();
    assert_eq!(
        migrations,
        vec![
            Some("state_version 1 -> 2".to_owned()),
            Some("shell_version 1 -> 2".to_owned())
        ]
    );
    // The history, replayed through this release's evolve, equals the migrated tables.
    verify_checked(&conn).unwrap();
    // Opening again migrates nothing; an older release refuses the newer tables.
    let path_free = conn;
    let o = super::load::open_connection(
        path_free,
        &super::testkit::genesis_args(test_params()),
        &TO_V2,
    )
    .unwrap();
    assert!(o.migrated.is_empty());
    let conn = o.store.into_connection();
    match super::load::open_connection(
        conn,
        &super::testkit::genesis_args(test_params()),
        &super::migrate::RELEASED,
    ) {
        Err(super::StoreError::Newer {
            what,
            found,
            supported,
        }) => assert_eq!((what, found, supported), ("state_version", 2, 1)),
        other => panic!("{other:?}"),
    }
}

/// The rows of the rowid state tables (`st_scalars`, `st_nodes`, `st_node_hot`) inserted so far
/// by the migration under `a_migration_streams_the_state_rows`; SQLite's update hook does not see
/// the WITHOUT ROWID tables.
static INSERTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// `INSERTED` at each call of `observed`.
static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

/// `reserialise`, recording how many rows the migration had written when it was called.
fn observed(r: super::state::Row) -> Result<Vec<super::state::Row>, String> {
    SEEN.lock()
        .unwrap()
        .push(INSERTED.load(std::sync::atomic::Ordering::SeqCst));
    reserialise(r)
}

#[test]
fn a_migration_streams_the_state_rows() {
    let want = fixture_state();
    let conn = fixture();
    conn.update_hook(Some(
        |action: rusqlite::hooks::Action, _: &str, table: &str, _: i64| {
            if action == rusqlite::hooks::Action::SQLITE_INSERT && table.starts_with("st_") {
                INSERTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        },
    ))
    .unwrap();
    let steps = super::migrate::Steps {
        state: &[(1, observed)],
        shell: &[],
        state_version: 2,
        shell_version: 1,
    };
    let tx = conn.unchecked_transaction().unwrap();
    let done = super::migrate::migrate(&tx, &steps, Time(1)).unwrap();
    tx.commit().unwrap();
    conn.update_hook(None::<fn(rusqlite::hooks::Action, &str, &str, i64)>)
        .unwrap();
    // Each row is written before the next one is read: the step never sees the whole state held
    // back, and the rows of the load order's first tables are in before the others are stepped.
    let seen = SEEN.lock().unwrap().clone();
    assert_eq!(seen.len() as u64, done[0].rows);
    assert!(seen.windows(2).all(|p| p[0] <= p[1]), "{seen:?}");
    let rowid_rows = 1 + 2 * want.nodes.len() as u64;
    assert_eq!(seen.last().copied(), Some(rowid_rows), "{seen:?}");
    // The set-aside tables are gone, and the new ones have their constraints.
    let aside: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'aside_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(aside, 0);
    let fresh = Connection::open_in_memory().unwrap();
    super::schema::create_tables(&fresh).unwrap();
    let schema = |c: &Connection| -> Vec<(String, String, Option<String>)> {
        c.prepare(
            "SELECT type, name, sql FROM sqlite_master WHERE name LIKE 'st_%' ORDER BY type, name",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    };
    assert_eq!(schema(&conn), schema(&fresh));
    assert_eq!(
        differences(&load_state(&conn).unwrap(), &want),
        Vec::<&str>::new()
    );
}

#[test]
fn a_migration_is_stamped_with_the_clamped_now() {
    let last = fixture_state().last_now;
    let stamps = |at: Time| -> Vec<Time> {
        let conn = fixture();
        let tx = conn.unchecked_transaction().unwrap();
        super::migrate::migrate(&tx, &TO_V2, at).unwrap();
        tx.commit().unwrap();
        super::history::txs_after(&conn, 0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|t| t.meta.kind == TxKind::Migrate)
            .map(|t| t.meta.at)
            .collect()
    };
    // A clock behind the stored last_now (T1 expects it): the transactions keep time's order.
    assert_eq!(stamps(Time(1)), vec![last, last]);
    let ahead = last.plus(forum_core::DAY);
    assert_eq!(stamps(ahead), vec![ahead, ahead]);
}

#[test]
fn replay_into_writes_this_releases_versions() {
    // A source of an older version: its history replays through this release's evolve (D.7.6)
    // into this release's tables.
    let src = fixture();
    super::schema::set_meta(&src, "state_version", "0").unwrap();
    super::schema::set_meta(&src, "shell_version", "0").unwrap();
    let dst = Connection::open_in_memory().unwrap();
    let tx = dst.unchecked_transaction().unwrap();
    replay_into_checked(&src, &tx).unwrap();
    tx.commit().unwrap();
    for (k, v) in [
        ("state_version", super::schema::STATE_VERSION),
        ("shell_version", super::schema::SHELL_VERSION),
    ] {
        assert_eq!(super::schema::get_version(&dst, k).unwrap(), v, "{k}");
    }
    for k in ["core_semver", "keys_version", "masking_version"] {
        assert_eq!(
            super::schema::get_meta(&dst, k).unwrap(),
            super::schema::get_meta(&src, k).unwrap(),
            "{k}"
        );
    }
    // The next startup migrates nothing.
    let o = super::load::open_connection(
        dst,
        &super::testkit::genesis_args(test_params()),
        &super::migrate::RELEASED,
    )
    .unwrap();
    assert!(o.migrated.is_empty());
    assert_eq!(differences(&o.state, &fixture_state()), Vec::<&str>::new());
}

#[test]
fn the_replay_checks_the_invariants_at_its_end() {
    // A valid history passes.
    let conn = fixture();
    verify_checked(&conn).unwrap();
    // The random histories keep only what evolve accepts and the four constraints, not every
    // invariant: their replay equals the tables, but the check refuses the state.
    let w = random_history(1, 220, |_, _| {});
    verify(&w.conn).unwrap();
    match verify_checked(&w.conn) {
        Err(super::StoreError::Check(v)) => assert!(!v.is_empty()),
        other => panic!("{other:?}"),
    }
    let dst = Connection::open_in_memory().unwrap();
    let tx = dst.unchecked_transaction().unwrap();
    match replay_into_checked(&w.conn, &tx) {
        Err(super::StoreError::Check(v)) => assert!(!v.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn startup_loads_the_op_records_without_their_responses() {
    let (mut w, s1) = with_session();
    w.run(TxKind::Cmd, |d| {
        let p = d.s.params.clone();
        d.push(Ev::OpRecorded {
            class: class("coder"),
            op: OpId::new("big").unwrap(),
            digest: ReqDigest([5; 32]),
            session: s1,
            expires_at: d.clock.plus(p.op_retain),
            outcome: Ok(Done {
                reply: forum_core::Reply::Status,
                warnings: Vec::new(),
                opened: None,
                stale_session: false,
                delivered: Vec::new(),
            }),
        });
    });
    // Startup's load never reads a response: with the column denied, it still loads.
    w.conn
        .authorizer(Some(|ctx: rusqlite::hooks::AuthContext<'_>| {
            match ctx.action {
                rusqlite::hooks::AuthAction::Read {
                    table_name: "st_ops",
                    column_name: "response" | "masking_version",
                } => rusqlite::hooks::Authorization::Deny,
                _ => rusqlite::hooks::Authorization::Allow,
            }
        }))
        .unwrap();
    let loaded = load_state(&w.conn).unwrap();
    assert_eq!(differences(&loaded, &w.s), Vec::<&str>::new());
    assert!(read_rows(&w.conn, Which::All).is_err());
    allow_everything(&w.conn);
    // A whole read, a migration's, has it.
    let rows = read_rows(&w.conn, Which::All).unwrap();
    assert!(rows.iter().any(|r| matches!(
        &r.extra,
        super::state::Extra::Op { response, .. } if response.as_slice() == b"response big"
    )));
    // A resident read's op row cannot be written back: it has no response.
    let resident = read_rows(&w.conn, Which::Resident).unwrap();
    let op = resident
        .iter()
        .find(|r| r.table() == super::state::Table::Ops)
        .unwrap();
    assert_eq!(op.extra, super::state::Extra::None);
    let tx = w.conn.unchecked_transaction().unwrap();
    assert!(super::state::put_row(&tx, op).is_err());
    tx.rollback().unwrap();
}

#[test]
fn a_failed_migration_changes_nothing() {
    let conn = fixture();
    let before = dump(
        &conn,
        &STATE_TABLES
            .iter()
            .chain(HISTORY_TABLES.iter())
            .chain(SHELL_TABLES.iter())
            .chain(["meta"].iter())
            .copied()
            .collect::<Vec<_>>(),
    );
    let steps = super::migrate::Steps {
        state: &[(1, refuse_sessions)],
        shell: &[(1, add_kind_code)],
        state_version: 2,
        shell_version: 2,
    };
    // An in-memory database cannot be reopened, so the test runs the open's transaction itself.
    let mut store = super::Store::from_connection(conn);
    let tx = store.begin().unwrap();
    let e = super::migrate::migrate(&tx, &steps, Time(1)).unwrap_err();
    assert!(
        matches!(
            &e,
            super::StoreError::Decode {
                table: "migration",
                ..
            }
        ),
        "{e:?}"
    );
    drop(tx);
    let conn = store.into_connection();
    let after = dump(
        &conn,
        &STATE_TABLES
            .iter()
            .chain(HISTORY_TABLES.iter())
            .chain(SHELL_TABLES.iter())
            .chain(["meta"].iter())
            .copied()
            .collect::<Vec<_>>(),
    );
    assert_eq!(after, before);
    // A missing step is refused too.
    let gap = super::migrate::Steps {
        state: &[],
        shell: &[],
        state_version: 2,
        shell_version: 1,
    };
    assert!(super::migrate::migrate(&conn, &gap, Time(1)).is_err());
}

#[test]
fn a_shell_migration_equals_writing_the_shell_tables_under_the_new_version() {
    // Migrated: the fixture's shell tables of version 1, migrated row to row to version 2.
    let migrated = fixture();
    let shell_only = super::migrate::Steps {
        state: &[],
        shell: &[(1, add_kind_code)],
        state_version: 1,
        shell_version: 2,
    };
    deny_history_reads(&migrated);
    let tx = migrated.unchecked_transaction().unwrap();
    super::migrate::migrate(&tx, &shell_only, Time(1)).unwrap();
    tx.commit().unwrap();
    allow_everything(&migrated);
    // Written: the same history written from the start by the writer of version 2.
    let written = Connection::open_in_memory().unwrap();
    super::schema::create_tables(&written).unwrap();
    add_kind_code(&written).unwrap();
    let mut s = State::default();
    for t in super::history::txs_after(&migrated, 0, 100_000).unwrap() {
        let events: Vec<Event> = super::history::events_of_tx(&migrated, t.id)
            .unwrap()
            .into_iter()
            .map(|x| x.event)
            .collect();
        if events.is_empty() {
            continue;
        }
        let mut dirty = forum_core::Tracked::new();
        for e in &events {
            forum_core::evolve_tracked(&mut s, e, &mut dirty);
        }
        super::write(&written, &t.meta, &events, &s, &dirty).unwrap();
        kind_code_v2(&written);
    }
    assert_eq!(
        dump(&written, &SHELL_TABLES),
        dump(&migrated, &SHELL_TABLES)
    );
    let n: i64 = written
        .query_row(
            "SELECT count(*) FROM entries WHERE kind_code IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
}
