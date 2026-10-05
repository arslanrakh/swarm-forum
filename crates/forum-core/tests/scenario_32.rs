//! The §32 example as a scenario test at core level (design D.12.2, WP-D6): the eight steps of the
//! example, through the shell's pipeline (pre-settle, `apply`, post-settle) with a fake clock,
//! with `check_fire` after every firing, `check_step` after every command, and
//! `check_invariants` after every call; at the end no word of the user is unrouted, and folding
//! the whole history from Genesis gives the state.
//!
//! The clock starts at 2026-10-01T09:00Z. Keys: `user:e1` and `chat:team/thread-1`; the person's
//! message in the thread is `chat:team/thread-1/1001`, the origin of `r2` (A-61). Both clients
//! serve the user without naming them, so their `hello`s pass `principal: user` (1.9, §8.1), and
//! every user entry and report belongs to `user` (CS-1).
//!
//! The next calls of every response pass G1 (`check_guide`, WP-D7); their values against D.16 are
//! asserted step by step, the guided run included, in `src/query/guide_tests.rs` (WP-D7).
//!
//! Step 1's clarification is an open question (reported with WP-D6): D.12.2's row
//! and §32 say it "attaches to `r1`, no new request" (`routed: {clarifies: r1}`), but D.12.2's
//! call order settles `r1` by the `to_user` just before it, and 1.4's addendum (D.14.5, §8.1,
//! §8.2, the decision of 2026-10-02) records a clarification of a settled request as a new
//! request that follows it. Until it is settled which changes (the order of step 1, or D.12.2
//! and §32), this test asserts only what both readings share: the clarification is routed to the
//! series and names `r1`, either as `clarifies: r1` or as a new request there that follows `r1`.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "test code: the deny list guards the crate's own code"
)]

use std::collections::{BTreeMap, BTreeSet};

use forum_core::query::{self, NoMeter};
use forum_core::*;

/// 2026-10-01T09:00Z.
const START: Time = Time(1_790_845_200_000);
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;

/// The world: the state, the clock, and every event since Genesis.
struct W {
    s: State,
    clock: Time,
    log: Vec<Event>,
}

impl W {
    fn new() -> W {
        let mut s = State::default();
        let g = genesis(Params::default(), START);
        evolve_mut(&mut s, &g);
        W {
            s,
            clock: START,
            log: vec![g],
        }
    }

    /// Fires every deadline due at the clock, one at a time, each checked (D.8.2).
    fn settle(&mut self) {
        while let Some(key) = self.s.ix.deadlines.first().cloned() {
            if key.at > self.clock {
                break;
            }
            let before = self.s.clone();
            let (evs, st) = settle(&mut self.s, self.clock, 1);
            assert!(!matches!(st, Settled::Stuck(_)), "stuck at {key:?}");
            if let Err(v) = check_fire(&before, &key, &evs, &self.s) {
                panic!("check_fire {key:?}: {v:?}\n{evs:?}");
            }
            self.log.extend(evs);
        }
    }

    fn advance(&mut self, ms: u64) {
        self.clock = self.clock.plus(Dur(ms));
        self.settle();
        check_invariants(&self.s, self.clock).unwrap();
    }

    /// One call through the pipeline, with its checks (D.8.1).
    fn call(&mut self, class: &str, session: SessionCtx, cmd: Cmd) -> Result<Done, Error> {
        self.settle();
        let ctx = Ctx {
            now: self.clock,
            class: ClassName::new(class).unwrap(),
            caps: None,
            session,
            op: None,
        };
        let before = self.s.clone();
        let (evs, out) = apply(&mut self.s, &ctx, &cmd);
        if let Err(v) = check_step(&before, &ctx, &cmd, &out, &evs, &self.s) {
            panic!("check_step {cmd:?}: {v:?}\n{out:?}\n{evs:?}");
        }
        assert_eq!(self.s.anomalies, 0, "an anomaly in {cmd:?}");
        // The session the next calls are for, as the writer passes it (D.8.26): the call's, or
        // the one a first call opened; none when the session check refused the call.
        let gs = match (&out, session) {
            (
                Err(Error {
                    hint: Hint::NotFound(What::Session) | Hint::Invalid(Why::ClassMismatch),
                    ..
                }),
                _,
            )
            | (_, SessionCtx::System) => None,
            (_, SessionCtx::Existing(x)) => Some(x),
            (_, SessionCtx::New) => out.as_ref().ok().and_then(|d| d.opened).or_else(|| {
                evs.iter().find_map(|e| match &e.ev {
                    Ev::SessionOpened { session, .. } => Some(*session),
                    _ => None,
                })
            }),
        };
        self.log.extend(evs);
        self.settle();
        if let Err(v) = check_invariants(&self.s, self.clock) {
            panic!("invariants after {cmd:?}: {v:?}");
        }
        // G1 (D.8.26, WP-D7): the next calls of the response, on the settled state.
        let g = query::guide(&self.s, gs, self.clock, &cmd, &out, &mut NoMeter);
        if let Err(v) = check_guide(&self.s, gs, self.clock, &cmd, &out, &g) {
            panic!("check_guide after {cmd:?} -> {out:?}: {v:?}\nnext calls {g:?}");
        }
        out
    }

