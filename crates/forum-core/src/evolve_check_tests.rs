//! The state checks clause by clause, and the review follow-ups of WP-D2 (D.13.2).
//!
//! `full()` builds a settled state with one of each entity the state checks read: settled external
//! and user requests, a pending `said`, an open commitment, open, expired, and escalated people
//! questions, pending and lapsed user questions with a refusal question among them, a shown digest
//! entry, a note, a wait, a due, a series with its waiting occurrence, a refusal, a pause, a signal
//! node, and a closed node. Each case breaks one clause of D.9.2 by hand and asserts both the
//! invariant and the detail it reports, so that removing the clause fails the test.
//!
//! The rest are regression tests of the review: the restores of D.4.12 that bring back a request
//! or a people question without the answer or the user question that left before it (Settled,
//! E1 (d), M1); an occurrence's `since` (§10.4); K2's reading of `repeat:` keys; the boundary of
//! the op-record prune (O1, T2); the owed sets (D.4.10); and the preconditions of `HoldIssued`,
//! `RequestSettled`, and `Fired`, whose failure changes nothing (D.7.5).

use super::*;
use crate::event::ConfirmVia;
use crate::index::cand_at;
use crate::invariants::{Violation, ViolationDetail};
use crate::state::{Held, Occurrence, SignalKey};
use crate::types::{EntryType, Label, Mark, SignalKind};

// ------------------------------------------------------------------------------------------------
// Helpers

fn kstr(s: &str) -> KeyStr {
    KeyStr::new(s).unwrap()
}

fn said(conv: &KeyStr, kind: SaidKind) -> EntryBody {
    EntryBody::Said {
        conv: conv.clone(),
        kind,
        corrects: None,
        voice: false,
    }
}

fn decision(settles: BTreeSet<Eid>) -> EntryBody {
    EntryBody::Decision {
        settles,
        supersedes: None,
        assumes: BTreeSet::new(),
        evidence: None,
        basis: Vec::new(),
    }
}

fn confirm(w: &mut World, a: Eid) {
    w.emit(Ev::SaidConfirmed {
        said: a,
        via: ConfirmVia::Explicit,
        link: None,
    });
}

/// A node under ROOT that carries conversation key `k`.
fn keyed(w: &mut World, n: &str, k: &str) -> NodeId {
    let mut i = init(n);
    i.keys.insert(kstr(k), NormVer(1));
    w.create(ROOT, n, i)
}

/// A people question on `node` in `conv`, asked through a confirmed `said` of kind question.
fn people_question(
    w: &mut World,
    node: NodeId,
    conv: &KeyStr,
    by: SessionId,
    recheck_at: Time,
    blocking: bool,
) -> Eid {
    let sq = w.append(node, Author::Session(by), said(conv, SaidKind::Question));
    confirm(w, sq);
    w.append(
        node,
        Author::Session(by),
        EntryBody::Question(QuestionBody::People {
            conv: conv.clone(),
            blocking,
            said: sq,
            supersedes: None,
            recheck_at,
        }),
    )
}

/// A question for the user about `about`, to `user` alone, without a default.
fn user_question(w: &mut World, source: UqSource, about: NodeId, due: Time) -> Eid {
    w.append(
        USER_DECISIONS,
        Author::Server,
        EntryBody::Question(QuestionBody::User {
            source,
            about: Some(about),
            to: alloc::vec![USER_PRINCIPAL],
            due,
            has_default: false,
        }),
    )
}

fn series_role(run_within: Dur) -> Role {
    Role::Series(crate::state::Series {
        repeat: crate::state::Repeat {
            cron_text: Text("0 7 * * *".into()),
            cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
            tz: forum_cron::TzTable {
                name: alloc::string::String::from("UTC"),
                tzdb_version: alloc::string::String::new(),
                initial: 0,
                transitions: Vec::new(),
            },
            run_within,
        },
        last_missed: false,
    })
}

/// A waiting occurrence of `series` at `slot`, with its `repeat:` key, as the server makes one.
fn occurrence(w: &mut World, series: NodeId, slot: Time, window: Dur) -> NodeId {
    let mut oi = init("run");
    oi.due = Some(slot.plus(window));
    oi.keys
        .insert(KeyStr::of_occurrence(series, slot), NormVer(0));
    oi.role = Role::Occurrence(Occurrence {
        series,
        slot,
        window_end: slot.plus(window),
        phase: OccPhase::Waiting,
        window_fired: false,
    });
    w.create(series, "run", oi)
}

/// Removes the queued key of `(kind, owner)` from the queue and from `deadline_at`.
fn unqueue(s: &mut State, kind: DeadlineKind, owner: &Owner) {
    s.ix.deadlines
        .retain(|k| !(k.kind == kind && k.owner == *owner));
    s.ix.deadline_at.remove(&(kind, owner.clone()));
}

fn violations(s: &State, now: Time) -> Vec<Violation> {
    check_invariants(s, now).err().unwrap_or_default()
}

fn has(v: &[Violation], inv: Inv, detail: &ViolationDetail) -> bool {
    v.iter().any(|x| x.inv == inv && x.detail == *detail)
}

// ------------------------------------------------------------------------------------------------
// A settled state with one of each entity

struct Full {
    w: World,
    s1: SessionId,
    s2: SessionId,
    n: NodeId,
    t: Token,
    r: Eid,
    a: Eid,
    d: Eid,
    ru: Eid,
    tu: Eid,
    pending: Eid,
    c: Eid,
    pq_open: Eid,
    pq_exp: Eid,
    pq_esc: Eid,
    uq_esc: Eid,
    uq_lapsed: Eid,
    uq: Eid,
    uq_refusal: Eid,
    dg: Eid,
    note: Eid,
    other: NodeId,
    quiet: NodeId,
    ask: NodeId,
    waiting: NodeId,
    due: NodeId,
    series: NodeId,
    slot: Time,
    occ: NodeId,
    refused: NodeId,
    paused: NodeId,
    sig_key: SignalKey,
    closed: NodeId,
}

fn full() -> Full {
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let s2 = w.session("w");
    let t = w.take(n, s1);
    let now = w.clock;
    // A settled external request: decided, then answered by a reply in its conversation.
    let r = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/1/1"),
            conv: chat.clone(),
            from_user: None,
            requester: Requester::External,
        },
    );
    let d = w.append(n, Author::Session(s1), decision([r].into()));
    let a = w.append(n, Author::Session(s1), said(&chat, SaidKind::Reply));
    w.emit(Ev::RequestAnswered { request: r, by: a });
    w.emit(Ev::RequestSettled {
        request: r,
        decision: d,
        answer: a,
    });
    confirm(&mut w, a);
    // A settled user request, answered by a result to its requester (CS-4, A-23).
    let ue = w.append(
        USER,
        Author::Session(s1),
        EntryBody::FromUser {
            principal: USER_PRINCIPAL,
        },
    );
    let uk = KeyStr::of_user_entry(ue);
    let ru = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: Key {
                key: uk.clone(),
                nv: NormVer(0),
            },
            conv: uk,
            from_user: Some(ue),
            requester: Requester::User(USER_PRINCIPAL),
        },
    );
    w.emit(Ev::EntryRouted {
        entry: ue,
        routing: Routing::Request {
            node: n,
            request: ru,
        },
        reason: None,
    });
    let du = w.append(n, Author::Session(s1), decision([ru].into()));
    let tu = w.append(
        USER,
        Author::Session(s1),
        EntryBody::ToUser {
            kind: ToUserKind::Result,
            verbatim: true,
            re: [ru].into(),
            principal: USER_PRINCIPAL,
        },
    );
    w.emit(Ev::RequestAnswered {
        request: ru,
        by: tu,
    });
    w.emit(Ev::RequestSettled {
        request: ru,
        decision: du,
        answer: tu,
    });
    // A pending `said` of s2, and an open commitment on a node nobody holds.
    let other = keyed(&mut w, "other", "chat:o/1");
    let pending = w.append(
        other,
        Author::Session(s2),
        said(&kstr("chat:o/1"), SaidKind::Reply),
    );
    let quiet = keyed(&mut w, "quiet", "chat:q/1");
    let c = w.append(
        quiet,
        Author::Session(s2),
        said(&kstr("chat:q/1"), SaidKind::Ack),
    );
    w.emit(Ev::CommitmentOpened {
        c,
        node: quiet,
        conv: kstr("chat:q/1"),
        followup_at: now.plus(w.s.params.commitment_followup),
    });
    confirm(&mut w, c);
    // People questions: open, expired with its assume mark, and escalated to a pending user
    // question.
    let ask = keyed(&mut w, "ask", "chat:p/1");
    let pk = kstr("chat:p/1");
    let later = now.plus(Dur(4 * HOUR.0));
    let pq_open = people_question(&mut w, ask, &pk, s2, later, false);
    let pq_exp = people_question(&mut w, ask, &pk, s2, now, false);
    w.emit(Ev::Fired {
        kind: DeadlineKind::Recheck,
        owner: Owner::Entry(pq_exp),
    });
    w.emit(Ev::PqExpired { q: pq_exp });
    w.emit(Ev::MarkSet {
        node: ask,
        mark: Mark::Assume,
        question: pq_exp,
        on: true,
    });
    let pq_esc = people_question(&mut w, ask, &pk, s2, later, true);
    let uq_esc = user_question(
        &mut w,
        UqSource::Escalation { question: pq_esc },
        ask,
        now.plus(DAY),
    );
    w.emit(Ev::PqEscalated {
        q: pq_esc,
        uq: uq_esc,
    });
    // User questions: lapsed with its assume mark, and pending.
    let agent = |k: u8| UqSource::Agent {
        session: s1,
        digest: ReqDigest([k; 32]),
    };
    let uq_lapsed = user_question(&mut w, agent(1), ask, now);
    w.emit(Ev::Fired {
        kind: DeadlineKind::UserQuestion,
        owner: Owner::Entry(uq_lapsed),
    });
    w.emit(Ev::UqLapsed { q: uq_lapsed });
    w.emit(Ev::MarkSet {
        node: ask,
        mark: Mark::Assume,
        question: uq_lapsed,
        on: true,
    });
    let uq = user_question(&mut w, agent(2), ask, now.plus(HOUR));
    // A digest entry, shown by a `to_user`.
    let dg = w.append(
        USER_DIGEST,
        Author::Server,
        EntryBody::System(SystemNote::Digest {
            item: DigestItem::Lapsed {
                question: uq_lapsed,
            },
            to: USER_PRINCIPAL,
        }),
    );
    let shown_by = w.append(
        USER,
        Author::Session(s1),
        EntryBody::ToUser {
            kind: ToUserKind::Ack,
            verbatim: false,
            re: [dg].into(),
            principal: USER_PRINCIPAL,
        },
    );
    w.emit(Ev::DigestShown {
        entry: dg,
        by: shown_by,
    });
    // A note, a wait, and a due.
    let note = w.append(
        other,
        Author::Session(s1),
        EntryBody::Note {
            to: Some(Addressee::Session(s2)),
            until: Some(now.plus(HOUR)),
            recipients: [s2].into(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        },
    );
    let waiting = w.node(ROOT, "waiting");
    w.emit(Ev::WaitSet {
        node: waiting,
        wait: Some(WaitOn::Time(now.plus(Dur(2 * HOUR.0)))),
    });
    let mut di = init("due");
    di.due = Some(now.plus(Dur(3 * HOUR.0)));
    let due = w.create(ROOT, "due", di);
    // A series and its waiting occurrence.
    let mut si = init("series");
    si.role = series_role(HOUR);
    let series = w.create(ROOT, "series", si);
    let slot = Time((now.0 / HOUR.0 + 5) * HOUR.0);
    let occ = occurrence(&mut w, series, slot, HOUR);
    // A class active by an open `next`, which refused a node with quorum 1, and its refusal
    // question (E1 (a)).
    let refused = w.node(ROOT, "refused");
    w.emit(Ev::NextCalled {
        session: s1,
        outcome: NextOutcome::Nothing,
        open: true,
        class_window_end: now.plus(w.s.params.refusal_window),
    });
    w.emit(Ev::Refused {
        node: refused,
        class: class("w"),
        session: s1,
        backoff_until: now.plus(HOUR),
        expires_at: now.plus(DAY),
        quorum: 1,
    });
    let uq_refusal = user_question(
        &mut w,
        UqSource::Refusal { node: refused },
        refused,
        now.plus(DAY),
    );
    // A pause, a signal node, and a closed node.
    let paused = w.node(ROOT, "paused");
    w.emit(Ev::Paused {
        scope: paused,
        until: now.plus(HOUR),
        ttl: w.s.params.pause_ttl,
        by: s1,
        class: class("w"),
        renewed: false,
    });
    let sig_key = SignalKey {
        kind: SignalKind::Duplicate,
        a: other.min(quiet),
        b: other.max(quiet),
    };
    let mut gi = init("dup");
    gi.role = Role::Signal(sig_key);
    gi.tags.insert(Label::new(OFFER).unwrap());
    let signal = w.create(SIGNALS, "dup", gi);
    w.emit(Ev::SignalOpened {
        key: sig_key,
        node: signal,
    });
    let closed = w.node(ROOT, "closed");
    w.emit(Ev::NodeClosed {
        node: closed,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    ok(&w);
    assert!(check_startup(&w.s, w.clock).is_ok());
    Full {
        w,
        s1,
        s2,
        n,
        t,
        r,
        a,
        d,
        ru,
        tu,
        pending,
        c,
        pq_open,
        pq_exp,
        pq_esc,
        uq_esc,
        uq_lapsed,
        uq,
        uq_refusal,
        dg,
        note,
        other,
        quiet,
        ask,
        waiting,
        due,
        series,
        slot,
        occ,
        refused,
        paused,
        sig_key,
        closed,
    }
}

// ------------------------------------------------------------------------------------------------
// Each clause of the state checks rejects its own violation (D.9.2, D.13.2 WP-D2)

/// One hand-built violation: the state, the invariant, and the detail the check must report.
type Case = (&'static str, State, Inv, ViolationDetail);

#[allow(
    clippy::similar_names,
    reason = "the fixture's names follow the entities"
)]
fn cases(f: &Full) -> Vec<Case> {
    use ViolationDetail as V;
    let s0 = &f.w.s;
    let now = f.w.clock;
    let w = class("w");
    let mut out: Vec<Case> = Vec::new();
    let mut add = |name: &'static str, edit: &dyn Fn(&mut State), inv: Inv, detail: V| {
        let mut s = s0.clone();
        edit(&mut s);
        out.push((name, s, inv, detail));
    };
    // H2: the taker retires after the hold's ceiling (CS-8).
    let hold = s0.holds[&f.t].clone();
    add(
        "H2 retire after the ceiling",
        &|s| {
            s.sessions.get_mut(&f.s1).unwrap().retire_at = hold.taken_at.plus(hold.terms.ceiling);
        },
        Inv::H2,
        V::Hold(f.t),
    );
    // H5: an unchecked hold's lease ends within lease.first of its take.
    add(
        "H5 first lease",
        &|s| {
            let h = s.holds.get_mut(&f.t).unwrap();
            h.lease_until = h.taken_at.plus(h.terms.first).plus(MINUTE);
        },
        Inv::H5,
        V::Hold(f.t),
    );
    // O2: every kept entry's seq is at most its node's.
    add(
        "O2 entry seq",
        &|s| {
            let seq = s.nodes[&f.n].seq;
            s.entries.get_mut(&f.r).unwrap().seq = Seq(seq.0 + 1);
        },
        Inv::O2,
        V::Entry(f.r),
    );
    // R1: the key set of `ix.next.of` equals that of `since`, and `ix.next.held` its recount.
    add(
        "R1 since keys",
        &|s| {
            s.since.insert(Cand::Node(f.closed), now);
        },
        Inv::R1,
        V::Index("since keys"),
    );
    add(
        "R1 next.held",
        &|s| {
            s.ix.next.held.insert(
                Cand::Node(f.closed),
                Held {
                    depends: true,
                    paused_by: None,
                    before_slot: false,
                },
            );
        },
        Inv::R1,
        V::Index("next.held"),
    );
    // K2: no key of a version above 0 has the repeat scheme (A-90).
    let foreign = KeyStr::of_occurrence(f.series, f.slot.plus(DAY));
    add(
        "K2 repeat scheme at a normaliser version",
        &|s| {
            s.nodes
                .get_mut(&f.other)
                .unwrap()
                .keys
                .insert(foreign.clone(), NormVer(1));
        },
        Inv::K2,
        V::Key(foreign.clone()),
    );
    // K2: a repeat: key of version 0 names a node that exists.
    let nowhere = KeyStr::of_occurrence(NodeId(999), f.slot);
    add(
        "K2 repeat key naming no node",
        &|s| {
            s.nodes
                .get_mut(&f.other)
                .unwrap()
                .keys
                .insert(nowhere.clone(), NormVer(0));
        },
        Inv::K2,
        V::Key(nowhere.clone()),
    );
    // K3: every alias target exists.
    add(
        "K3 alias target",
        &|s| {
            s.aliases.insert((ROOT, name("ghost")), NodeId(999));
        },
        Inv::K3,
        V::Node(NodeId(999)),
    );
    // K5: every merged_into chain ends.
    add(
        "K5 chain",
        &|s| {
            for (x, y) in [(f.closed, f.quiet), (f.quiet, f.closed)] {
                s.nodes.get_mut(&x).unwrap().life = Life::Closed(crate::state::Closure {
                    at: now,
                    outcome: Outcome::Superseded,
                    merged_into: Some(y),
                    upheld: None,
                    corrected_from: None,
                    by: None,
                });
            }
        },
        Inv::K5,
        V::Node(f.closed),
    );
    // S1: at most one open signal node per key.
    add(
        "S1 two open signals",
        &|s| {
            s.nodes.get_mut(&f.quiet).unwrap().role = Role::Signal(f.sig_key);
        },
        Inv::S1,
        V::Text("two open signals"),
    );
    // Settled, the Some half: listed, answered, and of the right kind and conversation.
    add(
        "Settled listed",
        &|s| {
            s.decisions.get_mut(&f.d).unwrap().settles.clear();
        },
        Inv::Settled,
        V::Entry(f.r),
    );
    add(
        "Settled answer",
        &|s| {
            s.requests.get_mut(&f.r).unwrap().answered_by = Some(f.pending);
        },
        Inv::Settled,
        V::Entry(f.r),
    );
    add(
        "Settled kind",
        &|s| {
            s.saids.get_mut(&f.a).unwrap().kind = SaidKind::Ack;
        },
        Inv::Settled,
        V::Entry(f.r),
    );
    add(
        "Settled conversation (§9.4)",
        &|s| {
            s.saids.get_mut(&f.a).unwrap().conv = kstr("mail:m@x");
        },
        Inv::Settled,
        V::Entry(f.r),
    );
    add(
        "Settled user answer is a to_user",
        &|s| {
            s.entries.get_mut(&f.tu).unwrap().ty = EntryType::Said;
        },
        Inv::Settled,
        V::Entry(f.ru),
    );
    add(
        "Settled user answer names a principal",
        &|s| {
            s.entries.get_mut(&f.tu).unwrap().principal = None;
        },
        Inv::Settled,
        V::Entry(f.ru),
    );
    // Settled, the None half: answered and listed by a decision that is not retracted.
    add(
        "Settled none half",
        &|s| {
            s.requests.get_mut(&f.r).unwrap().settled = None;
        },
        Inv::Settled,
        V::Entry(f.r),
    );
    // F2: no flag changes before the next queued key (a due that nothing queues).
    add(
        "F2 a flag that changes before the next key",
        &|s| {
            s.nodes.get_mut(&f.due).unwrap().due = Some(now.plus(MINUTE));
            unqueue(s, DeadlineKind::Due, &Owner::Node(f.due));
        },
        Inv::F,
        V::Node(f.due),
    );
    // E1 (a): a node every active class refused has its pending refusal question.
    add(
        "E1 (a) covered",
        &|s| {
            s.uquestions.get_mut(&f.uq_refusal).unwrap().source = UqSource::Agent {
                session: f.s1,
                digest: ReqDigest([9; 32]),
            };
        },
        Inv::E1,
        V::Node(f.refused),
    );
    // E1 (b): every refusal record has an entry, and every entry ends after its back-off and
    // after now.
    add(
        "E1 (b) an empty refusal record",
        &|s| {
            s.refusals
                .get_mut(&(f.refused, w.clone()))
                .unwrap()
                .by
                .clear();
        },
        Inv::E1,
        V::Node(f.refused),
    );
    add(
        "E1 (b) back-off after the expiry",
        &|s| {
            let x = s
                .refusals
                .get_mut(&(f.refused, w.clone()))
                .unwrap()
                .by
                .get_mut(&f.s1)
                .unwrap();
            x.backoff_until = x.expires_at.plus(Dur(1));
        },
        Inv::E1,
        V::Node(f.refused),
    );
    // E1 (c): an expired people question without an assumption has its assume mark.
    add(
        "E1 (c) the expired question's mark",
        &|s| {
            s.nodes
                .get_mut(&f.ask)
                .unwrap()
                .assume_marks
                .remove(&f.pq_exp);
        },
        Inv::E1,
        V::Entry(f.pq_exp),
    );
    // E1: closed nodes carry no assume mark (A-104).
    add(
        "E1 a closed node's marks",
        &|s| {
            s.nodes
                .get_mut(&f.closed)
                .unwrap()
                .assume_marks
                .insert(f.pq_exp);
        },
        Inv::E1,
        V::Node(f.closed),
    );
    // E1 (d): an escalated question has its user question.
    add(
        "E1 (d) the escalation's user question",
        &|s| {
            s.uquestions.get_mut(&f.uq_esc).unwrap().source = UqSource::Agent {
                session: f.s1,
                digest: ReqDigest([9; 32]),
            };
        },
        Inv::E1,
        V::Entry(f.pq_esc),
    );
    // E1 (e): a pending user question has its deadline queued.
    add(
        "E1 (e) queued",
        &|s| unqueue(s, DeadlineKind::UserQuestion, &Owner::Entry(f.uq)),
        Inv::E1,
        V::Entry(f.uq),
    );
    // E1 (f): a lapsed user question without an assumption has its assume mark.
    add(
        "E1 (f) the lapsed question's mark",
        &|s| {
            s.nodes
                .get_mut(&f.ask)
                .unwrap()
                .assume_marks
                .remove(&f.uq_lapsed);
        },
        Inv::E1,
        V::Entry(f.uq_lapsed),
    );
    // W1: a waiting node is a status candidate exactly while it carries open_commitment.
    add(
        "W1 status candidate",
        &|s| {
            let at = cand_at(s, Cand::Status(f.waiting), now).unwrap();
            s.ix.next.of.insert(Cand::Status(f.waiting), at);
        },
        Inv::W1,
        V::Node(f.waiting),
    );
    // D1: a shown entry names a `to_user` when that entry is resident.
    add(
        "D1 shown by a to_user",
        &|s| {
            s.digest.get_mut(&f.dg).unwrap().state = DigestState::Shown { by: f.a };
        },
        Inv::D1,
        V::Entry(f.dg),
    );
    // M1: every waiting object has its mover.
    add(
        "M1 wait",
        &|s| unqueue(s, DeadlineKind::Wait, &Owner::Node(f.waiting)),
        Inv::M1,
        V::Node(f.waiting),
    );
    add(
        "M1 open people question",
        &|s| unqueue(s, DeadlineKind::Recheck, &Owner::Entry(f.pq_open)),
        Inv::M1,
        V::Entry(f.pq_open),
    );
    add(
        "M1 escalated people question",
        &|s| {
            s.uquestions.get_mut(&f.uq_esc).unwrap().state = UqState::Lapsed;
        },
        Inv::M1,
        V::Entry(f.pq_esc),
    );
    add(
        "M1 commitment",
        &|s| unqueue(s, DeadlineKind::Commitment, &Owner::Entry(f.c)),
        Inv::M1,
        V::Entry(f.c),
    );
    add(
        "M1 pending said",
        &|s| unqueue(s, DeadlineKind::Silence, &Owner::Session(f.s2)),
        Inv::M1,
        V::Entry(f.pending),
    );
    add(
        "M1 refusal",
        &|s| {
            unqueue(
                s,
                DeadlineKind::RefusalExpiry,
                &Owner::Refusal(f.refused, w.clone()),
            );
        },
        Inv::M1,
        V::Node(f.refused),
    );
    add(
        "M1 pause",
        &|s| unqueue(s, DeadlineKind::Pause, &Owner::Node(f.paused)),
        Inv::M1,
        V::Node(f.paused),
    );
    // U1: the chain has no duplicates, and a pending question is listed for its addressee only.
    add(
        "U1 duplicates",
        &|s| {
            s.uquestions.get_mut(&f.uq).unwrap().to = alloc::vec![USER_PRINCIPAL, USER_PRINCIPAL];
        },
        Inv::U1,
        V::Entry(f.uq),
    );
    add(
        "U1 listed for its addressee",
        &|s| {
            for ranks in s.ix.pending_uq_by.values_mut() {
                ranks.retain(|x| x.q != f.uq);
            }
        },
        Inv::U1,
        V::Entry(f.uq),
    );
    // T3: every marker equals its predicate, and every time lies after now.
    add(
        "T3 silent before silence_at",
        &|s| s.sessions.get_mut(&f.s2).unwrap().silent = true,
        Inv::T3,
        V::Session(f.s2),
    );
    add(
        "T3 live after silence_at",
        &|s| s.sessions.get_mut(&f.s2).unwrap().silence_at = now,
        Inv::T3,
        V::Session(f.s2),
    );
    add(
        "T3 retire_at after now",
        &|s| s.sessions.get_mut(&f.s2).unwrap().retire_at = now,
        Inv::T3,
        V::Session(f.s2),
    );
    add(
        "T3 wait marker",
        &|s| {
            s.nodes
                .get_mut(&f.waiting)
                .unwrap()
                .wait
                .as_mut()
                .unwrap()
                .fired = true;
        },
        Inv::T3,
        V::Node(f.waiting),
    );
    add(
        "T3 due marker",
        &|s| s.nodes.get_mut(&f.due).unwrap().due_fired = true,
        Inv::T3,
        V::Node(f.due),
    );
    add(
        "T3 occurrence phase",
        &|s| {
            if let Role::Occurrence(x) = &mut s.nodes.get_mut(&f.occ).unwrap().role {
                x.phase = OccPhase::Available;
            }
        },
        Inv::T3,
        V::Node(f.occ),
    );
    add(
        "T3 occurrence window marker",
        &|s| {
            if let Role::Occurrence(x) = &mut s.nodes.get_mut(&f.occ).unwrap().role {
                x.window_fired = true;
            }
        },
        Inv::T3,
        V::Node(f.occ),
    );
    add(
        "T3 follow-up marker",
        &|s| s.commitments.get_mut(&f.c).unwrap().followup_fired = true,
        Inv::T3,
        V::Entry(f.c),
    );
    add(
        "T3 class profile",
        &|s| {
            for t in s.classes.get_mut(&w).unwrap().caps_seen.values_mut() {
                *t = now;
            }
        },
        Inv::T3,
        V::Text("class profile ended"),
    );
    add(
        "T3 note",
        &|s| s.notes.get_mut(&f.note).unwrap().until = s0.last_now,
        Inv::T3,
        V::Entry(f.note),
    );
    add(
        "T3 leaving time",
        &|s| {
            let x = EntityRef::Request(f.r);
            assert!(s.leaving.contains_key(&x));
            s.leaving.insert(x, s0.last_now);
        },
        Inv::T3,
        V::Text("leaving time"),
    );
    // T4: `deadline_at` agrees with the queue.
    add(
        "T4 deadline_at",
        &|s| {
            s.ix.deadline_at
                .insert((DeadlineKind::Due, Owner::Node(f.closed)), now.plus(HOUR));
        },
        Inv::T4,
        V::Index("deadline_at"),
    );
    // T3: settled at now (no key at or before it), every hold live, every pause and refusal entry
    // ending after now.
    let overdue = DeadlineKey {
        at: now,
        kind: DeadlineKind::Due,
        owner: Owner::Node(f.closed),
    };
    add(
        "T3 a key at now",
        &|s| {
            s.ix.deadlines.insert(overdue.clone());
        },
        Inv::T3,
        V::Deadline(overdue.clone()),
    );
    add(
        "T3 a hold past its lease",
        &|s| s.holds.get_mut(&f.t).unwrap().lease_until = now,
        Inv::T3,
        V::Hold(f.t),
    );
    add(
        "T3 a pause past its end",
        &|s| s.pauses.get_mut(&f.paused).unwrap().until = now,
        Inv::T3,
        V::Node(f.paused),
    );
    add(
        "T3 a refusal entry past its expiry",
        &|s| {
            s.refusals
                .get_mut(&(f.refused, w.clone()))
                .unwrap()
                .by
                .get_mut(&f.s1)
                .unwrap()
                .expires_at = now;
        },
        Inv::T3,
        V::Node(f.refused),
    );
    // R1: each candidate's `since` is the stored one, and none lies after now.
    add(
        "R1 since values",
        &|s| {
            let c = Cand::Node(f.ask);
            assert!(s.ix.next.of.contains_key(&c));
            s.since.insert(c, Time(now.0 - 1));
        },
        Inv::R1,
        V::Index("since values"),
    );
    add(
        "R1 since after now",
        &|s| {
            let c = Cand::Node(f.ask);
            s.since.insert(c, now.plus(MINUTE));
            s.ix.next.of.get_mut(&c).unwrap().since = now.plus(MINUTE);
        },
        Inv::R1,
        V::Index("since after now"),
    );
    // K3: every live name maps to a node at that place, and every node's place is a live name.
    add(
        "K3 a name naming a node elsewhere",
        &|s| {
            s.names.insert((ROOT, name("elsewhere")), f.other);
        },
        Inv::K3,
        V::Node(f.other),
    );
    add(
        "K3 a place that is no live name",
        &|s| {
            s.names.remove(&(ROOT, name("other")));
        },
        Inv::K3,
        V::Node(f.other),
    );
    // K8: each reserved node at its place, with role Plain and origin Reserved.
    add(
        "K8 a reserved node moved",
        &|s| {
            s.nodes.get_mut(&SYS).unwrap().place = Some(crate::state::Place {
                parent: USER,
                name: name("sys"),
            });
        },
        Inv::K8,
        V::Node(SYS),
    );
    add(
        "K8 a reserved node with repeat",
        &|s| s.nodes.get_mut(&USER).unwrap().role = series_role(HOUR),
        Inv::K8,
        V::Node(USER),
    );
    add(
        "K8 a reserved node made by an agent",
        &|s| s.nodes.get_mut(&PEOPLE).unwrap().origin = Origin::Agent(f.s1),
        Inv::K8,
        V::Node(PEOPLE),
    );
    // D1: a pending entry is listed for its principal.
    add(
        "D1 a pending entry listed",
        &|s| s.digest.get_mut(&f.dg).unwrap().state = DigestState::Pending,
        Inv::D1,
        V::Entry(f.dg),
    );
    // M1: an open occurrence has its mover, and so do an expired people question and a lapsed
    // user question without their assumptions.
    add(
        "M1 open occurrence",
        &|s| unqueue(s, DeadlineKind::OccurrenceSlot, &Owner::Node(f.occ)),
        Inv::M1,
        V::Node(f.occ),
    );
    add(
        "M1 expired people question",
        &|s| {
            s.nodes
                .get_mut(&f.ask)
                .unwrap()
                .assume_marks
                .remove(&f.pq_exp);
        },
        Inv::M1,
        V::Entry(f.pq_exp),
    );
    add(
        "M1 lapsed user question",
        &|s| {
            s.nodes
                .get_mut(&f.ask)
                .unwrap()
                .assume_marks
                .remove(&f.uq_lapsed);
        },
        Inv::M1,
        V::Entry(f.uq_lapsed),
    );
    add(
        "M1 pending user question",
        &|s| unqueue(s, DeadlineKind::UserQuestion, &Owner::Entry(f.uq)),
        Inv::M1,
        V::Entry(f.uq),
    );
    // H2: every hold's taker exists.
    add(
        "H2 a hold without its taker",
        &|s| {
            s.sessions.remove(&f.s1);
        },
        Inv::H2,
        V::Hold(f.t),
    );
    // D1: every entry's principal is interned.
    add(
        "D1 an entry for a principal never seen",
        &|s| s.digest.get_mut(&f.dg).unwrap().to = PrincipalId(77),
        Inv::D1,
        V::Entry(f.dg),
    );
    // U1: the chain ends with `user`.
    add(
        "U1 a chain that does not end with user",
        &|s| {
            s.uquestions.get_mut(&f.uq).unwrap().to = alloc::vec![USER_PRINCIPAL, PrincipalId(5)];
        },
        Inv::U1,
        V::Entry(f.uq),
    );
    // T4: the queue equals its recount (with `deadline_at` agreeing with the queue).
    let stray = DeadlineKey {
        at: now.plus(HOUR),
        kind: DeadlineKind::Due,
        owner: Owner::Node(f.closed),
    };
    add(
        "T4 a key no deadline_of gives",
        &|s| {
            s.ix.deadlines.insert(stray.clone());
            s.ix.deadline_at
                .insert((stray.kind, stray.owner.clone()), stray.at);
        },
        Inv::T4,
        V::Index("deadlines"),
    );
    out
}