    /// A call by an existing session that must succeed.
    fn ok(&mut self, class: &str, s: SessionId, cmd: Cmd) -> Done {
        self.call(class, SessionCtx::Existing(s), cmd)
            .unwrap_or_else(|e| panic!("refused: {e:?}"))
    }

    /// A first call, which opens its session, and must succeed.
    fn first(&mut self, class: &str, cmd: Cmd) -> (SessionId, Done) {
        let d = self
            .call(class, SessionCtx::New, cmd)
            .unwrap_or_else(|e| panic!("refused: {e:?}"));
        (d.opened.expect("a session opened"), d)
    }

    fn node(&self, path: &str) -> NodeId {
        let mut n = ROOT;
        for step in path.split('/').filter(|x| !x.is_empty()) {
            n = self.s.names[&(n, Name::new(step).unwrap())];
        }
        n
    }

    fn flags(&self, n: NodeId) -> FlagSet {
        self.s.ix.node[&n].flags
    }
}

fn text(t: &str) -> Text {
    Text(String::from(t))
}

fn path(p: &str) -> Path {
    Path(
        p.split('/')
            .filter(|x| !x.is_empty())
            .map(|x| Name::new(x).unwrap())
            .collect(),
    )
}

fn key(k: &str) -> Key {
    Key {
        key: KeyStr::new(k).unwrap(),
        nv: NormVer(1),
    }
}

fn label(l: &str) -> Label {
    Label::new(l).unwrap()
}

/// Europe/Berlin around the example: CEST until 2026-10-25T01:00Z, then CET, and CEST again from
/// 2027-03-28T01:00Z.
fn berlin() -> forum_cron::TzTable {
    forum_cron::TzTable {
        name: String::from("Europe/Berlin"),
        tzdb_version: String::from("2026c"),
        initial: 3_600,
        transitions: vec![
            forum_cron::Transition {
                at: 1_774_746_000,
                offset: 7_200,
            },
            forum_cron::Transition {
                at: 1_792_890_000,
                offset: 3_600,
            },
            forum_cron::Transition {
                at: 1_806_195_600,
                offset: 7_200,
            },
        ],
    }
}

fn create(p: &str, f: impl FnOnce(&mut CreateArgs)) -> Cmd {
    let mut a = CreateArgs {
        path: path(p),
        keys: Vec::new(),
        kind: None,
        title: text("t"),
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
    };
    f(&mut a);
    Cmd::Node(NodeCmd::Create(a))
}

fn said(at: NodeId, kind: SaidKind, t: &str) -> Cmd {
    Cmd::Talk(TalkCmd::Said {
        at: NodeRef::Id(at),
        where_: None,
        text: text(t),
        kind,
        corrects: None,
        seen: None,
    })
}

fn confirm(a: Eid) -> Cmd {
    Cmd::Talk(TalkCmd::SaidConfirm {
        said: a,
        confirmed: text("https://chat.example/team/thread-1/2002"),
    })
}

fn decide(at: NodeId, settles: &[Eid], t: &str) -> Cmd {
    Cmd::Talk(TalkCmd::Decide {
        at: NodeRef::Id(at),
        text: text(t),
        settles: settles.iter().copied().collect(),
        supersedes: None,
        basis: Vec::new(),
        evidence: None,
    })
}

fn take(n: NodeId) -> Cmd {
    Cmd::Hold(HoldCmd::Take {
        target: NodeRef::Id(n),
        lease: None,
    })
}