#[test]
fn each_index_r1_compares_is_reported_by_its_name() {
    // R1 compares every index with its recount, one name per index (D.9.2): emptying an index the
    // fixture fills, or adding an entry no recount has to one it leaves empty, is reported under
    // that index's name.
    let f = full();
    let now = f.w.clock;
    let rc = recount(&f.w.s, now);
    let mut empty: Vec<&'static str> = Vec::new();
    let mut missed: Vec<&'static str> = Vec::new();
    let mut tried: BTreeSet<&'static str> = BTreeSet::new();
    let mut check = |name: &'static str, s: &State, missed: &mut Vec<&'static str>| {
        tried.insert(name);
        if !has(&violations(s, now), Inv::R1, &ViolationDetail::Index(name)) {
            missed.push(name);
        }
    };
    macro_rules! emptied {
        ($($name:literal => $($path:ident).+),* $(,)?) => {
            $(
                if rc.$($path).+ == Default::default() {
                    empty.push($name);
                } else {
                    let mut s = f.w.s.clone();
                    s.ix.$($path).+ = Default::default();
                    check($name, &s, &mut missed);
                }
            )*
        };
    }
    emptied!(
        "node" => node,
        "children" => children,
        "flagged_children" => flagged_children,
        "attended_children" => attended_children,
        "unheld_flagged_children" => unheld_flagged_children,
        "unheld_flagged_by_part" => unheld_flagged_by_part,
        "keys_open" => keys_open,
        "keys_all" => keys_all,
        "origins" => origins,
        "unsettled_requests" => unsettled_requests,
        "requests_by" => requests_by,
        "unsettled_by" => unsettled_by,
        "people" => people,
        "items" => items,
        "settling" => settling,
        "clarifications" => clarifications,
        "flagged" => flagged,
        "flagged_active" => flagged_active,
        "next.of" => next.of,
        "next.parts" => next.parts,
        "next.held" => next.held,
        "next.nodes" => next.nodes,
        "next.needs_trie" => next.needs_trie,
        "next.waiting" => next.waiting,
        "series_open" => series_open,
        "agreements" => agreements,
        "unrouted" => unrouted,
        "pending_uq_by" => pending_uq_by,
        "refusal_question" => refusal_question,
        "pending_digest_by" => pending_digest_by,
        "waiters" => waiters,
        "dependents" => dependents,
        "backlinks" => backlinks,
        "cited_by" => cited_by,
        "marked_by_question" => marked_by_question,
        "holds_by_session" => holds_by_session,
        "open_saids_by" => open_saids_by,
        "unrouted_by" => unrouted_by,
        "refused" => refused,
        "live_sessions" => live_sessions,
        "live_by_class" => live_by_class,
        "ops_expiry" => ops_expiry,
        "notes_until" => notes_until,
        "leaving_by_time" => leaving_by_time,
        "inbox" => inbox,
        "pending_holder" => pending_holder,
        "branch_notes" => branch_notes,
        "user_notes" => user_notes,
        "principal_key" => principal_key,
        "person_of" => person_of,
        "aliases_of" => aliases_of,
        "profiles_by_time" => profiles_by_time,
        "class_ends" => class_ends,
    );
    // The indices the fixture leaves empty: an entry no recount has.
    let junk = Eid(9_999);
    let mut s0 = f.w.s.clone();
    let x = &mut s0.ix;
    x.unsettled_requests.insert(junk);
    x.unsettled_by.insert(USER_PRINCIPAL, [junk].into());
    x.next.held.insert(
        Cand::Node(f.closed),
        Held {
            depends: true,
            paused_by: None,
            before_slot: false,
        },
    );
    x.waiters.insert(WaitKey::Node(f.n), [f.other].into());
    x.next.waiting.insert(f.closed);
    x.agreements.insert(f.other);
    x.clarifications.insert(f.r, [junk].into());
    x.unrouted.insert(junk);
    x.pending_digest_by.insert(USER_PRINCIPAL, [junk].into());
    x.dependents.insert(f.n, [f.other].into());
    x.backlinks
        .insert(f.n, [(crate::types::LinkRel::DerivedFrom, f.other)].into());
    x.cited_by.insert(BasisTarget::Node(f.n), [f.d].into());
    x.unrouted_by.insert(f.s1, [junk].into());
    x.ops_expiry
        .insert((now, class("w"), OpId::new("op").unwrap()));
    x.pending_holder.insert(f.n, [junk].into());
    x.branch_notes.insert(ROOT, [junk].into());
    x.user_notes.insert(USER_PRINCIPAL, [(now, junk)].into());
    x.person_of.insert(USER_PRINCIPAL, f.other);
    x.aliases_of.insert(f.other, [USER_PRINCIPAL].into());
    for name in empty.clone() {
        check(name, &s0, &mut missed);
    }
    assert!(
        missed.is_empty(),
        "not reported: {missed:?} (empty: {empty:?})"
    );
    assert_eq!(tried.len(), 53, "{tried:?}");
}

#[test]
fn each_clause_of_the_state_checks_rejects_its_own_violation() {
    let f = full();
    let now = f.w.clock;
    let mut missed = Vec::new();
    for (name, s, inv, detail) in cases(&f) {
        let v = violations(&s, now);
        if !has(&v, inv, &detail) {
            missed.push(alloc::format!("{name}: {inv:?} {detail:?} not in {v:?}"));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
    // T3: the state is settled at `now`, which is never before `last_now`.
    assert!(has(
        &violations(&f.w.s, Time(f.w.s.last_now.0 - 1)),
        Inv::T3,
        &ViolationDetail::Text("now before last_now")
    ));
}

#[test]
fn a_holder_past_silence_at_is_not_silent_by_t3() {
    // T3 (1.4, R27): `silent ⇔ silence_at <= now ∧` the session holds nothing.
    let f = full();
    let now = f.w.clock;
    let mut s = f.w.s.clone();
    s.sessions.get_mut(&f.s1).unwrap().silence_at = now;
    assert!(!has(
        &violations(&s, now),
        Inv::T3,
        &ViolationDetail::Session(f.s1)
    ));
    s.sessions.get_mut(&f.s1).unwrap().silent = true;
    let v = violations(&s, now);
    assert!(has(&v, Inv::T3, &ViolationDetail::Session(f.s1)), "{v:?}");
    assert!(has(&v, Inv::H2, &ViolationDetail::Hold(f.t)), "{v:?}");
}

#[test]
fn check_startup_reports_another_node_with_origin_reserved() {
    // K8 (A-118): no node but the eight reserved ones has origin Reserved.
    let f = full();
    let mut s = f.w.s.clone();
    s.nodes.get_mut(&f.other).unwrap().origin = Origin::Reserved;
    let v = check_startup(&s, f.w.clock).expect_err("K8");
    assert!(has(&v, Inv::K8, &ViolationDetail::Node(f.other)), "{v:?}");
}

// ------------------------------------------------------------------------------------------------
// Restores of D.4.12 that bring back one entity without what left before it

#[test]
fn a_settled_request_restored_without_its_answer_keeps_the_state_checks() {
    // D.4.12: a command naming a settled request whose answer has left too (`talk decide
    // {settles: [r]}` or `to_user {re: [r]}`) restores the request alone. Settled ranges over the
    // resident state and does not evaluate the answer that left (review finding 1).
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let r = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/1/7"),
            conv: chat.clone(),
            from_user: None,
            requester: Requester::External,
        },
    );
    let a = w.append(n, Author::Session(s1), said(&chat, SaidKind::Reply));
    w.emit(Ev::RequestAnswered { request: r, by: a });
    let d = w.append(n, Author::Session(s1), decision([r].into()));
    w.emit(Ev::RequestSettled {
        request: r,
        decision: d,
        answer: a,
    });
    confirm(&mut w, a);
    // A user request answered by a result, for the same case with a `to_user` answer.
    let ue = w.append(
        USER,
        Author::Session(s1),
        EntryBody::FromUser {
            principal: USER_PRINCIPAL,
        },
    );
    let uk = KeyStr::of_user_entry(ue);
    let ru = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: Key {
                key: uk.clone(),
                nv: NormVer(0),
            },
            conv: uk,
            from_user: Some(ue),
            requester: Requester::User(USER_PRINCIPAL),
        },
    );
    w.emit(Ev::EntryRouted {
        entry: ue,
        routing: Routing::Request {
            node: n,
            request: ru,
        },
        reason: None,
    });
    let du = w.append(n, Author::Session(s1), decision([ru].into()));
    let tu = w.append(
        USER,
        Author::Session(s1),
        EntryBody::ToUser {
            kind: ToUserKind::Result,
            verbatim: true,
            re: [ru].into(),
            principal: USER_PRINCIPAL,
        },
    );
    w.emit(Ev::RequestAnswered {
        request: ru,
        by: tu,
    });
    w.emit(Ev::RequestSettled {
        request: ru,
        decision: du,
        answer: tu,
    });
    w.touch(s1);
    ok(&w);
    let rows = [
        (r, w.s.requests[&r].clone(), w.s.entries[&r]),
        (ru, w.s.requests[&ru].clone(), w.s.entries[&ru]),
    ];
    // The requests leave FINISHED_RETAIN after they settled, and their answers FINISHED_RETAIN
    // after that.
    for _ in 0..2 {
        w.at(w.clock.plus(FINISHED_RETAIN));
        w.touch(s1);
    }
    for (x, ..) in &rows {
        assert!(!w.s.requests.contains_key(x));
    }
    assert!(!w.s.saids.contains_key(&a) && !w.s.entries.contains_key(&a));
    assert!(!w.s.entries.contains_key(&tu));
    assert!(check_invariants(&w.s, w.clock).is_ok());
    for (x, row, meta) in rows {
        let before = w.s.clone();
        let mut dirty = crate::facts::Tracked::new();
        let evs = restore_tracked(
            &mut w.s,
            w.clock,
            alloc::vec![Restored::Request(x, row, meta)],
            &mut dirty,
        );
        assert_eq!(evs.len(), 1);
        w.log.extend(evs);
        w.consistent(&before, &dirty).unwrap();
        assert!(w.s.requests.contains_key(&x));
        let v = violations(&w.s, w.clock);
        assert!(v.is_empty(), "after restoring {x:?} alone: {v:?}");
    }
    // A resident answer is still checked: an answer in another conversation is refused.
    let mut s = w.s.clone();
    s.saids.insert(
        a,
        Said {
            node: n,
            session: s1,
            conv: kstr("mail:m@x"),
            kind: SaidKind::Reply,
            state: SaidState::Confirmed,
            moved_from: None,
            with_reply: false,
        },
    );
    s.entries.insert(
        a,
        EntryMeta {
            node: n,
            seq: Seq(1),
            at: T0,
            by: Author::Session(s1),
            ty: EntryType::Said,
            principal: None,
        },
    );
    assert!(has(
        &violations(&s, w.clock),
        Inv::Settled,
        &ViolationDetail::Entry(r)
    ));
    // The restored requests leave again, and the history replays.
    w.at(w.clock.plus(FINISHED_RETAIN));
    w.emit(Ev::SessionStatus {
        session: s1,
        text: Text("x".into()),
    });
    assert!(w.s.requests.is_empty());
    assert_eq!(w.replay(), w.s);
}