fn close(n: NodeId, verified: Option<&str>) -> Cmd {
    Cmd::Node(NodeCmd::Close {
        at: NodeRef::Id(n),
        outcome: Outcome::Done,
        summary: text("done"),
        verified: verified.map(text),
        settles: Some(BTreeSet::new()),
        learnings: None,
        token: None,
        unsent: BTreeSet::new(),
    })
}

fn token(d: &Done) -> Token {
    match &d.reply {
        Reply::Created { token: Some(t), .. } | Reply::Taken { token: t, .. } => *t,
        other => panic!("no hold: {other:?}"),
    }
}

fn said_of(d: &Done) -> Eid {
    match &d.reply {
        Reply::Said { said, .. } => *said,
        other => panic!("not a said: {other:?}"),
    }
}

fn uncovered(d: &Done) -> Option<&WarningDetail> {
    d.warnings
        .iter()
        .find(|w| w.code == WarningCode::UncoveredSlot)
        .map(|w| &w.detail)
}

#[test]
fn the_example_of_section_32() {
    let mut w = W::new();

    // 1. The user tells their chat assistant: "summarise new support tickets every morning".
    let (assistant, d) = w.first(
        "chat-assistant",
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: PrincipalKey::new("user"),
            from_user: Some(FromUserArg {
                text: text("summarise new support tickets every morning"),
                client: text("chat"),
                principal: None,
                answers: None,
                to: None,
                clarifies: None,
                reason: None,
            }),
        }),
    );
    let e1 = match &d.reply {
        Reply::Hello {
            user_entry: Some(u),
            ..
        } => {
            assert!(matches!(u.routed, RoutedReply::Unrouted(_)), "{u:?}");
            u.entry
        }
        other => panic!("{other:?}"),
    };
    assert!(w.flags(USER).contains(Flag::Unrouted));
    let ca = "chat-assistant";
    let d = w.ok(
        ca,
        assistant,
        create("/digest/daily-tickets", |a| {
            a.keys = vec![key(&format!("user:{}", e1.render()))];
            a.title = text("Daily digest of new support tickets");
            a.repeat = Some(RepeatArg {
                cron_text: text("0 7 * * *"),
                cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
                tz: berlin(),
                run_within: Dur(2 * HOUR_MS),
            });
        }),
    );
    let series = w.node("/digest/daily-tickets");
    let r1 = match &d.reply {
        Reply::Created { user_requests, .. } => {
            assert_eq!(user_requests.len(), 1);
            user_requests[0]
        }
        other => panic!("{other:?}"),
    };
    assert!(!w.flags(USER).contains(Flag::Unrouted));
    // One waiting occurrence, named after its UTC slot (07:00 CEST is 05:00Z).
    let occ = w.s.ix.series_open[&series].clone();
    assert_eq!(occ.len(), 1);
    let o = *occ.iter().next().unwrap();
    assert_eq!(w.node("/digest/daily-tickets/20261002T0500Z"), o);
    assert!(matches!(&w.s.nodes[&o].role, Role::Occurrence(x) if x.phase == OccPhase::Waiting));
    // No session history: no usual hour, no first start, and the run will likely be missed.
    match uncovered(&d) {
        Some(WarningDetail::UncoveredSlot {
            covered_days,
            days,
            usual_hours,
            first_start,
        }) => {
            assert_eq!((*covered_days, *days), (0, 28));
            assert!(usual_hours.is_empty());
            assert_eq!(*first_start, None);
        }
        other => panic!("{other:?}"),
    }
    w.ok(
        ca,
        assistant,
        decide(series, &[r1], "set up the daily digest"),
    );
    w.ok(
        ca,
        assistant,
        Cmd::Talk(TalkCmd::ToUser {
            text: text("The digest runs every morning at 07:00 Berlin time."),
            kind: ToUserKind::Result,
            verbatim: true,
            principal: None,
            re: [e1].into(),
            at: None,
        }),
    );
    assert!(
        w.s.requests[&r1].settled.is_some(),
        "r1 settled after the report"
    );
    let d = w.ok(
        ca,
        assistant,
        Cmd::Talk(TalkCmd::FromUser {
            text: text("and skip tickets tagged spam"),
            client: text("chat"),
            principal: None,
            answers: None,
            to: None,
            clarifies: Some(r1),
            reason: None,
        }),
    );
    // Either reading of step 1 (see the module's note): it attaches to `r1`, or it is a new
    // request on the series that follows `r1`.
    match &d.reply {
        Reply::FromUser {
            routed: RoutedReply::Clarifies { request },
            ..
        } => assert_eq!(*request, r1),
        Reply::FromUser {
            routed:
                RoutedReply::Request {
                    node,
                    follows: Some(f),
                    ..
                },
            ..
        } => assert_eq!((*node, *f), (series, r1)),
        other => panic!("{other:?}"),
    }

    // 2. A person writes in a team chat; two agent runs start on the same message.
    let request = |w: &W| -> Cmd {
        let _ = w;
        create("/summary-gap", |a| {
            a.keys = vec![key("chat:team/thread-1")];
            a.title = text("Why did yesterday's summary miss two tickets?");
            a.request = Some(RequestArg {
                text: text("why did yesterday's summary miss two tickets?"),
                origin: Some(key("chat:team/thread-1/1001")),
                where_: None,
                requester: text("alex"),
            });
        })
    };
    let cmd = request(&w);
    let (s1, d) = w.first("chat-worker", cmd);
    let gap = w.node("/summary-gap");
    let t1 = token(&d);
    let r2 = match &d.reply {
        Reply::Created {
            request: Some(r), ..
        } => *r,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        w.s.requests[&r2].origin.key.as_str(),
        "chat:team/thread-1/1001"
    );
    assert_eq!(w.s.requests[&r2].conv.as_str(), "chat:team/thread-1");
    let cmd = request(&w);
    let (_s2, d) = w.first("chat-worker", cmd);
    match &d.reply {
        Reply::Joined {
            existing, request, ..
        } => {
            assert_eq!(*existing, gap);
            assert_eq!(*request, None, "the same message: the same request (Q1)");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(w.s.requests.values().filter(|r| r.node == gap).count(), 1);

    // 3. s-1 acknowledges, splits the work, and releases summary-gap.
    let cw = "chat-worker";
    let ack = said_of(&w.ok(cw, s1, said(gap, SaidKind::Ack, "looking")));
    w.ok(cw, s1, confirm(ack));
    assert!(
        w.s.commitments
            .values()
            .any(|c| c.node == gap && c.state == CommitmentState::Open)
    );
    for p in ["/summary-gap/audit", "/summary-gap/rerun-test"] {
        w.ok(
            cw,
            s1,
            create(p, |a| {
                a.take = Some(false);
                a.tags = [label("offer")].into();
            }),
        );
    }
    let audit = w.node("/summary-gap/audit");
    let rerun = w.node("/summary-gap/rerun-test");
    w.ok(
        cw,
        s1,
        Cmd::Hold(HoldCmd::Release {
            token: t1,
            done: text("acknowledged and split the work"),
            next_step: text("audit and re-run"),
            reason: ReleaseReason::Handoff,
            waits_for: None,
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    );
    for n in [audit, rerun] {
        assert!(w.s.ix.next.of.contains_key(&Cand::Node(n)), "{n:?} offered");
    }
    assert!(w.flags(gap).contains(Flag::UnansweredRequest));
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(gap)));

    // 4. s-3 takes audit, asks in the thread, and releases waiting for the answer.
    let (s3, d) = w.first(cw, take(audit));
    let t3 = token(&d);
    let d = w.ok(
        cw,
        s3,
        Cmd::Talk(TalkCmd::Ask {
            at: NodeRef::Id(audit),
            text: text("were the two tickets created after the cutoff?"),
            to: AskTo::Person {
                where_: None,
                blocking: false,
            },
            again: false,
        }),
    );
    let (q1, ask) = match &d.reply {
        Reply::Asked {
            question,
            said: Some(a),
            node,
            ..
        } => {
            assert_eq!(*node, gap, "the conversation node (C1)");
            (*question, *a)
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(w.s.pquestions[&q1].node, gap);
    assert_eq!(w.s.saids[&ask].node, gap);
    assert!(
        !d.warnings
            .iter()
            .any(|x| x.code == WarningCode::ConversationHeld),
        "s-3 holds a descendant"
    );
    match d.warnings.iter().find(|x| x.code == WarningCode::Moved) {
        Some(Warning {
            detail: WarningDetail::Moved { entries, .. },
            ..
        }) => {
            assert!(
                entries.contains(&r2) && entries.contains(&ack),
                "{entries:?}"
            );
        }
        other => panic!("moved: {other:?}"),
    }
    w.ok(
        cw,
        s3,
        Cmd::Hold(HoldCmd::Release {
            token: t3,
            done: text("asked in the thread"),
            next_step: text("wait for the answer"),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::Question(q1)),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
    );
    assert_eq!(
        w.s.saids[&ask].state,
        SaidState::Confirmed,
        "the release confirms it"
    );
    assert!(matches!(
        w.s.nodes[&audit].wait.as_ref().map(|x| x.on),
        Some(WaitOn::Question { q, .. }) if q == q1
    ));
    assert!(
        w.s.ix
            .deadlines
            .iter()
            .any(|k| k.kind == DeadlineKind::Recheck
                && k.owner == Owner::Entry(q1)
                && k.at == w.clock.plus(Dur(4 * HOUR_MS)))
    );
    w.advance(2 * HOUR_MS);
    assert!(
        !w.flags(gap).contains(Flag::OpenCommitment),
        "the open question suppresses it"
    );

    // 5. The person answers; s-4 runs on that event.
    let (s4, d) = w.first(
        cw,
        create("/summary-gap", |a| a.keys = vec![key("chat:team/thread-1")]),
    );
    match &d.reply {
        Reply::Joined {
            existing,
            open_question,
            ..
        } => {
            assert_eq!((*existing, *open_question), (gap, Some(q1)));
        }
        other => panic!("{other:?}"),
    }
    w.ok(
        cw,
        s4,
        Cmd::Talk(TalkCmd::Heard {
            at: NodeRef::Id(gap),
            where_: None,
            from: text("alex"),
            text: text("yes, both after the cutoff"),
            answers: Some(q1),
        }),
    );
    assert!(matches!(
        w.s.pquestions[&q1].state,
        PqState::Answered { .. }
    ));
    assert!(
        !spec::waiting(&w.s, audit, w.clock),
        "audit's wait resolved"
    );
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(audit)));
    assert!(w.flags(gap).contains(Flag::OpenCommitment));

    // 6. s-5 takes rerun-test, records an intent, and disappears; s-6 takes it over.
    let (s5, d) = w.first(cw, take(rerun));
    let t5 = token(&d);
    w.ok(
        cw,
        s5,
        Cmd::Hold(HoldCmd::Checkpoint {
            token: t5,
            done: text("prepared the test environment"),
            next_step: text("re-run the digest"),
            intent: Some(text("re-run yesterday's digest in a test environment")),
            intent_done: None,
            files: Vec::new(),
        }),
    );
    // The checkpoint extended the lease to 20 minutes: s-5 is not silent at 15 (R27).
    w.advance(16 * MINUTE_MS);
    assert!(!w.s.sessions[&s5].silent, "a holder is never silent");
    w.advance(5 * MINUTE_MS);
    assert!(!w.s.holds.contains_key(&t5), "the hold expired");
    assert!(w.s.sessions[&s5].silent, "silent at its lease's end");
    assert!(w.flags(rerun).contains(Flag::OpenIntent));
    assert!(w.s.nodes[&rerun].tags.contains(&label("offer")));
    let (s6, _) = w.first(
        cw,
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: None,
            from_user: None,
        }),
    );
    let (ranked, _) = query::ranked(&w.s, s6, w.clock, &NextQuery::default(), 3, &mut NoMeter);
    assert_eq!(ranked[0].cand, Cand::Node(gap), "tier 0, older (A-62)");
    let brief = query::brief(&w.s, rerun, Some(s6), w.clock, &mut NoMeter);
    assert!(
        format!("{brief:?}").contains("OpenIntent"),
        "the brief shows open_intent"
    );
    let t6 = token(&w.ok(cw, s6, take(rerun)));
    w.ok(
        cw,
        s6,
        Cmd::Hold(HoldCmd::Checkpoint {
            token: t6,
            done: text("checked: the re-run had not happened; re-ran it"),
            next_step: text("close"),
            intent: None,
            intent_done: Some(text("re-ran it myself")),
            files: Vec::new(),
        }),
    );
    w.ok(
        cw,
        s6,
        close(rerun, Some("the re-run digest lists both tickets")),
    );

    // 7. audit closes with a decision; s-7 settles the request and closes summary-gap.
    let t = token(&w.ok(cw, s6, take(audit)));
    let _ = t;
    w.ok(cw, s6, decide(audit, &[], "the cutoff excluded them"));
    let d = w.ok(cw, s6, close(audit, None));
    match &d.reply {
        Reply::Closed { leave_better, .. } => {
            assert_eq!(
                leave_better.first(),
                Some(&gap),
                "its parent at its level (A-63)"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(w.flags(gap).contains(Flag::ReadyToClose));
    let (s7, _) = w.first(cw, take(gap));
    w.ok(cw, s7, decide(gap, &[r2], "move the cutoff to midnight"));
    let result = said_of(&w.ok(
        cw,
        s7,
        said(
            gap,
            SaidKind::Result,
            "they were created after the cutoff; fixed",
        ),
    ));
    w.ok(cw, s7, confirm(result));
    assert!(w.s.requests[&r2].settled.is_some());
    assert!(
        w.s.commitments
            .values()
            .all(|c| c.node != gap || c.state != CommitmentState::Open)
    );
    let d = w.ok(cw, s7, close(gap, None));
    assert!(d.warnings.is_empty(), "no warnings: {:?}", d.warnings);

    // 8. The user asks a coding agent: "what about my tasks?".
    let (coder, d) = w.first(
        "coding-agent",
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: PrincipalKey::new("user"),
            from_user: Some(FromUserArg {
                text: text("what about my tasks?"),
                client: text("ide"),
                principal: None,
                answers: None,
                to: None,
                clarifies: None,
                reason: None,
            }),
        }),
    );
    let e = match &d.reply {
        Reply::Hello {
            user_entry: Some(u),
            ..
        } => {
            assert!(matches!(u.routed, RoutedReply::Unrouted(_)));
            u.entry
        }
        other => panic!("{other:?}"),
    };
    assert!(w.s.sessions[&coder].principals.contains(&USER_PRINCIPAL));
    w.ok(
        "coding-agent",
        coder,
        Cmd::View(ViewCmd::Requests {
            status: None,
            requester: None,
        }),
    );
    let scope = query::board_scope(&w.s, coder, None);
    assert_eq!(
        scope,
        BoardScope::All,
        "user is the only principal seen (§8.3)"
    );
    let page = query::board(&w.s, None, &scope, w.clock, 50, &mut NoMeter);
    let row = |pred: &dyn Fn(&query::BoardRow) -> bool| page.rows.iter().find(|r| pred(r));
    assert!(row(&|r| r.entry == Some(e) && r.state == BoardState::Unrouted).is_some());
    assert!(row(&|r| r.request == Some(r1) && r.state == BoardState::Answered).is_some());
    assert!(row(&|r| r.request == Some(r2) && r.state == BoardState::Answered).is_some());
    let d = w.ok(
        "coding-agent",
        coder,
        Cmd::Talk(TalkCmd::ToUser {
            text: text("The daily digest is set up, and the team chat has its answer."),
            kind: ToUserKind::Result,
            verbatim: true,
            principal: None,
            re: [e].into(),
            at: None,
        }),
    );
    // It answers no request, but replies to the user's own entry: no `result_answered_nothing`
    // (§21, 1.5).
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let d = w.ok(
        "coding-agent",
        coder,
        Cmd::Talk(TalkCmd::Route {
            entry: e,
            to: Some(forum_core::RouteTo::Nowhere),
            answers: None,
            clarifies: None,
            reason: Some(text("answered from the request board")),
        }),
    );
    assert!(matches!(
        &d.reply,
        Reply::Routed {
            routed: RoutedReply::NoAction,
            ..
        }
    ));
    assert!(!w.flags(USER).contains(Flag::Unrouted));

    // Throughout, the invariants held; at the end no word of the user is unrouted, and the
    // history folds to the state.
    assert!(
        w.s.user_entries
            .values()
            .all(|u| u.routing != Routing::Unrouted)
    );
    let mut folded = State::default();
    for ev in &w.log {
        evolve_mut(&mut folded, ev);
    }
    assert!(folded == w.s, "the history folds to the state");
    // The series' first run comes at its slot: available, a candidate, and copied afresh.
    w.clock = Time(1_790_917_200_000);
    w.settle();
    assert!(matches!(&w.s.nodes[&o].role, Role::Occurrence(x) if x.phase == OccPhase::Available));
    assert!(w.s.ix.next.of.contains_key(&Cand::Node(o)));
    check_invariants(&w.s, w.clock).unwrap();
}