#[test]
fn an_escalated_question_restored_without_its_user_question_keeps_the_state_checks() {
    // D.4.12: `heard {answers: q}` or `hold release {waits_for: {question: q}}` naming an escalated
    // question whose user question has left too restores q alone. E1 (d) and M1 range over the
    // resident state: a user question that left memory was finished (review finding 2).
    let mut w = world();
    let s1 = w.session("w");
    let ask = keyed(&mut w, "ask", "chat:p/1");
    let pk = kstr("chat:p/1");
    let now = w.clock;
    let q = people_question(&mut w, ask, &pk, s1, now.plus(HOUR), true);
    let uq = user_question(
        &mut w,
        UqSource::Escalation { question: q },
        ask,
        now.plus(DAY),
    );
    w.emit(Ev::PqEscalated { q, uq });
    // Nobody answers: it lapses, and the server records the assumption (D.8.17).
    w.at(w.clock.plus(DAY));
    w.emit(Ev::Fired {
        kind: DeadlineKind::UserQuestion,
        owner: Owner::Entry(uq),
    });
    w.emit(Ev::UqLapsed { q: uq });
    let d = w.append(
        USER_DECISIONS,
        Author::Server,
        EntryBody::Decision {
            settles: BTreeSet::new(),
            supersedes: None,
            assumes: [uq].into(),
            evidence: None,
            basis: Vec::new(),
        },
    );
    w.emit(Ev::UqAssumed { q: uq, decision: d });
    let other = w.node(ROOT, "other");
    let row = w.s.pquestions[&q].clone();
    let meta = w.s.entries[&q];
    // q leaves FINISHED_RETAIN later, and then its user question.
    for _ in 0..2 {
        w.at(w.clock.plus(FINISHED_RETAIN));
        w.touch(s1);
    }
    assert!(!w.s.pquestions.contains_key(&q));
    assert!(!w.s.uquestions.contains_key(&uq));
    ok(&w);
    let before = w.s.clone();
    let mut dirty = crate::facts::Tracked::new();
    let evs = restore_tracked(
        &mut w.s,
        w.clock,
        alloc::vec![Restored::PQuestion(q, row, meta)],
        &mut dirty,
    );
    assert_eq!(evs.len(), 1);
    w.log.extend(evs);
    w.consistent(&before, &dirty).unwrap();
    ok(&w);
    // The release that named it waits on it, which holds q back for good.
    let k = w.s.pquestions[&q].rechecks;
    w.emit(Ev::WaitSet {
        node: other,
        wait: Some(WaitOn::Question {
            q,
            rechecks_at_set: k,
        }),
    });
    ok(&w);
    w.at(w.clock.plus(FINISHED_RETAIN));
    w.touch(s1);
    assert!(w.s.pquestions.contains_key(&q));
    ok(&w);
    assert_eq!(w.replay(), w.s);
    // A user question that was never made is still missing: E1 (d) and M1 report it.
    let mut s = w.s.clone();
    let ghost = Eid(s.counters.next_eid.0 + 5);
    s.pquestions.get_mut(&q).unwrap().state = PqState::Escalated { uq: ghost };
    let v = violations(&s, w.clock);
    assert!(has(&v, Inv::E1, &ViolationDetail::Entry(q)), "{v:?}");
    assert!(has(&v, Inv::M1, &ViolationDetail::Entry(q)), "{v:?}");
}

// ------------------------------------------------------------------------------------------------
// An occurrence's `since` is its slot (§10.4, D.8.5)

#[test]
fn an_occurrence_offered_again_after_a_hold_keeps_its_slot_as_since() {
    // §10.4: `candidate_since` is when the item became a candidate, "for an occurrence, its slot".
    let mut w = world();
    let s1 = w.session("w");
    let mut si = init("series");
    si.role = series_role(Dur(6 * HOUR.0));
    let x = w.create(ROOT, "series", si);
    let slot = Time((T0.0 / HOUR.0 + 1) * HOUR.0);
    let o = occurrence(&mut w, x, slot, Dur(6 * HOUR.0));
    w.at(slot);
    w.emit(Ev::Fired {
        kind: DeadlineKind::OccurrenceSlot,
        owner: Owner::Node(o),
    });
    w.emit(Ev::OccurrenceAvailable { node: o });
    assert_eq!(w.s.since[&Cand::Node(o)], slot);
    // Taken ten minutes after its slot and released ten minutes later: offered again, still at
    // its slot, in its slot's hour group.
    w.at(slot.plus(Dur(10 * MINUTE.0)));
    w.touch(s1);
    let t = w.take(o, s1);
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(o)));
    assert!(!w.s.since.contains_key(&Cand::Node(o)));
    w.at(slot.plus(Dur(20 * MINUTE.0)));
    w.emit(Ev::HoldEnded {
        token: t,
        end: HoldEnd::Released,
    });
    assert_eq!(w.s.since[&Cand::Node(o)], slot);
    let at = &w.s.ix.next.of[&Cand::Node(o)];
    assert_eq!(at.since, slot);
    // 1.11 (§10.4): in the band of a candidate with no request and no needs.
    assert_eq!(
        at.key.group,
        crate::index::group_of(slot, 0) + crate::index::band(false, false, false, 0)
    );
    assert_eq!(recount(&w.s, w.clock).next, w.s.ix.next);
    assert_eq!(w.replay(), w.s);
}

#[test]
fn a_closed_occurrence_offered_before_its_slot_has_since_now() {
    // An occurrence dropped before its slot, with an objection, becomes a closed candidate once
    // its series closes: its slot lies ahead, so its `since` is the time it appeared (R1: no
    // `since` after now).
    let mut w = world();
    let s1 = w.session("w");
    let mut si = init("series");
    si.role = series_role(HOUR);
    let x = w.create(ROOT, "series", si);
    let slot = Time((T0.0 / HOUR.0 + 5) * HOUR.0);
    let o = occurrence(&mut w, x, slot, HOUR);
    // 1.9: an unanswered request flags it; an objection would make it a review item (§10.4).
    w.append(
        o,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/9/1"),
            conv: KeyStr::new("chat:c/9").unwrap(),
            from_user: None,
            requester: Requester::External,
        },
    );
    w.emit(Ev::NodeClosed {
        node: o,
        outcome: Outcome::Dropped,
        merged_into: None,
        by: None,
    });
    assert!(!w.s.ix.next.of.contains_key(&Cand::Node(o)));
    w.at(T0.plus(MINUTE));
    w.emit(Ev::NodeClosed {
        node: x,
        outcome: Outcome::Done,
        merged_into: None,
        by: None,
    });
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(o)));
    assert_eq!(w.s.since[&Cand::Node(o)], w.clock);
    assert_eq!(recount(&w.s, w.clock).next, w.s.ix.next);
}

// ------------------------------------------------------------------------------------------------
// K2's `repeat:` keys (§26 K2, D.9.2; the reading recorded at `k2`)

#[test]
fn k2_keeps_a_detached_occurrences_key_and_refuses_one_that_names_no_node() {
    // Removing `repeat` from a series detaches its available occurrence, which keeps its key
    // while the former series is Plain (§12): a state the literal "names a series" would reject.
    let mut w = world();
    let mut si = init("series");
    si.role = series_role(HOUR);
    let x = w.create(ROOT, "series", si);
    let slot = Time((T0.0 / HOUR.0 + 1) * HOUR.0);
    let o = occurrence(&mut w, x, slot, HOUR);
    w.at(slot);
    w.emit(Ev::Fired {
        kind: DeadlineKind::OccurrenceSlot,
        owner: Owner::Node(o),
    });
    w.emit(Ev::OccurrenceAvailable { node: o });
    w.emit(Ev::OccurrenceDetached { node: o });
    w.emit(Ev::NodeUpdated {
        node: x,
        patch: NodePatch {
            repeat: Change::Clear,
            ..NodePatch::default()
        },
    });
    assert_eq!(w.s.nodes[&x].role, Role::Plain);
    assert_eq!(w.s.nodes[&o].role, Role::Plain);
    let k = KeyStr::of_occurrence(x, slot);
    assert!(w.s.nodes[&o].keys.contains_key(&k));
    let v = violations(&w.s, w.clock);
    assert!(!v.iter().any(|x| x.inv == Inv::K2), "{v:?}");
    // A repeat: key naming no node, or with a slot name that does not decode, is refused.
    let mut s = w.s.clone();
    let nowhere = KeyStr::of_occurrence(NodeId(999), slot);
    let garbled = kstr("repeat:9:2026-01-01");
    for bad in [&nowhere, &garbled] {
        s.nodes
            .get_mut(&o)
            .unwrap()
            .keys
            .insert(bad.clone(), NormVer(0));
    }
    let v = violations(&s, w.clock);
    for bad in [nowhere, garbled] {
        assert!(has(&v, Inv::K2, &ViolationDetail::Key(bad)), "{v:?}");
    }
}

// ------------------------------------------------------------------------------------------------
// The op-record prune's boundary (O1, D.12.5 `op_expiry`)

#[test]
fn an_op_record_leaves_at_the_first_event_at_its_expiry() {
    let mut w = world();
    let s1 = w.session("w");
    let expires = w.clock.plus(MINUTE);
    w.emit(Ev::OpRecorded {
        class: class("w"),
        op: OpId::new("op-1").unwrap(),
        digest: ReqDigest([1; 32]),
        session: s1,
        expires_at: expires,
        outcome: Err(crate::error::Error {
            code: crate::error::ErrorCode::OpIdConflict,
            hint: crate::error::Hint::OpIdConflict,
        }),
    });
    w.at(Time(expires.0 - 1));
    w.emit(Ev::SessionStatus {
        session: s1,
        text: Text("x".into()),
    });
    assert_eq!(w.s.ops.len(), 1);
    ok(&w);
    w.at(expires);
    w.emit(Ev::SessionStatus {
        session: s1,
        text: Text("x".into()),
    });
    assert!(w.s.ops.is_empty());
    assert!(w.s.ix.ops_expiry.is_empty());
    ok(&w);
}

// ------------------------------------------------------------------------------------------------
// The owed sets (D.4.10, rules 6 and 7 of §20.9)

#[test]
fn owed_reply_needs_a_listing_decision_and_owed_decision_an_answer() {
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let mail = kstr("mail:m@x");
    let request = |w: &mut World, origin: &str, conv: &KeyStr| {
        w.append(
            n,
            Author::Session(s1),
            EntryBody::Request {
                origin: key(origin),
                conv: conv.clone(),
                from_user: None,
                requester: Requester::External,
            },
        )
    };
    let r = request(&mut w, "chat:c/1/7", &chat);
    let r2 = request(&mut w, "mail:m@x/9", &mail);
    let owed = |w: &World| {
        let it = w.s.ix.items.get(&n).cloned().unwrap_or_default();
        (it.owed_reply, it.owed_decision)
    };
    // Unanswered and listed by no decision: owed nothing yet.
    assert_eq!(owed(&w), (BTreeSet::new(), BTreeSet::new()));
    let d = w.append(n, Author::Session(s1), decision([r].into()));
    // Listed: r is owed a reply; r2, listed by no decision, is not; neither is owed a decision.
    assert_eq!(owed(&w), ([r].into(), BTreeSet::new()));
    // Retracting the decision takes r out again (`ix.settling` holds no retracted decision).
    let x = w.append(
        n,
        Author::Session(s1),
        EntryBody::Retract {
            target: BasisTarget::Entry(d),
        },
    );
    w.emit(Ev::DecisionRetracted {
        d,
        by: x,
        node: None,
    });
    assert_eq!(owed(&w), (BTreeSet::new(), BTreeSet::new()));
    // Answered and unsettled: owed a decision, and no longer a reply.
    let a = w.append(n, Author::Session(s1), said(&mail, SaidKind::Reply));
    w.emit(Ev::RequestAnswered { request: r2, by: a });
    assert_eq!(owed(&w), (BTreeSet::new(), [r2].into()));
    let d2 = w.append(n, Author::Session(s1), decision([r, r2].into()));
    assert_eq!(owed(&w), ([r].into(), [r2].into()));
    w.emit(Ev::RequestSettled {
        request: r2,
        decision: d2,
        answer: a,
    });
    assert_eq!(owed(&w), ([r].into(), BTreeSet::new()));
    ok(&w);
}

// ------------------------------------------------------------------------------------------------
// Preconditions whose failure changes nothing (D.7.4, D.7.5)

#[test]
fn a_failed_precondition_counts_an_anomaly_and_changes_nothing() {
    let mut w = world();
    let (s1, n, chat) = conversation(&mut w);
    let mut di = init("due");
    di.due = Some(T0.plus(HOUR));
    let due = w.create(ROOT, "due", di);
    let r = w.append(
        n,
        Author::Session(s1),
        EntryBody::Request {
            origin: key("chat:c/1/7"),
            conv: chat,
            from_user: None,
            requester: Requester::External,
        },
    );
    // A decision that does not list r.
    let d = w.append(n, Author::Session(s1), decision(BTreeSet::new()));
    let p = w.s.params.clone();
    let bad = [
        // CS-8: the taker must retire after the hold's ceiling.
        Ev::HoldIssued {
            token: Token {
                node: n,
                fence: Fence(w.s.nodes[&n].fence.0 + 1),
            },
            session: s1,
            lease_until: w.clock.plus(p.lease_first),
            terms: LeaseTerms {
                ceiling: p.session_retain,
                ..terms(&p)
            },
            l: p.lease_default,
            reopened: None,
        },
        // The decision must be in `ix.settling[r]`.
        Ev::RequestSettled {
            request: r,
            decision: d,
            answer: d,
        },
        // A firing before its time, and one of a key that is not queued.
        Ev::Fired {
            kind: DeadlineKind::Due,
            owner: Owner::Node(due),
        },
        Ev::Fired {
            kind: DeadlineKind::Wait,
            owner: Owner::Node(due),
        },
        Ev::Fired {
            kind: DeadlineKind::Lease,
            owner: Owner::Hold(Token {
                node: n,
                fence: Fence(1),
            }),
        },
    ];
    for ev in bad {
        let before = w.s.clone();
        let dirty = w.emit_raw(ev.clone());
        assert_eq!(w.s.anomalies, before.anomalies + 1, "{ev:?}");
        let mut after = w.s.clone();
        after.anomalies = before.anomalies;
        after.counters.events = before.counters.events;
        assert_eq!(after, before, "{ev:?}");
        w.consistent(&before, &dirty).unwrap();
    }
    // The same events at their time, or with their precondition met, apply.
    w.at(T0.plus(HOUR));
    w.emit(Ev::Fired {
        kind: DeadlineKind::Due,
        owner: Owner::Node(due),
    });
    assert!(w.s.nodes[&due].due_fired);
}
