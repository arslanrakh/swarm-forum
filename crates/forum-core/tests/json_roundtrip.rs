//! The JSON round trip of every type (design D.7.6, D.13.2 WP-D1): every value decodes to itself
//! and re-encodes to the same bytes; every variant of every enum is covered, every `Change<T>`
//! state of `Patch` and `NodePatch` included; a map whose keys are neither strings nor integers
//! goes through `serde_pairs`; and the validated newtypes refuse invalid values when decoding.
//!
//! The big enums are covered through an exhaustive `match` that numbers their variants: a new
//! variant does not compile until it is numbered, and the test then fails until a sample of it is
//! added.

#![cfg(feature = "serde")]
#![allow(clippy::too_many_lines, reason = "one sample per variant")]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use forum_core::health::{
    CapacityReport, Health, HealthReason, Level, MoverGap, PauseExclusion, UnreachableBy,
};
use forum_core::invariants::registry::{INVARIANTS, InvKind, VerusStatus};
use forum_core::query::guide::{
    Because, End, Guidance, Moment, NextCall, RouteAlt, RouteSk, Rule, Skeleton,
};
use forum_core::query::{
    AgreementsPage, Backlinks, BoardPage, BoardRow, BriefData, BriefItem, FindResult, HelloData,
    HoldStatus, ListFilter, ListPage, Orientation, PauseView, Predecessor, Present, Ranked,
    RestsOn, RestsVia, Rollup, Section, SectionName, StatusLine, UserItems, WhoPage,
};
use forum_core::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

// ------------------------------------------------------------------------------------------------
// The round trip

/// Encodes, decodes, compares, and re-encodes to the same bytes (canonical).
fn rt<T: Serialize + DeserializeOwned + PartialEq + Debug>(v: &T) {
    let json = serde_json::to_string(v).unwrap();
    let back: T = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("decoding {json} into {}: {e}", std::any::type_name::<T>()));
    assert_eq!(&back, v, "{json}");
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
}

fn rt_all<T: Serialize + DeserializeOwned + PartialEq + Debug>(vs: &[T]) {
    for v in vs {
        rt(v);
    }
}

/// Every index in `0..n` is covered by `vs`, numbered by `index`.
fn covers<T>(vs: &[T], n: usize, index: impl Fn(&T) -> usize) {
    let seen: BTreeSet<usize> = vs.iter().map(index).collect();
    let missing: Vec<usize> = (0..n).filter(|i| !seen.contains(i)).collect();
    assert!(missing.is_empty(), "variants without a sample: {missing:?}");
}

// ------------------------------------------------------------------------------------------------
// Samples

fn label(s: &str) -> Label {
    Label::new(s).unwrap()
}
fn labels(s: &[&str]) -> BTreeSet<Label> {
    s.iter().map(|x| label(x)).collect()
}
fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}
fn class(s: &str) -> ClassName {
    ClassName::new(s).unwrap()
}
fn key(s: &str) -> KeyStr {
    KeyStr::new(s).unwrap()
}
fn nkey(s: &str) -> Key {
    Key {
        key: key(s),
        nv: NormVer(1),
    }
}
fn pk(s: &str) -> PrincipalKey {
    PrincipalKey::new(s).unwrap()
}
fn text(s: &str) -> Text {
    Text(s.to_owned())
}
fn path(steps: &[&str]) -> Path {
    Path(steps.iter().map(|s| name(s)).collect())
}
fn nref() -> NodeRef {
    NodeRef::Path(path(&["a", "b c"]))
}
fn tok() -> Token {
    Token {
        node: NodeId(12),
        fence: Fence(3),
    }
}
fn digest() -> ReqDigest {
    let mut d = [0_u8; 32];
    for (i, b) in d.iter_mut().enumerate() {
        *b = u8::try_from(i * 7 % 256).unwrap();
    }
    ReqDigest(d)
}
fn fields() -> BTreeMap<Text, Text> {
    BTreeMap::from([(text("k"), text("{\"a\":1}")), (text("z"), text("\"x\""))])
}
fn tz() -> forum_cron::TzTable {
    forum_cron::TzTable {
        name: "Europe/Berlin".to_owned(),
        tzdb_version: "2026c".to_owned(),
        initial: 3_600,
        transitions: vec![forum_cron::Transition {
            at: 1_774_746_000,
            offset: 7_200,
        }],
    }
}
fn repeat() -> Repeat {
    Repeat {
        cron_text: text("0 7 * * *"),
        cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
        tz: tz(),
        run_within: Dur(7_200_000),
    }
}
fn repeat_arg() -> RepeatArg {
    RepeatArg {
        cron_text: text("*/10 * * * *"),
        cron: forum_cron::parse_cron("*/10 * * * *").unwrap(),
        tz: tz(),
        run_within: Dur(240_000),
    }
}
fn meta() -> EntryMeta {
    EntryMeta {
        node: NodeId(9),
        seq: Seq(4),
        at: Time(1_000),
        by: Author::Session(SessionId(2)),
        ty: EntryType::FromUser,
        principal: Some(PrincipalId(1)),
    }
}
fn text_all() -> EntryText {
    EntryText {
        main: text("main"),
        extra: TEXT_FIELDS.iter().map(|f| (*f, text("x"))).collect(),
    }
}
const TEXT_FIELDS: [TextField; 20] = [
    TextField::Done,
    TextField::NextStep,
    TextField::Intent,
    TextField::IntentDone,
    TextField::File,
    TextField::Option,
    TextField::Recommendation,
    TextField::Default,
    TextField::Client,
    TextField::From,
    TextField::Requester,
    TextField::Summary,
    TextField::Verified,
    TextField::Learnings,
    TextField::Reason,
    TextField::Checked,
    TextField::Basis,
    TextField::Link,
    TextField::What,
    TextField::Why,
];

fn outcomes() -> Vec<Outcome> {
    vec![
        Outcome::Done,
        Outcome::Failed,
        Outcome::Dropped,
        Outcome::Superseded,
        Outcome::Missed,
        Outcome::Other(label("parked")),
    ]
}

fn request() -> Request {
    Request {
        node: NodeId(9),
        origin: nkey("chat:team/thread-1/1001"),
        conv: key("chat:team/thread-1"),
        from_user: None,
        requester: Requester::External,
        answered_by: Some(Eid(20)),
        settled: Some(Settlement {
            decision: Eid(18),
            answer: Eid(20),
        }),
        moved_from: Some(NodeId(8)),
    }
}
fn pquestion(state: PqState) -> PQuestion {
    PQuestion {
        node: NodeId(9),
        conv: key("chat:x/1"),
        blocking: true,
        said: Eid(4),
        state,
        rechecks: 1,
        recheck_at: Time(5),
        lapse: Some(Lapse::Escalated),
        assumed_by: Some(Eid(7)),
        moved_from: None,
    }
}
fn uquestion(source: UqSource, state: UqState) -> UQuestion {
    UQuestion {
        source,
        about: Some(NodeId(9)),
        to: vec![PrincipalId(2), PrincipalId(0)],
        stage: 1,
        due: Time(77),
        has_default: false,
        state,
        assumed_by: None,
    }
}
fn commitment(state: CommitmentState) -> Commitment {
    Commitment {
        node: NodeId(9),
        conv: key("mail:m@x"),
        last_ack: Time(1),
        followup_at: Time(2),
        followup_fired: true,
        state,
        moved_from: None,
    }
}
fn said(state: SaidState) -> Said {
    Said {
        node: NodeId(9),
        conv: key("chat:x/1"),
        kind: SaidKind::Result,
        session: SessionId(3),
        state,
        moved_from: Some(NodeId(10)),
        with_reply: true,
    }
}
fn decision(state: DecisionState) -> Decision {
    Decision {
        node: NodeId(9),
        settles: BTreeSet::from([Eid(3), Eid(5)]),
        state,
        basis: basis_refs(),
    }
}
fn basis_refs() -> Vec<BasisRef> {
    vec![
        BasisRef::Node {
            id: NodeId(100),
            version: Some(Version(4)),
            life_at: Some(Outcome::Done),
        },
        BasisRef::Node {
            id: NodeId(101),
            version: None,
            life_at: None,
        },
        BasisRef::Entry(Eid(57)),
        BasisRef::External {
            checked_at: Time(9),
            recheck: Some(Time(99)),
        },
        BasisRef::External {
            checked_at: Time(9),
            recheck: None,
        },
    ]
}
fn user_entry(routing: Routing) -> UserEntry {
    UserEntry {
        session: SessionId(1),
        principal: PrincipalId(0),
        at: Time(3),
        routing,
    }
}
fn routings() -> Vec<Routing> {
    vec![
        Routing::Unrouted,
        Routing::Request {
            node: NodeId(9),
            request: Eid(2),
        },
        Routing::Clarifies { request: Eid(2) },
        Routing::Answers { question: Eid(5) },
        Routing::NoAction,
    ]
}
fn digest_items() -> Vec<DigestItem> {
    vec![
        DigestItem::Assumption { question: Eid(1) },
        DigestItem::DefaultApplied { question: Eid(2) },
        DigestItem::RefusalDrop {
            node: NodeId(9),
            question: Eid(3),
        },
        DigestItem::Lapsed { question: Eid(4) },
    ]
}
fn addressees() -> Vec<Addressee> {
    vec![
        Addressee::Session(SessionId(1)),
        Addressee::Holder(NodeId(9)),
        Addressee::Holders(NodeId(0)),
        Addressee::Principal(PrincipalId(3)),
    ]
}
fn note() -> Note {
    Note {
        node: NodeId(9),
        to: Addressee::Holder(NodeId(9)),
        until: Time(50),
        recipients: BTreeSet::from([SessionId(1), SessionId(2)]),
        pending_holder: false,
        read_by: BTreeSet::from([SessionId(2)]),
    }
}
fn session() -> Session {
    Session {
        class: class("coding-agent"),
        started_at: Time(1),
        last_seen: Time(2),
        silence_at: Time(3),
        retire_at: Time(4),
        silent: false,
        caps: labels(&["x", "team:payments"]),
        horizon: Some(64),
        principals: BTreeSet::from([PrincipalId(0), PrincipalId(4)]),
        shown: BTreeMap::from([(NodeId(9), Seq(3))]),
        branch_seen: BTreeMap::from([(NodeId(0), Eid(17))]),
        told: BTreeSet::from([PrincipalId(4)]),
        handed_off: Some(forum_core::HandedOff {
            token: Token {
                node: NodeId(9),
                fence: Fence(3),
            },
            until: Time(9),
        }),
    }
}
fn terms() -> LeaseTerms {
    LeaseTerms {
        first: Dur(1),
        default: Dur(2),
        max: Dur(3),
        ceiling: Dur(4),
        renew_max: 3,
    }
}
fn hold() -> Hold {
    Hold {
        session: SessionId(1),
        taken_at: Time(1),
        lease_until: Time(9),
        last_extended: Time(5),
        terms: terms(),
        l: Dur(2),
        renews: 1,
        checkpointed: true,
        reopened: Some(Outcome::Failed),
    }
}
fn owners() -> Vec<Owner> {
    vec![
        Owner::Hold(tok()),
        Owner::Node(NodeId(9)),
        Owner::Entry(Eid(5)),
        Owner::Refusal(NodeId(9), class("worker")),
        Owner::Class(class("worker")),
        Owner::Session(SessionId(4)),
    ]
}
fn deadline_kinds() -> Vec<DeadlineKind> {
    vec![
        DeadlineKind::Lease,
        DeadlineKind::Wait,
        DeadlineKind::Recheck,
        DeadlineKind::UserQuestion,
        DeadlineKind::Commitment,
        DeadlineKind::OccurrenceSlot,
        DeadlineKind::OccurrenceWindow,
        DeadlineKind::RefusalExpiry,
        DeadlineKind::Due,
        DeadlineKind::Pause,
        DeadlineKind::Silence,
        DeadlineKind::Retire,
        DeadlineKind::Stale,
    ]
}
fn signal_key() -> SignalKey {
    SignalKey {
        kind: SignalKind::Duplicate,
        a: NodeId(9),
        b: NodeId(11),
    }
}
fn roles() -> Vec<Role> {
    vec![
        Role::Plain,
        Role::Series(Series {
            repeat: repeat(),
            last_missed: true,
        }),
        Role::Occurrence(Occurrence {
            series: NodeId(9),
            slot: Time(60_000),
            window_end: Time(120_000),
            phase: OccPhase::Waiting,
            window_fired: false,
        }),
        Role::Occurrence(Occurrence {
            series: NodeId(9),
            slot: Time(60_000),
            window_end: Time(120_000),
            phase: OccPhase::Available,
            window_fired: true,
        }),
        Role::Signal(signal_key()),
    ]
}
fn wait_ons() -> Vec<WaitOn> {
    vec![
        WaitOn::Question {
            q: Eid(4),
            rechecks_at_set: 1,
        },
        WaitOn::UserQuestion(Eid(5)),
        WaitOn::Node(NodeId(9)),
        WaitOn::Time(Time(10)),
        WaitOn::External(Time(11)),
    ]
}
fn node(role: Role, life: Life, origin: Origin) -> Node {
    Node {
        place: Some(Place {
            parent: NodeId(0),
            name: name("summary-gap"),
        }),
        origin,
        created_at: Time(1),
        title: text("why did yesterday's summary miss two tickets?"),
        body: text("body"),
        fields: fields(),
        files: BTreeMap::from([(text("report.md"), text("https://x.example/r sha256:ab"))]),
        status: text("open"),
        digest: text("digest"),
        agreements: text("review before close"),
        agreements_at: Some(Time(1)),
        kind: Some(label("task")),
        tags: labels(&["offer"]),
        capacity: 2,
        priority: -3,
        est: Some(30),
        needs: labels(&["x"]),
        due: Some(Time(99)),
        due_fired: false,
        owners: BTreeSet::from([NodeId(12)]),
        keys: BTreeMap::from([
            (key("chat:team/thread-1"), NormVer(1)),
            (key("user:e1"), NormVer(0)),
        ]),
        links: BTreeSet::from([
            (LinkRel::Continues, NodeId(3)),
            (LinkRel::Duplicate, NodeId(4)),
        ]),
        depends_on: BTreeSet::from([NodeId(10)]),
        wait: Some(Wait {
            on: WaitOn::Question {
                q: Eid(4),
                rechecks_at_set: 0,
            },
            set_at: Time(5),
            fired: false,
        }),
        role,
        life,
        version: Version(3),
        digest_version: Version(2),
        fence: Fence(4),
        seq: Seq(17),
        content_version: Version(2),
        attempts: 1,
        checkpoints: 2,
        last_checkpoint: Some(Eid(30)),
        expired_hold: Some(ExpiredHold {
            token: Token {
                node: NodeId(9),
                fence: Fence(3),
            },
            session: SessionId(3),
            at: Time(7),
        }),
        open_intent: Some(Intent {
            eid: Eid(31),
            session: SessionId(2),
            pr_named: true,
        }),
        recheck_marks: BTreeSet::from([Eid(4)]),
        assume_marks: BTreeSet::from([Eid(5)]),
        held_ms: 60_000,
        people_left: Seq(2),
        left: 1,
        touched_at: Some(Time(8)),
        stale_fired: true,
        retracted_decision: Some(Eid(7)),
        pr_named: true,
    }
}
fn lives() -> Vec<Life> {
    vec![
        Life::Open,
        Life::Closed(Closure {
            at: Time(5),
            outcome: Outcome::Superseded,
            merged_into: Some(NodeId(11)),
            upheld: None,
            corrected_from: None,
            by: None,
        }),
        Life::Closed(Closure {
            at: Time(5),
            outcome: Outcome::Other(label("refuted")),
            merged_into: None,
            upheld: Some(Eid(12)),
            corrected_from: Some(Outcome::Done),
            by: Some(SessionId(3)),
        }),
    ]
}
fn origins() -> Vec<Origin> {
    vec![
        Origin::Reserved,
        Origin::Agent(SessionId(1)),
        Origin::Ancestor(SessionId(1)),
        Origin::Occurrence,
        Origin::Signal,
    ]
}
fn part() -> PartKey {
    PartKey {
        needs: labels(&["x", "y"]),
        est_bucket: 5,
    }
}
fn next_key() -> NextKey {
    NextKey {
        tier: 1,
        group: -4,
        fnv: u64::MAX,
        id: 9,
        kind: 0,
    }
}
fn cands() -> Vec<Cand> {
    vec![
        Cand::Node(NodeId(9)),
        Cand::Status(NodeId(9)),
        Cand::Route(Eid(1)),
    ]
}
fn entity_refs() -> Vec<EntityRef> {
    vec![
        EntityRef::Request(Eid(1)),
        EntityRef::PQuestion(Eid(2)),
        EntityRef::UQuestion(Eid(3)),
        EntityRef::Commitment(Eid(4)),
        EntityRef::Said(Eid(5)),
        EntityRef::Decision(Eid(6)),
        EntityRef::Objection(Eid(7)),
        EntityRef::UserEntry(Eid(8)),
        EntityRef::Digest(Eid(9)),
        EntityRef::Meta(Eid(10)),
    ]
}
fn restored() -> Vec<Restored> {
    vec![
        Restored::Request(Eid(1), request(), meta()),
        Restored::PQuestion(Eid(2), pquestion(PqState::Open), meta()),
        Restored::UQuestion(
            Eid(3),
            uquestion(UqSource::Refusal { node: NodeId(9) }, UqState::Lapsed),
            meta(),
        ),
        Restored::Commitment(Eid(4), commitment(CommitmentState::Open), meta()),
        Restored::Said(Eid(5), said(SaidState::Confirmed), meta()),
        Restored::Decision(
            Eid(6),
            decision(DecisionState::Superseded { by: Eid(9) }),
            meta(),
        ),
        Restored::Objection(
            Eid(7),
            Objection {
                node: NodeId(9),
                state: ObjectionState::Resolved { by: Eid(8) },
                moved_from: None,
                deferred: None,
            },
            meta(),
        ),
        Restored::Objection(
            Eid(7),
            Objection {
                node: NodeId(9),
                state: ObjectionState::Upheld { by: Eid(8) },
                moved_from: None,
                deferred: None,
            },
            meta(),
        ),
        Restored::UserEntry(Eid(8), user_entry(Routing::NoAction), meta()),
        Restored::Digest(
            Eid(9),
            DigestEntry {
                item: DigestItem::Lapsed { question: Eid(3) },
                to: PrincipalId(2),
                state: DigestState::Shown { by: Eid(12) },
            },
            meta(),
        ),
        Restored::Meta(Eid(10), meta()),
    ]
}

/// A state with an entry in every map, the maps that `serde_pairs` encodes included.
fn full_state() -> State {
    let mut s = State {
        last_now: Time(1_000),
        anomalies: 0,
        ..State::default()
    };
    s.counters.events = 12;
    for (i, (role, life)) in roles()
        .into_iter()
        .zip(lives().into_iter().cycle())
        .enumerate()
    {
        let id = NodeId(u64::try_from(i).unwrap() + 8);
        s.nodes
            .insert(id, node(role, life, Origin::Agent(SessionId(1))));
    }
    s.nodes
        .insert(NodeId(0), node(Role::Plain, Life::Open, Origin::Reserved));
    s.names.insert((NodeId(0), name("a")), NodeId(8));
    s.aliases.insert((NodeId(0), name("old")), NodeId(9));
    s.entries.insert(Eid(1), meta());
    s.holds.insert(tok(), hold());
    s.requests.insert(Eid(2), request());
    s.pquestions.insert(Eid(3), pquestion(PqState::Expired));
    s.aquestions.insert(
        Eid(4),
        AQuestion {
            node: NodeId(9),
            asker: SessionId(1),
            state: AqState::Answered { by: Eid(6) },
        },
    );
    s.uquestions.insert(
        Eid(5),
        uquestion(
            UqSource::Agent {
                session: SessionId(1),
                digest: digest(),
            },
            UqState::Pending,
        ),
    );
    s.commitments.insert(
        Eid(6),
        commitment(CommitmentState::Fulfilled { by: Eid(7) }),
    );
    s.saids.insert(Eid(7), said(SaidState::Unconfirmed));
    s.decisions.insert(Eid(8), decision(DecisionState::Active));
    s.objections.insert(
        Eid(9),
        Objection {
            node: NodeId(9),
            state: ObjectionState::Open,
            moved_from: Some(NodeId(8)),
            deferred: Some(Eid(10)),
        },
    );
    s.user_entries
        .insert(Eid(10), user_entry(Routing::Unrouted));
    s.digest.insert(
        Eid(11),
        DigestEntry {
            item: DigestItem::RefusalDrop {
                node: NodeId(9),
                question: Eid(5),
            },
            to: PrincipalId(0),
            state: DigestState::Pending,
        },
    );
    s.notes.insert(Eid(12), note());
    s.approvals.insert((NodeId(9), Version(2)), 2);
    s.retracted.insert(BasisTarget::Node(NodeId(100)));
    s.retracted.insert(BasisTarget::Entry(Eid(57)));
    s.refusals.insert(
        (NodeId(9), class("worker")),
        Refusal {
            quorum: 3,
            by: BTreeMap::from([(
                SessionId(1),
                RefusalEntry {
                    backoff_until: Time(10),
                    expires_at: Time(20),
                },
            )]),
        },
    );
    s.classes.insert(
        class("worker"),
        ClassActivity {
            caps_seen: BTreeMap::from([(labels(&["x"]), Time(30)), (labels(&[]), Time(31))]),
        },
    );
    s.signals.insert(signal_key(), NodeId(13));
    s.pauses.insert(
        NodeId(0),
        Pause {
            until: Time(40),
            last_renewed: Time(30),
            ttl: Dur(10),
            by: SessionId(1),
            class: class("worker"),
        },
    );
    s.sessions.insert(SessionId(1), session());
    s.principal_ids.insert(pk("user"), PrincipalId(0));
    s.principal_ids
        .insert(pk("mail:alice@corp.example"), PrincipalId(1));
    s.ops.insert(
        (class("worker"), OpId::new("op-1 ~").unwrap()),
        OpRecord {
            digest: digest(),
            session: SessionId(1),
            expires_at: Time(99),
        },
    );
    s.stats.days.insert(
        3,
        DayStats {
            demand_min: 1,
            supply_ms: 2,
            lost_ms: 3,
            next_calls: 4,
            next_empty: 5,
            next_empty_for_you: 6,
            next_callers: [Some(SessionId(1)), Some(SessionId(2)), None],
            flagged_end: 7,
        },
    );
    s.stats.done_minutes = vec![3, 1, 2];
    s.stats.demand_counted.insert(NodeId(9));
    s.stats.profiles.insert(
        (labels(&["x"]), 6),
        ProfileSeen {
            max_horizon: Some(60),
            last_seen: Time(8),
        },
    );
    s.stats.profiles.insert(
        (labels(&[]), 33),
        ProfileSeen {
            max_horizon: None,
            last_seen: Time(9),
        },
    );
    s.stats.starts.insert(29_000_000, 2);
    for c in cands() {
        s.since.insert(c, Time(5));
    }
    s.user_msgs.insert(PrincipalId(0), vec![Time(1), Time(2)]);
    for e in entity_refs() {
        s.leaving.insert(e, Time(700));
    }
    s.digest_left.insert(PrincipalId(0), 4);
    s.ix = full_ix();
    s
}

fn full_ix() -> Ix {
    let mut ix = Ix::default();
    ix.node.insert(
        NodeId(9),
        NodeIx {
            flags: FlagSet::EMPTY.with(Flag::BasisChanged),
            depth: 1,
            children: 2,
            open_agent_children: 1,
            open_children: 1,
            held_children: 1,
            flagged_children: 1,
            live: 1,
            subtree_live: 2,
            sub: Subtree {
                open: 1,
                done: 2,
                failed: 3,
                other: 4,
                flagged: 5,
            },
        },
    );
    ix.children.insert(NodeId(0), BTreeSet::from([NodeId(9)]));
    ix.flagged_children
        .insert(NodeId(0), BTreeSet::from([NodeId(9)]));
    ix.attended_children
        .insert(NodeId(0), BTreeSet::from([NodeId(9)]));
    ix.unheld_flagged_children
        .insert(NodeId(0), BTreeSet::from([NodeId(9)]));
    ix.unheld_flagged_by_part
        .insert(part(), BTreeSet::from([NodeId(9)]));
    ix.keys_open.insert(key("chat:x/1"), NodeId(9));
    ix.keys_all
        .insert(key("chat:x/1"), BTreeSet::from([NodeId(9), NodeId(3)]));
    ix.origins.insert(key("chat:x/1/1"), Eid(2));
    ix.unsettled_requests.insert(Eid(2));
    ix.requests_by
        .insert(PrincipalId(0), BTreeSet::from([Eid(2)]));
    ix.unsettled_by
        .insert(PrincipalId(0), BTreeSet::from([Eid(2)]));
    ix.people.insert((NodeId(9), Seq(3)), Eid(2));
    ix.items.insert(
        NodeId(9),
        NodeItems {
            requests: BTreeSet::from([Eid(2)]),
            pquestions: BTreeSet::from([Eid(3)]),
            aquestions: BTreeSet::from([Eid(4)]),
            uquestions: BTreeSet::from([Eid(5)]),
            commitments: BTreeSet::from([Eid(6)]),
            saids: BTreeSet::from([Eid(7)]),
            decisions: BTreeSet::from([Eid(8)]),
            objections: BTreeSet::from([Eid(9)]),
            unsettled: BTreeSet::from([Eid(2)]),
            requesters_by_age: BTreeSet::from([(Eid(2), PrincipalId(0))]),
            unanswered: BTreeMap::from([(key("chat:x/1"), BTreeSet::from([Eid(2)]))]),
            owed_reply: BTreeSet::from([Eid(2)]),
            owed_decision: BTreeSet::from([Eid(22)]),
            open_pq: BTreeMap::from([(key("chat:x/1"), BTreeSet::from([Eid(3)]))]),
            open_commitments: BTreeSet::from([Eid(6)]),
            open_saids: BTreeSet::from([Eid(7)]),
            unconfirmed: BTreeSet::from([Eid(7)]),
            open_objections: BTreeSet::from([Eid(9)]),
            active_decisions: BTreeSet::from([Eid(8)]),
            stale_basis: BTreeSet::from([Eid(8)]),
        },
    );
    ix.settling.insert(Eid(2), BTreeSet::from([Eid(8)]));
    ix.clarifications.insert(Eid(2), BTreeSet::from([Eid(10)]));
    ix.flagged.insert(NodeId(9));
    ix.flagged_active.insert(NodeId(9));
    ix.next.of.insert(
        Cand::Node(NodeId(9)),
        CandAt {
            since: Time(5),
            part: part(),
            key: next_key(),
        },
    );
    ix.next.parts.insert(part(), BTreeSet::from([next_key()]));
    ix.next.held.insert(
        Cand::Status(NodeId(10)),
        Held {
            depends: true,
            paused_by: Some(NodeId(0)),
            before_slot: true,
        },
    );
    ix.next.nodes = 2;
    ix.next.needs_trie = NeedsTrie {
        buckets: BTreeSet::from([0]),
        next: BTreeMap::from([(
            label("x"),
            NeedsTrie {
                buckets: BTreeSet::from([5]),
                next: BTreeMap::new(),
            },
        )]),
    };
    ix.series_open
        .insert(NodeId(9), BTreeSet::from([NodeId(10)]));
    ix.unrouted.insert(Eid(10));
    ix.pending_uq_by.insert(
        PrincipalId(2),
        BTreeSet::from([UqRank {
            neg_priority: -3,
            due: Time(77),
            q: Eid(5),
        }]),
    );
    ix.refusal_question.insert(NodeId(9), Eid(5));
    ix.pending_digest_by
        .insert(PrincipalId(0), BTreeSet::from([Eid(11)]));
    ix.waiters
        .insert(WaitKey::Node(NodeId(10)), BTreeSet::from([NodeId(9)]));
    ix.waiters
        .insert(WaitKey::Question(Eid(4)), BTreeSet::from([NodeId(9)]));
    ix.dependents
        .insert(NodeId(10), BTreeSet::from([NodeId(9)]));
    ix.backlinks
        .insert(NodeId(3), BTreeSet::from([(LinkRel::Continues, NodeId(9))]));
    ix.cited_by
        .insert(BasisTarget::Node(NodeId(100)), BTreeSet::from([Eid(8)]));
    ix.cited_by
        .insert(BasisTarget::Entry(Eid(57)), BTreeSet::from([Eid(8)]));
    ix.marked_by_question
        .insert(Eid(4), BTreeSet::from([NodeId(9)]));
    ix.holds_by_session
        .insert(SessionId(1), BTreeSet::from([tok()]));
    ix.open_saids_by
        .insert(SessionId(3), BTreeSet::from([Eid(7)]));
    ix.unrouted_by
        .insert(SessionId(1), BTreeSet::from([Eid(10)]));
    ix.refused
        .insert(NodeId(9), BTreeSet::from([class("worker")]));
    ix.live_sessions.insert(SessionId(1));
    ix.live_by_class.insert(class("worker"), 1);
    ix.ops_expiry
        .insert((Time(99), class("worker"), OpId::new("op-1 ~").unwrap()));
    ix.notes_until.insert((Time(50), Eid(12)));
    ix.leaving_by_time
        .insert((Time(700), EntityRef::Said(Eid(5))));
    ix.inbox.insert(SessionId(1), BTreeSet::from([Eid(12)]));
    ix.pending_holder
        .insert(NodeId(9), BTreeSet::from([Eid(12)]));
    ix.branch_notes.insert(NodeId(0), BTreeSet::from([Eid(12)]));
    ix.user_notes
        .insert(PrincipalId(1), BTreeSet::from([(Time(50), Eid(12))]));
    ix.principal_key
        .insert(PrincipalId(1), pk("mail:alice@corp.example"));
    ix.person_of.insert(PrincipalId(1), NodeId(14));
    ix.aliases_of
        .insert(NodeId(14), BTreeSet::from([PrincipalId(1)]));
    ix.profiles_by_time.insert((Time(8), (labels(&["x"]), 6)));
    ix.class_ends.insert(
        class("worker"),
        BTreeSet::from([(Time(30), labels(&["x"]))]),
    );
    for (kind, owner) in deadline_kinds()
        .into_iter()
        .zip(owners().into_iter().cycle())
    {
        ix.deadlines.insert(DeadlineKey {
            at: Time(70),
            kind,
            owner: owner.clone(),
        });
        ix.deadline_at.insert((kind, owner), Time(70));
    }
    ix
}

// ------------------------------------------------------------------------------------------------
// Commands

fn create_args() -> CreateArgs {
    CreateArgs {
        path: path(&["summary-gap"]),
        keys: vec![nkey("chat:team/thread-1")],
        kind: Some(label("task")),
        title: text("title"),
        body: Some(text("body")),
        digest: Some(text("digest")),
        fields: fields(),
        tags: labels(&["offer"]),
        capacity: Some(1),
        needs: Some(labels(&[])),
        est: Some(30),
        priority: Some(-1),
        due: Some(When::In(Dur(60_000))),
        depends_on: vec![NodeRef::Id(NodeId(3))],
        owners: vec![nref()],
        repeat: Some(repeat_arg()),
        agreements: Some(text("agreements")),
        request: Some(RequestArg {
            text: text("why?"),
            origin: Some(nkey("chat:team/thread-1/1001")),
            where_: None,
            requester: text("alex"),
        }),
        take: Some(false),
    }
}

fn patch(
    kind: Change<Label>,
    est: Change<u32>,
    due: Change<When>,
    rep: Change<RepeatArg>,
) -> Patch {
    Patch {
        title: Some(text("t")),
        body: None,
        digest: Some(text("d")),
        fields: Some(fields()),
        files: Some(BTreeMap::new()),
        status: None,
        agreements: Some(text("")),
        kind,
        tags_add: labels(&["offer"]),
        tags_remove: labels(&["stale"]),
        capacity: Some(2),
        priority: None,
        est,
        needs_add: labels(&["x"]),
        needs_remove: BTreeSet::new(),
        due,
        depends_on: Some(vec![]),
        owners: None,
        repeat: rep,
    }
}

/// `Patch` with every state of every `Change<T>` field.
fn patches() -> Vec<Patch> {
    vec![
        patch(Change::Keep, Change::Keep, Change::Keep, Change::Keep),
        patch(Change::Clear, Change::Clear, Change::Clear, Change::Clear),
        patch(
            Change::Set(label("bug")),
            Change::Set(15),
            Change::Set(When::At(Time(5))),
            Change::Set(repeat_arg()),
        ),
        patch(
            Change::Keep,
            Change::Set(0),
            Change::Set(When::In(Dur(1))),
            Change::Clear,
        ),
    ]
}

fn node_patch(
    kind: Change<Label>,
    est: Change<u32>,
    due: Change<Time>,
    rep: Change<Repeat>,
) -> NodePatch {
    NodePatch {
        title: Some(text("t")),
        body: Some(text("b")),
        fields: None,
        files: Some(fields()),
        status: Some(text("s")),
        digest: None,
        agreements: Some(text("a")),
        kind,
        tags: Some(labels(&["offer"])),
        capacity: None,
        priority: Some(1),
        est,
        needs: Some(BTreeSet::new()),
        due,
        depends_on: Some(BTreeSet::from([NodeId(3)])),
        owners: Some(BTreeSet::new()),
        repeat: rep,
    }
}

/// `NodePatch` with every state of every `Change<T>` field.
fn node_patches() -> Vec<NodePatch> {
    vec![
        node_patch(Change::Keep, Change::Keep, Change::Keep, Change::Keep),
        node_patch(Change::Clear, Change::Clear, Change::Clear, Change::Clear),
        node_patch(
            Change::Set(label("bug")),
            Change::Set(15),
            Change::Set(Time(5)),
            Change::Set(repeat()),
        ),
    ]
}

fn user_ask() -> UserAsk {
    UserAsk {
        to: Some(pk("principal-scheme:bob")),
        options: vec![text("drop"), text("keep")],
        recommendation: Some(text("drop")),
        default: Some(text("drop")),
        due: Some(When::At(Time(9))),
        digest: digest(),
    }
}

fn cmds() -> Vec<Cmd> {
    let mut v = vec![
        Cmd::Session(SessionCmd::Hello {
            intent: Some(text("i")),
            caps: Some(labels(&["x"])),
            horizon: Some(60),
            principal: Some(pk("mail:alice@corp.example")),
            from_user: Some(FromUserArg {
                text: text("summarise"),
                client: text("ide"),
                principal: None,
                answers: None,
                to: None,
                clarifies: None,
                reason: None,
            }),
        }),
        Cmd::Session(SessionCmd::Hello {
            intent: None,
            caps: None,
            horizon: None,
            principal: None,
            from_user: None,
        }),
        Cmd::Session(SessionCmd::Status { text: text("s") }),
        Cmd::View(ViewCmd::Node {
            targets: vec![
                nref(),
                NodeRef::Id(NodeId(12)),
                NodeRef::Path(Path::default()),
            ],
            view: Some(ViewMode::Full),
            page: Some(2),
        }),
        Cmd::View(ViewCmd::Find {
            by: FindBy::Key(key("chat:x/1")),
        }),
        Cmd::View(ViewCmd::Find {
            by: FindBy::Text(text("summary")),
        }),
        Cmd::View(ViewCmd::Feed {
            at: nref(),
            since: Some(Seq(3)),
            types: BTreeSet::from([EntryType::Said, EntryType::Late]),
            limit: Some(10),
        }),
        Cmd::View(ViewCmd::List {
            under: Some(nref()),
            kind: Some(label("task")),
            tags: labels(&["offer"]),
            status: Some(text("open")),
            fields: fields(),
            flags: FlagSet::EMPTY.with(Flag::Missed),
            closed: Some(true),
            limit: Some(500),
        }),
        Cmd::View(ViewCmd::History {
            at: nref(),
            before: Some(99),
            limit: None,
        }),
        Cmd::View(ViewCmd::Requests {
            status: Some(BoardState::InProgress),
            requester: Some(RequesterFilter::Principal(pk("mail:bob@corp.example"))),
        }),
        Cmd::View(ViewCmd::UserLog {
            since: Some(Eid(3)),
            limit: Some(5),
        }),
        Cmd::View(ViewCmd::Constitution),
        Cmd::View(ViewCmd::Agreements {
            under: Some(nref()),
            limit: Some(20),
        }),
        Cmd::View(ViewCmd::Agreements {
            under: None,
            limit: None,
        }),
        Cmd::Node(NodeCmd::Create(create_args())),
        Cmd::Node(NodeCmd::Link {
            at: nref(),
            rel: LinkRel::SupersededBy,
            to: NodeRef::Id(NodeId(4)),
        }),
        Cmd::Node(NodeCmd::Key {
            at: nref(),
            change: KeyChange::Add(nkey("issue:p#1")),
        }),
        Cmd::Node(NodeCmd::Key {
            at: nref(),
            change: KeyChange::Remove(key("issue:p#1")),
        }),
        Cmd::Node(NodeCmd::Close {
            at: nref(),
            outcome: Outcome::Other(label("parked")),
            summary: text("s"),
            verified: Some(text("v")),
            settles: Some(BTreeSet::from([Eid(2)])),
            learnings: Some(text("l")),
            token: Some(tok()),
            unsent: BTreeSet::from([Eid(7)]),
        }),
        Cmd::Node(NodeCmd::Reopen {
            at: nref(),
            reason: text("r"),
        }),
        Cmd::Node(NodeCmd::Move {
            at: nref(),
            to: path(&["work", "x"]),
        }),
        Cmd::Node(NodeCmd::Merge {
            from: nref(),
            into: NodeRef::Id(NodeId(9)),
        }),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry {
                eid: Eid(5),
                node: NodeId(9),
            },
            reason: text("r"),
        }),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::File {
                at: nref(),
                name: text("report.md"),
            },
            reason: text("r"),
        }),
        Cmd::Hold(HoldCmd::Take {
            target: nref(),
            lease: Some(Dur(0)),
        }),
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery {
                under: Some(nref()),
                kind: Some(label("task")),
                tags: labels(&["x"]),
            },
            lease: None,
        }),
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery::default(),
            lease: Some(Dur(5)),
        }),
        Cmd::Hold(HoldCmd::Checkpoint {
            token: tok(),
            done: text("d"),
            next_step: text("n"),
            intent: Some(text("i")),
            intent_done: Some(text("id")),
            files: vec![text("f")],
        }),
        Cmd::Hold(HoldCmd::Renew {
            token: tok(),
            why: text("w"),
        }),
        Cmd::Hold(HoldCmd::Check {
            target: nref(),
            token: Some(tok()),
            intent_done: None,
        }),
        Cmd::Talk(TalkCmd::FromUser {
            text: text("t"),
            client: text("c"),
            principal: Some(pk("user")),
            answers: Some(Eid(5)),
            to: Some(forum_core::RouteTo::Node(nref())),
            clarifies: Some(Eid(2)),
            reason: None,
        }),
        Cmd::Talk(TalkCmd::ToUser {
            text: text("t"),
            kind: ToUserKind::Result,
            verbatim: true,
            principal: None,
            re: BTreeSet::from([Eid(1), Eid(2)]),
            at: None,
        }),
        Cmd::Talk(TalkCmd::Route {
            entry: Eid(1),
            to: Some(forum_core::RouteTo::Nowhere),
            answers: None,
            clarifies: None,
            reason: Some(text("r")),
        }),
        Cmd::Talk(TalkCmd::Request {
            at: nref(),
            text: text("t"),
            origin: nkey("chat:x/1/1"),
            where_: Some(nkey("chat:x/1")),
            requester: text("alex"),
        }),
        Cmd::Talk(TalkCmd::Said {
            at: nref(),
            where_: None,
            text: text("t"),
            kind: SaidKind::Ack,
            corrects: Some(Eid(3)),
            seen: Some(Seq(4)),
        }),
        Cmd::Talk(TalkCmd::SaidConfirm {
            said: Eid(7),
            confirmed: text("https://chat.example/m/1"),
        }),
        Cmd::Talk(TalkCmd::Heard {
            at: nref(),
            where_: Some(nkey("mail:m@x")),
            from: text("alex"),
            text: text("t"),
            answers: Some(Eid(3)),
        }),
        Cmd::Talk(TalkCmd::Ask {
            at: nref(),
            text: text("q"),
            to: AskTo::Person {
                where_: Some(nkey("chat:x/1")),
                blocking: true,
            },
            again: true,
        }),
        Cmd::Talk(TalkCmd::Answer {
            question: Eid(4),
            node: NodeId(9),
            asker: Some(SessionId(1)),
            text: text("a"),
        }),
        Cmd::Talk(TalkCmd::Say {
            at: nref(),
            text: text("t"),
            to: Some(AddresseeArg::User),
            expects: Some(Expects::Reply),
            reply_to: Some(Eid(3)),
            until: Some(When::In(Dur(5))),
        }),
        Cmd::Talk(TalkCmd::Decide {
            at: nref(),
            text: text("d"),
            settles: BTreeSet::from([Eid(2)]),
            supersedes: Some(Eid(8)),
            basis: vec![
                BasisArg::Node {
                    at: nref(),
                    version: Some(Version(4)),
                },
                BasisArg::Entry(Eid(57)),
                BasisArg::External {
                    what: text("arXiv"),
                    checked_at: When::At(Time(1)),
                    recheck: Some(When::In(Dur(1))),
                },
            ],
            evidence: Some(Evidence::Attempt),
        }),
        Cmd::Talk(TalkCmd::Approve {
            on: nref(),
            version: Version(3),
            checked: text("c"),
        }),
        Cmd::Talk(TalkCmd::Object {
            on: nref(),
            note: text("n"),
        }),
        Cmd::Talk(TalkCmd::Resolve {
            objection: Eid(9),
            note: text("n"),
            upheld: false,
            defer: false,
        }),
        Cmd::Talk(TalkCmd::Resolve {
            objection: Eid(9),
            note: text("n"),
            upheld: false,
            defer: true,
        }),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Entry {
                eid: Eid(57),
                node: NodeId(9),
            },
            text: text("t"),
            reason: text("r"),
        }),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Node(nref()),
            text: text("t"),
            reason: text("r"),
        }),
        Cmd::Talk(TalkCmd::Wait {
            timeout: Dur(50_000),
            for_: Some(nref()),
            since: Some(Seq(2)),
        }),
        Cmd::Talk(TalkCmd::Who { at: None }),
        Cmd::Control(ControlCmd::Pause {
            scope: NodeRef::Path(Path::default()),
            reason: text("r"),
        }),
        Cmd::Control(ControlCmd::Unpause { scope: nref() }),
        Cmd::Control(ControlCmd::Health),
        Cmd::Control(ControlCmd::Constitution),
        Cmd::Control(ControlCmd::Export {
            after: Some(5),
            limit: Some(1_000),
        }),
        Cmd::Admin(AdminCmd::SetParams(Params::default())),
    ];
    for p in patches() {
        v.push(Cmd::Node(NodeCmd::Update {
            at: nref(),
            version: Version(3),
            digest_version: Some(Version(2)),
            token: None,
            patch: p,
        }));
    }
    for reason in [
        ReleaseReason::Handoff,
        ReleaseReason::Blocked,
        ReleaseReason::Cannot,
    ] {
        for waits_for in [
            None,
            Some(WaitArg::Question(Eid(3))),
            Some(WaitArg::Node(nref())),
            Some(WaitArg::Time(When::At(Time(5)))),
            Some(WaitArg::External {
                what: text("ci"),
                recheck_after: When::In(Dur(60_000)),
            }),
        ] {
            v.push(Cmd::Hold(HoldCmd::Release {
                token: tok(),
                done: text("d"),
                next_step: text("n"),
                reason,
                intent_done: (reason == ReleaseReason::Handoff).then(|| text("posted")),
                waits_for,
                learnings: None,
                unsent: BTreeSet::new(),
                // 1.12 (§10.3): a blocked release settles what a result answered.
                settles: if reason == ReleaseReason::Blocked {
                    BTreeSet::from([Eid(2), Eid(5)])
                } else {
                    BTreeSet::new()
                },
            }));
        }
    }
    for to in [
        AskTo::Agent(AgentAddressee::Session(SessionId(1))),
        AskTo::Agent(AgentAddressee::Holder(nref())),
        AskTo::Agent(AgentAddressee::Holders(nref())),
        AskTo::User(user_ask()),
    ] {
        v.push(Cmd::Talk(TalkCmd::Ask {
            at: nref(),
            text: text("q"),
            to,
            again: false,
        }));
    }
    for to in [
        AddresseeArg::Session(SessionId(1)),
        AddresseeArg::Holder(nref()),
        AddresseeArg::Holders(nref()),
        AddresseeArg::Principal(pk("slack:t01/u0dave")),
    ] {
        v.push(Cmd::Talk(TalkCmd::Say {
            at: nref(),
            text: text("t"),
            to: Some(to),
            expects: None,
            reply_to: None,
            until: None,
        }));
    }
    for requester in [RequesterFilter::Mine, RequesterFilter::All] {
        v.push(Cmd::View(ViewCmd::Requests {
            status: None,
            requester: Some(requester),
        }));
    }
    v
}

/// Numbers the variants of `Cmd` and of each tool's enum, one index each.
fn cmd_index(c: &Cmd) -> usize {
    match c {
        Cmd::Session(SessionCmd::Hello { .. }) => 0,
        Cmd::Session(SessionCmd::Status { .. }) => 1,
        Cmd::View(v) => match v {
            ViewCmd::Node { .. } => 2,
            ViewCmd::Find { .. } => 3,
            ViewCmd::Feed { .. } => 4,
            ViewCmd::List { .. } => 5,
            ViewCmd::History { .. } => 6,
            ViewCmd::Requests { .. } => 7,
            ViewCmd::UserLog { .. } => 8,
            ViewCmd::Constitution => 9,
            ViewCmd::Agreements { .. } => 48,
        },
        Cmd::Node(n) => match n {
            NodeCmd::Create(_) => 10,
            NodeCmd::Update { .. } => 11,
            NodeCmd::Link { .. } => 12,
            NodeCmd::Key { .. } => 13,
            NodeCmd::Close { .. } => 14,
            NodeCmd::Reopen { .. } => 15,
            NodeCmd::Move { .. } => 16,
            NodeCmd::Merge { .. } => 17,
            NodeCmd::Hide { .. } => 18,
        },
        Cmd::Hold(h) => match h {
            HoldCmd::Take { .. } => 19,
            HoldCmd::Next { .. } => 20,
            HoldCmd::Checkpoint { .. } => 21,
            HoldCmd::Renew { .. } => 22,
            HoldCmd::Release { .. } => 23,
            HoldCmd::Check { .. } => 24,
        },
        Cmd::Talk(t) => match t {
            TalkCmd::FromUser { .. } => 25,
            TalkCmd::ToUser { .. } => 26,
            TalkCmd::Route { .. } => 27,
            TalkCmd::Request { .. } => 28,
            TalkCmd::Said { .. } => 29,
            TalkCmd::SaidConfirm { .. } => 30,
            TalkCmd::Heard { .. } => 31,
            TalkCmd::Ask { .. } => 32,
            TalkCmd::Answer { .. } => 33,
            TalkCmd::Say { .. } => 34,
            TalkCmd::Decide { .. } => 35,
            TalkCmd::Approve { .. } => 36,
            TalkCmd::Object { .. } => 37,
            TalkCmd::Resolve { .. } => 38,
            TalkCmd::Retract { .. } => 39,
            TalkCmd::Wait { .. } => 40,
            TalkCmd::Who { .. } => 41,
        },
        Cmd::Control(c) => match c {
            ControlCmd::Pause { .. } => 42,
            ControlCmd::Unpause { .. } => 43,
            ControlCmd::Health => 44,
            ControlCmd::Constitution => 45,
            ControlCmd::Export { .. } => 46,
        },
        Cmd::Admin(AdminCmd::SetParams(_)) => 47,
    }
}
const CMD_VARIANTS: usize = 49;

// ------------------------------------------------------------------------------------------------
// Events

fn entry_bodies() -> Vec<EntryBody> {
    let mut v = vec![
        EntryBody::Note {
            to: None,
            until: None,
            recipients: BTreeSet::new(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        },
        EntryBody::Request {
            origin: Key {
                key: key("user:e1"),
                nv: NormVer(0),
            },
            conv: key("user:e1"),
            from_user: Some(Eid(1)),
            requester: Requester::User(PrincipalId(0)),
        },
        EntryBody::Answer {
            question: Eid(4),
            recipient: SessionId(1),
            until: Time(9),
        },
        EntryBody::Decision {
            settles: BTreeSet::from([Eid(2)]),
            supersedes: Some(Eid(8)),
            assumes: BTreeSet::from([Eid(3)]),
            evidence: Some(Evidence::Observation),
            basis: basis_refs(),
        },
        EntryBody::Checkpoint {
            token: tok(),
            release: Some(ReleaseReason::Cannot),
            pr_named: None,
            handed_off_until: None,
        },
        EntryBody::Checkpoint {
            token: tok(),
            release: None,
            pr_named: Some(true),
            handed_off_until: None,
        },
        EntryBody::Checkpoint {
            token: tok(),
            release: Some(ReleaseReason::Handoff),
            pr_named: Some(false),
            handed_off_until: Some(Time(9)),
        },
        EntryBody::Said {
            conv: key("chat:x/1"),
            kind: SaidKind::Correction,
            corrects: Some(Eid(3)),
            voice: true,
        },
        EntryBody::Heard {
            conv: key("chat:x/1"),
            answers: None,
        },
        EntryBody::FromUser {
            principal: PrincipalId(2),
        },
        EntryBody::ToUser {
            kind: ToUserKind::Question,
            verbatim: false,
            re: BTreeSet::from([Eid(11)]),
            principal: PrincipalId(2),
        },
        EntryBody::Approve {
            version: Version(3),
        },
        EntryBody::Object,
        EntryBody::Resolve {
            objection: Eid(9),
            upheld: false,
            deferred: false,
        },
        EntryBody::Resolve {
            objection: Eid(9),
            upheld: true,
            deferred: false,
        },
        EntryBody::Resolve {
            objection: Eid(9),
            upheld: false,
            deferred: true,
        },
        EntryBody::IntentChecked {
            intent: Eid(31),
            pr_named: true,
        },
        EntryBody::Retract {
            target: BasisTarget::Entry(Eid(57)),
        },
        EntryBody::Late {
            token: tok(),
            attempted: Box::new(Cmd::Hold(HoldCmd::Renew {
                token: tok(),
                why: text("w"),
            })),
        },
    ];
    for a in addressees() {
        v.push(EntryBody::Note {
            to: Some(a),
            until: Some(Time(9)),
            recipients: BTreeSet::from([SessionId(1)]),
            pending_holder: true,
            expects: Some(Expects::Ack),
            reply_to: Some(Eid(2)),
        });
    }
    for q in question_bodies() {
        v.push(EntryBody::Question(q));
    }
    for n in system_notes() {
        v.push(EntryBody::System(n));
    }
    v
}

fn question_bodies() -> Vec<QuestionBody> {
    let mut v = vec![
        QuestionBody::People {
            conv: key("chat:x/1"),
            blocking: false,
            said: Eid(4),
            supersedes: Some(Eid(2)),
            recheck_at: Time(8),
        },
        QuestionBody::Agent {
            to: Addressee::Holders(NodeId(0)),
            until: Time(9),
            recipients: BTreeSet::new(),
            pending_holder: false,
        },
    ];
    for source in [
        UqSource::Agent {
            session: SessionId(1),
            digest: digest(),
        },
        UqSource::Escalation { question: Eid(3) },
        UqSource::Refusal { node: NodeId(9) },
    ] {
        v.push(QuestionBody::User {
            source,
            about: None,
            to: vec![PrincipalId(0)],
            due: Time(9),
            has_default: true,
        });
    }
    v
}

fn system_notes() -> Vec<SystemNote> {
    let mut v = vec![
        SystemNote::Reopened,
        SystemNote::KeysDropped {
            keys: vec![(key("chat:x/1"), NodeId(9))],
        },
        SystemNote::MergedFrom {
            from: NodeId(9),
            last_checkpoint: Some(Eid(30)),
        },
        SystemNote::Moved {
            from: Place {
                parent: NodeId(6),
                name: name("bob"),
            },
        },
        SystemNote::Renewed { token: tok() },
        SystemNote::ScheduleEnded,
        SystemNote::Paused { until: Time(9) },
        SystemNote::Unpaused,
        SystemNote::PauseExpired,
        SystemNote::Hidden(HideTarget::Entry(Eid(5))),
        SystemNote::Hidden(HideTarget::File {
            node: NodeId(9),
            name: text("report.md"),
        }),
        SystemNote::Defaulted { question: Eid(5) },
    ];
    for item in digest_items() {
        v.push(SystemNote::Digest {
            item,
            to: PrincipalId(2),
        });
    }
    for outcome in outcomes() {
        v.push(SystemNote::Closed { outcome });
    }
    v
}

fn body_index(b: &EntryBody) -> usize {
    match b {
        EntryBody::Note { .. } => 0,
        EntryBody::Request { .. } => 1,
        EntryBody::Question(_) => 2,
        EntryBody::Answer { .. } => 3,
        EntryBody::Decision { .. } => 4,
        EntryBody::Checkpoint { .. } => 5,
        EntryBody::Said { .. } => 6,
        EntryBody::Heard { .. } => 7,
        EntryBody::FromUser { .. } => 8,
        EntryBody::ToUser { .. } => 9,
        EntryBody::Approve { .. } => 10,
        EntryBody::Object => 11,
        EntryBody::Resolve { .. } => 12,
        EntryBody::Retract { .. } => 13,
        EntryBody::System(_) => 14,
        EntryBody::Late { .. } => 15,
        EntryBody::IntentChecked { .. } => 16,
    }
}

fn system_index(n: &SystemNote) -> usize {
    match n {
        SystemNote::Digest { .. } => 0,
        SystemNote::Closed { .. } => 1,
        SystemNote::Reopened => 2,
        SystemNote::KeysDropped { .. } => 3,
        SystemNote::MergedFrom { .. } => 4,
        SystemNote::Moved { .. } => 5,
        SystemNote::Renewed { .. } => 6,
        SystemNote::ScheduleEnded => 7,
        SystemNote::Paused { .. } => 8,
        SystemNote::Unpaused => 9,
        SystemNote::PauseExpired => 10,
        SystemNote::Hidden(_) => 11,
        SystemNote::Defaulted { .. } => 12,
    }
}

fn node_init(role: Role) -> NodeInit {
    NodeInit {
        title: text("t"),
        body: text("b"),
        fields: fields(),
        status: text(""),
        digest: text("d"),
        agreements: text(""),
        kind: None,
        tags: labels(&["offer"]),
        capacity: 1,
        priority: 0,
        est: None,
        needs: labels(&["x"]),
        due: Some(Time(9)),
        depends_on: BTreeSet::from([NodeId(3)]),
        owners: BTreeSet::from([NodeId(12)]),
        keys: BTreeMap::from([(key("repeat:9:20261002T0500Z"), NormVer(0))]),
        links: BTreeSet::from([(LinkRel::ConflictsWith, NodeId(9))]),
        role,
    }
}

fn evs() -> Vec<Ev> {
    let mut v = vec![
        Ev::Genesis {
            params: Params::default(),
        },
        Ev::ParamsChanged {
            params: Params::default(),
        },
        Ev::SessionOpened {
            session: SessionId(1),
            class: class("chat-assistant"),
            caps: labels(&["x"]),
            horizon: Some(60),
            intent: Some(text("i")),
            silence_at: Time(9),
            retire_at: Time(99),
            principal: Some(PrincipalId(0)),
        },
        Ev::SessionHello {
            session: SessionId(1),
            caps: None,
            horizon: Some(5),
            intent: None,
            principal: Some(PrincipalId(3)),
        },
        Ev::PrincipalSeen {
            id: PrincipalId(3),
            key: pk("mail:carol@corp.example"),
        },
        Ev::Touched {
            session: SessionId(1),
            silence_at: Time(9),
            retire_at: Time(99),
        },
        Ev::SessionRetired {
            session: SessionId(1),
        },
        Ev::SessionStatus {
            session: SessionId(1),
            text: text("s"),
        },
        Ev::InboxRead {
            session: SessionId(1),
            notes: BTreeSet::from([Eid(12)]),
        },
        Ev::Shown {
            session: SessionId(1),
            node: NodeId(9),
            seq: Seq(3),
        },
        Ev::NodeUpdated {
            node: NodeId(9),
            patch: NodePatch::default(),
        },
        Ev::KeyAdded {
            node: NodeId(9),
            key: nkey("chat:x/1"),
        },
        Ev::KeyRemoved {
            node: NodeId(9),
            key: key("chat:x/1"),
        },
        Ev::LinkAdded {
            node: NodeId(9),
            rel: LinkRel::DerivedFrom,
            to: NodeId(3),
        },
        Ev::NodeMoved {
            node: NodeId(9),
            parent: NodeId(0),
            name: name("x"),
        },
        Ev::NodeReopened { node: NodeId(9) },
        Ev::OfferSet {
            node: NodeId(9),
            on: true,
        },
        Ev::DependsRedirected {
            node: NodeId(9),
            from: NodeId(3),
            to: None,
        },
        Ev::MarkSet {
            node: NodeId(9),
            mark: Mark::Recheck,
            question: Eid(3),
            on: true,
        },
        Ev::MarkSet {
            node: NodeId(9),
            mark: Mark::Assume,
            question: Eid(3),
            on: false,
        },
        Ev::IntentOpened {
            node: NodeId(9),
            intent: Intent {
                eid: Eid(31),
                session: SessionId(2),
                pr_named: true,
            },
        },
        Ev::IntentClosed {
            node: NodeId(9),
            end: IntentEnd::Done { by: Eid(33) },
        },
        Ev::IntentClosed {
            node: NodeId(9),
            end: IntentEnd::ResolvedByClose,
        },
        Ev::IntentMoved {
            from: NodeId(9),
            to: NodeId(10),
        },
        Ev::SeriesOutcome {
            series: NodeId(9),
            missed: true,
        },
        Ev::OccurrenceAvailable { node: NodeId(10) },
        Ev::OccurrenceDetached { node: NodeId(10) },
        Ev::SignalDetached { node: NodeId(13) },
        Ev::HoldIssued {
            token: tok(),
            session: SessionId(1),
            lease_until: Time(9),
            terms: terms(),
            l: Dur(5),
            reopened: None,
        },
        Ev::RequestAnswered {
            request: Eid(2),
            by: Eid(20),
        },
        Ev::RequestSettled {
            request: Eid(2),
            decision: Eid(18),
            answer: Eid(20),
        },
        Ev::RequestMoved {
            request: Eid(2),
            to: NodeId(10),
        },
        Ev::PqRecheckAt {
            q: Eid(3),
            at: Time(9),
        },
        Ev::PqRechecked {
            q: Eid(3),
            rechecks: 1,
            next: Time(19),
        },
        Ev::PqExpired { q: Eid(3) },
        Ev::PqEscalated {
            q: Eid(3),
            uq: Eid(5),
        },
        Ev::PqAnswered {
            q: Eid(3),
            by: Eid(9),
        },
        Ev::PqSuperseded {
            q: Eid(3),
            by: Eid(4),
        },
        Ev::PqWithdrawn {
            q: Eid(3),
            by: Eid(18),
        },
        Ev::PqAssumed {
            q: Eid(3),
            decision: Eid(8),
        },
        Ev::PqMoved {
            q: Eid(3),
            to: NodeId(10),
        },
        Ev::AqAnswered {
            q: Eid(4),
            by: Eid(6),
        },
        Ev::UqAnswered {
            q: Eid(5),
            by: Eid(6),
        },
        Ev::UqDefaulted { q: Eid(5) },
        Ev::UqReaddressed {
            q: Eid(5),
            stage: 1,
            due: Time(99),
        },
        Ev::UqLapsed { q: Eid(5) },
        Ev::UqAssumed {
            q: Eid(5),
            decision: Eid(8),
        },
        Ev::CommitmentOpened {
            c: Eid(6),
            node: NodeId(9),
            conv: key("chat:x/1"),
            followup_at: Time(9),
        },
        Ev::CommitmentRefreshed {
            c: Eid(6),
            followup_at: Time(19),
        },
        Ev::CommitmentFulfilled {
            c: Eid(6),
            by: Eid(7),
        },
        Ev::CommitmentMoved {
            c: Eid(6),
            to: NodeId(10),
        },
        Ev::SaidUnconfirmed { said: Eid(7) },
        Ev::SaidWithheld { said: Eid(7) },
        Ev::SaidMoved {
            said: Eid(7),
            to: NodeId(10),
        },
        Ev::DecisionSuperseded {
            d: Eid(8),
            by: Eid(9),
        },
        Ev::DecisionRetracted {
            d: Eid(8),
            by: Eid(9),
            node: Some(NodeId(9)),
        },
        Ev::ObjectionResolved {
            o: Eid(9),
            by: Eid(10),
            upheld: false,
        },
        Ev::ObjectionResolved {
            o: Eid(9),
            by: Eid(10),
            upheld: true,
        },
        Ev::NodeCorrected {
            node: NodeId(9),
            outcome: Outcome::Other(label("refuted")),
            by: Eid(10),
            was: Some(Outcome::Done),
        },
        Ev::CorrectionWithdrawn {
            node: NodeId(9),
            outcome: Outcome::Done,
        },
        Ev::ObjectionMoved {
            o: Eid(9),
            to: NodeId(10),
        },
        Ev::DigestShown {
            entry: Eid(11),
            by: Eid(12),
        },
        Ev::Refused {
            node: NodeId(9),
            class: class("worker"),
            session: SessionId(1),
            backoff_until: Time(10),
            expires_at: Time(20),
            quorum: 3,
        },
        Ev::RefusalExpired {
            node: NodeId(9),
            class: class("worker"),
        },
        Ev::RefusalsCleared {
            node: NodeId(9),
            by: Eid(5),
        },
        Ev::SignalOpened {
            key: signal_key(),
            node: NodeId(13),
        },
        Ev::SignalResolved { key: signal_key() },
        Ev::Paused {
            scope: NodeId(0),
            until: Time(9),
            ttl: Dur(5),
            by: SessionId(1),
            class: class("worker"),
            renewed: true,
        },
        Ev::Unpaused { scope: NodeId(0) },
        Ev::PauseExpired { scope: NodeId(0) },
        Ev::NoteBound {
            note: Eid(12),
            session: SessionId(1),
        },
        Ev::Hidden {
            target: HideTarget::Entry(Eid(5)),
            reason: text("r"),
        },
        Ev::OpRecorded {
            class: class("worker"),
            op: OpId::new("op").unwrap(),
            digest: digest(),
            session: SessionId(1),
            expires_at: Time(99),
            outcome: Ok(done(Reply::Replay)),
        },
        Ev::OpRecorded {
            class: class("worker"),
            op: OpId::new("op").unwrap(),
            digest: digest(),
            session: SessionId(1),
            expires_at: Time(99),
            outcome: Err(Error::invalid(Why::BadParams)),
        },
        Ev::Restored { items: restored() },
    ];
    for owner in owners() {
        for kind in deadline_kinds() {
            v.push(Ev::Fired {
                kind,
                owner: owner.clone(),
            });
        }
    }
    for outcome in [
        NextOutcome::Item,
        NextOutcome::Nothing,
        NextOutcome::NothingForYou,
    ] {
        v.push(Ev::NextCalled {
            session: SessionId(1),
            outcome,
            open: true,
            class_window_end: Time(9),
        });
    }
    for (i, role) in roles().into_iter().enumerate() {
        v.push(Ev::NodeCreated {
            id: NodeId(20 + u64::try_from(i).unwrap()),
            parent: NodeId(0),
            name: name("n"),
            origin: origins()[i % origins().len()],
            init: node_init(role),
        });
    }
    for p in node_patches() {
        v.push(Ev::NodeUpdated {
            node: NodeId(9),
            patch: p,
        });
    }
    for outcome in outcomes() {
        v.push(Ev::NodeClosed {
            node: NodeId(9),
            outcome,
            merged_into: Some(NodeId(10)),
            by: Some(SessionId(2)),
        });
    }
    for wait in wait_ons().into_iter().map(Some).chain([None]) {
        v.push(Ev::WaitSet {
            node: NodeId(9),
            wait,
        });
    }
    for by in [Extension::Checkpoint, Extension::Renew] {
        v.push(Ev::HoldExtended {
            token: tok(),
            lease_until: Time(9),
            by,
        });
    }
    for end in [
        HoldEnd::Released,
        HoldEnd::Expired,
        HoldEnd::EndedByClose(EndCause::Close),
        HoldEnd::EndedByClose(EndCause::Merge),
        HoldEnd::EndedByClose(EndCause::Move),
    ] {
        v.push(Ev::HoldEnded { token: tok(), end });
    }
    for (i, body) in entry_bodies().into_iter().enumerate() {
        v.push(Ev::Appended {
            eid: Eid(40 + u64::try_from(i).unwrap()),
            node: NodeId(9),
            seq: Seq(1),
            by: if i % 2 == 0 {
                Author::Server
            } else {
                Author::Session(SessionId(1))
            },
            body,
            text: text_all(),
        });
    }
    for via in [
        ConfirmVia::Explicit,
        ConfirmVia::Release,
        ConfirmVia::Close,
        ConfirmVia::Recorded,
    ] {
        v.push(Ev::SaidConfirmed {
            said: Eid(7),
            via,
            link: Some(text("l")),
        });
    }
    for routing in routings() {
        v.push(Ev::EntryRouted {
            entry: Eid(10),
            routing,
            reason: None,
        });
    }
    v
}

fn ev_index(e: &Ev) -> usize {
    match e {
        Ev::Genesis { .. } => 0,
        Ev::ParamsChanged { .. } => 1,
        Ev::Fired { .. } => 2,
        Ev::SessionOpened { .. } => 3,
        Ev::SessionHello { .. } => 4,
        Ev::PrincipalSeen { .. } => 5,
        Ev::Touched { .. } => 6,
        Ev::SessionRetired { .. } => 7,
        Ev::SessionStatus { .. } => 8,
        Ev::InboxRead { .. } => 9,
        Ev::Shown { .. } => 10,
        Ev::NextCalled { .. } => 11,
        Ev::NodeCreated { .. } => 12,
        Ev::NodeUpdated { .. } => 13,
        Ev::KeyAdded { .. } => 14,
        Ev::KeyRemoved { .. } => 15,
        Ev::LinkAdded { .. } => 16,
        Ev::NodeMoved { .. } => 17,
        Ev::NodeClosed { .. } => 18,
        Ev::NodeReopened { .. } => 19,
        Ev::OfferSet { .. } => 20,
        Ev::WaitSet { .. } => 21,
        Ev::DependsRedirected { .. } => 22,
        Ev::MarkSet { .. } => 23,
        Ev::IntentOpened { .. } => 24,
        Ev::IntentClosed { .. } => 25,
        Ev::IntentMoved { .. } => 26,
        Ev::SeriesOutcome { .. } => 27,
        Ev::OccurrenceAvailable { .. } => 28,
        Ev::OccurrenceDetached { .. } => 29,
        Ev::SignalDetached { .. } => 30,
        Ev::HoldIssued { .. } => 31,
        Ev::HoldExtended { .. } => 32,
        Ev::HoldEnded { .. } => 33,
        Ev::Appended { .. } => 34,
        Ev::RequestAnswered { .. } => 35,
        Ev::RequestSettled { .. } => 36,
        Ev::RequestMoved { .. } => 37,
        Ev::PqRecheckAt { .. } => 38,
        Ev::PqRechecked { .. } => 39,
        Ev::PqExpired { .. } => 40,
        Ev::PqEscalated { .. } => 41,
        Ev::PqAnswered { .. } => 42,
        Ev::PqSuperseded { .. } => 43,
        Ev::PqAssumed { .. } => 44,
        Ev::PqMoved { .. } => 45,
        Ev::AqAnswered { .. } => 46,
        Ev::UqAnswered { .. } => 47,
        Ev::UqDefaulted { .. } => 48,
        Ev::UqReaddressed { .. } => 49,
        Ev::UqLapsed { .. } => 50,
        Ev::UqAssumed { .. } => 51,
        Ev::CommitmentOpened { .. } => 52,
        Ev::CommitmentRefreshed { .. } => 53,
        Ev::CommitmentFulfilled { .. } => 54,
        Ev::CommitmentMoved { .. } => 55,
        Ev::SaidConfirmed { .. } => 56,
        Ev::SaidUnconfirmed { .. } => 57,
        Ev::SaidMoved { .. } => 58,
        Ev::EntryRouted { .. } => 59,
        Ev::DecisionSuperseded { .. } => 60,
        Ev::DecisionRetracted { .. } => 61,
        Ev::ObjectionResolved { .. } => 62,
        Ev::ObjectionMoved { .. } => 63,
        Ev::DigestShown { .. } => 64,
        Ev::Refused { .. } => 65,
        Ev::RefusalExpired { .. } => 66,
        Ev::RefusalsCleared { .. } => 67,
        Ev::SignalOpened { .. } => 68,
        Ev::SignalResolved { .. } => 69,
        Ev::Paused { .. } => 70,
        Ev::Unpaused { .. } => 71,
        Ev::PauseExpired { .. } => 72,
        Ev::NoteBound { .. } => 73,
        Ev::Hidden { .. } => 74,
        Ev::OpRecorded { .. } => 75,
        Ev::Restored { .. } => 76,
        Ev::NodeCorrected { .. } => 77,
        Ev::SaidWithheld { .. } => 78,
        Ev::CorrectionWithdrawn { .. } => 79,
        Ev::PqWithdrawn { .. } => 80,
    }
}
const EV_VARIANTS: usize = 81;

// ------------------------------------------------------------------------------------------------
// Replies, errors, warnings

fn done(reply: Reply) -> Done {
    Done {
        reply,
        warnings: warnings(),
        opened: Some(SessionId(4)),
        stale_session: true,
        delivered: vec![Eid(40), Eid(41)],
    }
}

fn why_this() -> WhyThis {
    WhyThis {
        tier: 0,
        flags: FlagSet::EMPTY.with(Flag::OpenIntent),
        aged: -2,
        reason: Reason::ClosedFlagged,
        no_estimate: true,
        longer: true,
        beyond_horizon: Some(forum_core::BeyondHorizon {
            count: 9,
            capped: true,
            first: NodeId(9),
            est: 60,
            horizon: 45,
        }),
    }
}

fn empty_why() -> EmptyWhy {
    EmptyWhy {
        candidates: 9,
        depends: 1,
        refused: 2,
        needs: 3,
        horizon: 4,
        paused: 5,
        own: 6,
        query: 7,
        waiting: 8,
        before_slot: 2,
        query_capped: true,
        pause_scopes: vec![NodeId(0)],
        first: FirstExcluded {
            needs: Some((NodeId(11), labels(&["support"]))),
            horizon: Some((NodeId(12), 60, 30)),
            refused: Some(NodeId(13)),
            depends: Some((NodeId(14), NodeId(15))),
            waiting: Some(NodeId(16)),
        },
    }
}

fn routed_replies() -> Vec<RoutedReply> {
    vec![
        RoutedReply::Request {
            request: Eid(2),
            node: NodeId(9),
            follows: None,
        },
        RoutedReply::Request {
            request: Eid(4),
            node: NodeId(9),
            follows: Some(Eid(2)),
        },
        RoutedReply::Answers { question: Eid(5) },
        RoutedReply::Clarifies { request: Eid(2) },
        RoutedReply::NoAction,
        RoutedReply::Unrouted(UnroutedWhy::NotPending { question: Eid(5) }),
        RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 2 }),
    ]
}

fn not_settled() -> Vec<NotSettled> {
    vec![
        NotSettled {
            request: Eid(2),
            why: NotSettledWhy::SettledBefore,
        },
        NotSettled {
            request: Eid(3),
            why: NotSettledWhy::AwaitsResult {
                requester: PrincipalId(1),
            },
        },
        NotSettled {
            request: Eid(4),
            why: NotSettledWhy::AwaitsSaid {
                node: NodeId(9),
                conv: key("chat:x/1"),
            },
        },
    ]
}

fn replies() -> Vec<Reply> {
    let mut v = vec![
        Reply::Hello {
            session: SessionId(1),
            user_entry: Some(UserEntryReply {
                entry: Eid(1),
                key: key("user:e1"),
                routed: RoutedReply::Unrouted(UnroutedWhy::NoTarget { holds: 0 }),
                principal: PrincipalId(0),
            }),
            inbox: vec![Eid(12)],
        },
        Reply::Status,
        Reply::ViewNodes {
            nodes: vec![NodeId(9)],
            view: ViewMode::Review,
            page: 0,
        },
        Reply::ViewFind {
            by: FindBy::Key(key("chat:x/1")),
        },
        Reply::ViewFeed {
            node: NodeId(9),
            after: Seq(0),
            upto: Seq(5),
            types: BTreeSet::new(),
            limit: 50,
        },
        Reply::ViewList {
            nodes: vec![NodeId(9)],
            total: Total::AtLeast(2_000),
        },
        Reply::ViewHistory {
            node: NodeId(9),
            before: None,
            limit: 50,
        },
        Reply::ViewRequests {
            status: Some(BoardState::Unrouted),
            scope: BoardScope::Principals(BTreeSet::from([PrincipalId(1)])),
        },
        Reply::ViewRequests {
            status: None,
            scope: BoardScope::All,
        },
        Reply::ViewUserLog {
            since: None,
            limit: 50,
        },
        Reply::ViewConstitution,
        Reply::Agreements {
            items: vec![
                AgreementItem {
                    node: NodeId(0),
                    approvals: 0,
                    changed: None,
                },
                AgreementItem {
                    node: NodeId(9),
                    approvals: 2,
                    changed: Some(Time(5)),
                },
            ],
            total: Total::AtLeast(2),
        },
        Reply::Created {
            id: NodeId(9),
            token: Some(tok()),
            lease_until: Some(Time(9)),
            continues: vec![NodeId(3)],
            request: Some(Eid(2)),
            user_requests: vec![Eid(4)],
        },
        Reply::Joined {
            existing: NodeId(9),
            holders: vec![SessionId(1)],
            open_question: Some(Eid(3)),
            added_keys: vec![key("chat:x/1")],
            request: None,
            user_requests: vec![Eid(4), Eid(6)],
        },
        Reply::Updated {
            version: Version(4),
            digest_version: Version(2),
        },
        Reply::Linked {
            existing: false,
            signal: Some(NodeId(13)),
        },
        Reply::KeyAdded {
            already: true,
            user_request: None,
        },
        Reply::KeyAdded {
            already: false,
            user_request: Some(Eid(4)),
        },
        Reply::KeyExisting {
            existing: NodeId(9),
            signal: None,
        },
        Reply::KeyRemoved,
        Reply::Closed {
            node: NodeId(9),
            decision: Some(Eid(8)),
            settled: vec![Eid(2)],
            not_settled: vec![
                NotSettled {
                    request: Eid(3),
                    why: NotSettledWhy::AwaitsDecision,
                },
                NotSettled {
                    request: Eid(4),
                    why: NotSettledWhy::AwaitsResult {
                        requester: PrincipalId(2),
                    },
                },
            ],
            dropped: Some(NodeId(10)),
            leave_better: vec![NodeId(8)],
        },
        Reply::Reopened {
            node: NodeId(9),
            dropped_keys: vec![key("chat:x/1")],
            offered: true,
        },
        Reply::Moved { node: NodeId(9) },
        Reply::Merged {
            from: NodeId(9),
            into: NodeId(10),
        },
        Reply::Hidden,
        Reply::Taken {
            node: NodeId(9),
            token: tok(),
            lease_until: Time(9),
            reopened: Some(Outcome::Failed),
            existing: false,
        },
        Reply::Checkpointed {
            entry: Eid(31),
            lease_until: Time(9),
            intent: Some(Eid(31)),
            intent_done: Some(Eid(29)),
        },
        Reply::Renewed {
            lease_until: Time(9),
            renews_left: 2,
        },
        Reply::Released {
            intent_done: Some(Eid(29)),
            leave_better: vec![],
            decision: Some(Eid(32)),
            settled: vec![Eid(2)],
            not_settled: not_settled(),
        },
        Reply::HoldCheck {
            node: NodeId(9),
            token_current: Some(false),
            entry: None,
            intent_done: None,
        },
        Reply::HoldCheck {
            node: NodeId(9),
            token_current: None,
            entry: Some(Eid(40)),
            intent_done: Some(Eid(31)),
        },
        Reply::FromUser {
            entry: Eid(1),
            key: key("user:e1"),
            routed: RoutedReply::Request {
                request: Eid(2),
                node: NodeId(9),
                follows: None,
            },
            principal: PrincipalId(0),
        },
        Reply::ToUser {
            entry: Eid(13),
            principal: PrincipalId(2),
            settled: vec![Eid(2)],
            shown_digest: vec![Eid(11)],
            re: vec![Eid(2)],
        },
        Reply::Request {
            request: Eid(2),
            node: NodeId(9),
            existing: true,
        },
        Reply::Said {
            said: Eid(7),
            node: NodeId(9),
            answered: vec![Eid(2), Eid(3), Eid(5)],
            settled: vec![Eid(2), Eid(3)],
            fulfilled: vec![Eid(6)],
            keyed: Some(key("chat:x/1")),
        },
        Reply::SaidConfirmed { already: false },
        Reply::Heard {
            entry: Eid(14),
            node: NodeId(9),
            answered: Some(Eid(3)),
            keyed: None,
        },
        Reply::Asked {
            question: Eid(3),
            said: Some(Eid(4)),
            node: NodeId(9),
            existing: false,
            keyed: Some(key("chat:x/1")),
        },
        Reply::Answered { entry: Eid(15) },
        Reply::Say { entry: Eid(16) },
        Reply::Decided {
            decision: Eid(8),
            settled: vec![],
            not_settled: not_settled(),
        },
        Reply::Approved {
            entry: Eid(17),
            approvals: 2,
        },
        Reply::Objected { objection: Eid(9) },
        Reply::ObjectionResolved {
            entry: None,
            corrected: None,
            already: Some(Verdict {
                verdict: Eid(11),
                upheld: false,
            }),
            disputed: None,
            withdrawn: None,
            deferred: None,
        },
        Reply::ObjectionResolved {
            entry: Some(Eid(19)),
            corrected: Some(Corrected {
                node: NodeId(9),
                was: Outcome::Done,
                outcome: Outcome::Other(label("refuted")),
            }),
            already: None,
            disputed: None,
            withdrawn: None,
            deferred: None,
        },
        Reply::ObjectionResolved {
            entry: Some(Eid(20)),
            corrected: None,
            already: None,
            disputed: Some(Disputed {
                objection: Eid(20),
                node: NodeId(9),
                disputes: Eid(9),
                earlier: Verdict {
                    verdict: Eid(19),
                    upheld: true,
                },
            }),
            withdrawn: Some(Corrected {
                node: NodeId(9),
                was: Outcome::Other(label("refuted")),
                outcome: Outcome::Done,
            }),
            deferred: None,
        },
        Reply::ObjectionResolved {
            entry: Some(Eid(21)),
            corrected: None,
            already: None,
            disputed: None,
            withdrawn: None,
            deferred: Some(Deferred {
                until: Some(Time(30)),
            }),
        },
        Reply::Retracted {
            entry: Eid(18),
            withdrawn: Some(Eid(3)),
        },
        Reply::Waited {
            delivered: vec![Eid(12)],
            news: Some((NodeId(9), Seq(5))),
        },
        Reply::Who {
            at: Some(NodeId(0)),
        },
        Reply::Paused {
            scope: NodeId(0),
            until: Time(9),
            renewed: false,
        },
        Reply::Unpaused { scope: NodeId(0) },
        Reply::Health,
        Reply::Constitution,
        Reply::Export {
            after: None,
            limit: 1_000,
        },
        Reply::ParamsSet,
        Reply::Replay,
        Reply::Next(NextReply::Hold {
            node: NodeId(9),
            token: tok(),
            lease_until: Time(9),
            why: why_this(),
            reopened: None,
        }),
        Reply::Next(NextReply::Route {
            entry: Eid(1),
            why: why_this(),
        }),
        Reply::Next(NextReply::Status {
            node: NodeId(9),
            why: why_this(),
        }),
        Reply::Next(NextReply::Review {
            node: NodeId(9),
            objection: Eid(9),
            why: why_this(),
        }),
        Reply::Next(NextReply::Empty {
            why: empty_why(),
            leave_better: vec![NodeId(8)],
        }),
    ];
    for routed in routed_replies() {
        v.push(Reply::Routed {
            entry: Eid(1),
            routed,
            existing: false,
        });
    }
    v
}

fn reply_index(r: &Reply) -> usize {
    match r {
        Reply::Hello { .. } => 0,
        Reply::Status => 1,
        Reply::ViewNodes { .. } => 2,
        Reply::ViewFind { .. } => 3,
        Reply::ViewFeed { .. } => 4,
        Reply::ViewList { .. } => 5,
        Reply::ViewHistory { .. } => 6,
        Reply::ViewRequests { .. } => 7,
        Reply::ViewUserLog { .. } => 8,
        Reply::ViewConstitution => 9,
        Reply::Created { .. } => 10,
        Reply::Joined { .. } => 11,
        Reply::Updated { .. } => 12,
        Reply::Linked { .. } => 13,
        Reply::KeyAdded { .. } => 14,
        Reply::KeyExisting { .. } => 15,
        Reply::KeyRemoved => 16,
        Reply::Closed { .. } => 17,
        Reply::Reopened { .. } => 18,
        Reply::Moved { .. } => 19,
        Reply::Merged { .. } => 20,
        Reply::Hidden => 21,
        Reply::Taken { .. } => 22,
        Reply::Next(_) => 23,
        Reply::Checkpointed { .. } => 24,
        Reply::Renewed { .. } => 25,
        Reply::Released { .. } => 26,
        Reply::HoldCheck { .. } => 27,
        Reply::FromUser { .. } => 28,
        Reply::ToUser { .. } => 29,
        Reply::Routed { .. } => 30,
        Reply::Request { .. } => 31,
        Reply::Said { .. } => 32,
        Reply::SaidConfirmed { .. } => 33,
        Reply::Heard { .. } => 34,
        Reply::Asked { .. } => 35,
        Reply::Answered { .. } => 36,
        Reply::Say { .. } => 37,
        Reply::Decided { .. } => 38,
        Reply::Approved { .. } => 39,
        Reply::Objected { .. } => 40,
        Reply::ObjectionResolved { .. } => 41,
        Reply::Retracted { .. } => 42,
        Reply::Waited { .. } => 43,
        Reply::Who { .. } => 44,
        Reply::Paused { .. } => 45,
        Reply::Unpaused { .. } => 46,
        Reply::Health => 47,
        Reply::Constitution => 48,
        Reply::Export { .. } => 49,
        Reply::ParamsSet => 50,
        Reply::Replay => 51,
        Reply::Agreements { .. } => 52,
    }
}
const REPLY_VARIANTS: usize = 53;

fn warnings() -> Vec<Warning> {
    let details = vec![
        (
            WarningCode::Moved,
            WarningDetail::Moved {
                entries: vec![Eid(2)],
                total: Total::Exact(1),
            },
        ),
        (
            WarningCode::UnsettledRequests,
            WarningDetail::UnsettledRequests {
                requests: vec![Eid(2)],
            },
        ),
        (
            WarningCode::OpenCommitments,
            WarningDetail::OpenCommitments {
                commitments: vec![Eid(6)],
            },
        ),
        (
            WarningCode::OpenObjections,
            WarningDetail::OpenObjections {
                objections: vec![Eid(9)],
            },
        ),
        (
            WarningCode::Agreement,
            WarningDetail::Agreement {
                sources: vec![NodeId(0)],
                approvals: 1,
                content_version: Version(2),
            },
        ),
        (
            WarningCode::ConversationHeld,
            WarningDetail::ConversationHeld {
                holders: vec![SessionId(2)],
            },
        ),
        (
            WarningCode::HeldByOther,
            WarningDetail::HeldByOther {
                holders: vec![SessionId(2)],
                left: vec![],
            },
        ),
        (
            WarningCode::NoHold,
            WarningDetail::NoHold {
                why: NoHoldWhy::HoldsLimit,
            },
        ),
        (
            WarningCode::NoHold,
            WarningDetail::NoHold {
                why: NoHoldWhy::Paused,
            },
        ),
        (
            WarningCode::NoHold,
            WarningDetail::NoHold {
                why: NoHoldWhy::NoCapacity,
            },
        ),
        (
            WarningCode::UncoveredSlot,
            WarningDetail::UncoveredSlot {
                covered_days: 0,
                days: 28,
                usual_hours: vec![9, 10, 14],
                first_start: Some(Time(9)),
            },
        ),
        (
            WarningCode::UncoveredSlot,
            WarningDetail::UncoveredSlot {
                covered_days: 0,
                days: 28,
                usual_hours: Vec::new(),
                first_start: None,
            },
        ),
        (
            WarningCode::DuplicateKey,
            WarningDetail::DuplicateKey {
                keys: vec![(key("chat:x/1"), NodeId(9))],
            },
        ),
        (
            WarningCode::SecretMasked,
            WarningDetail::SecretMasked { fields: 2 },
        ),
        (
            WarningCode::UserSaturated,
            WarningDetail::UserSaturated {
                principal: PrincipalId(2),
                pending: 21,
            },
        ),
        (
            WarningCode::SessionOpened,
            WarningDetail::SessionOpened {
                session: SessionId(4),
            },
        ),
        (
            WarningCode::OpenIntent,
            WarningDetail::OpenIntent { intent: Eid(31) },
        ),
        (
            WarningCode::ExpectsIgnored,
            WarningDetail::ExpectsIgnored {
                expects: Expects::Reply,
            },
        ),
        (
            WarningCode::SeriesStopped,
            WarningDetail::SeriesStopped {
                occurrence: NodeId(10),
            },
        ),
        (
            WarningCode::SelfBasisDropped,
            WarningDetail::SelfBasisDropped { dropped: 2 },
        ),
        (
            WarningCode::SupersededBasisDropped,
            WarningDetail::SupersededBasisDropped {
                decision: Eid(8),
                dropped: 1,
            },
        ),
        (
            WarningCode::ResultAnsweredNothing,
            WarningDetail::ResultAnsweredNothing {
                requests: vec![Eid(2)],
            },
        ),
        (
            WarningCode::Waiting,
            WarningDetail::Waiting {
                on: WaitOn::External(Time(5)),
                set_at: Time(4),
                release: Some(Eid(3)),
            },
        ),
        (
            WarningCode::UnnamedUser,
            WarningDetail::UnnamedUser { entry: Eid(7) },
        ),
        (
            WarningCode::IgnoredArguments,
            WarningDetail::AtIgnored { node: NodeId(9) },
        ),
        (
            WarningCode::NoHumanInReach,
            WarningDetail::NoHumanInReach {
                due: Some(Time(9)),
                default: true,
            },
        ),
        (
            WarningCode::ClientContext,
            WarningDetail::ClientContext { entry: Eid(7) },
        ),
        (
            WarningCode::UnmetNeeds,
            WarningDetail::UnmetNeeds {
                needs: vec![label("lean")],
                take: true,
            },
        ),
        (
            WarningCode::NoBasis,
            WarningDetail::NoBasis {
                node: NodeId(9),
                decision: Eid(3),
            },
        ),
        (
            WarningCode::FollowUp,
            WarningDetail::FollowUp {
                entry: Eid(71),
                request: Eid(62),
                node: NodeId(49),
                requester: PrincipalId(2),
            },
        ),
        (
            WarningCode::Paused,
            WarningDetail::Paused {
                scope: NodeId(8),
                until: Time(9),
            },
        ),
        (
            WarningCode::NoHumanInReach,
            WarningDetail::NobodyServes {
                question: Eid(16),
                chain: vec![PrincipalId(2), PrincipalId(0)],
            },
        ),
        (
            WarningCode::NoRecipient,
            WarningDetail::NoRecipient {
                node: NodeId(38),
                question: true,
            },
        ),
        (
            WarningCode::AddresseeGone,
            WarningDetail::AddresseeGone {
                session: SessionId(25),
                since: Time(9),
            },
        ),
        (
            WarningCode::AnsweredNothing,
            WarningDetail::AnsweredNothing {
                node: NodeId(44),
                conv: key("chat:x/1"),
            },
        ),
    ];
    details
        .into_iter()
        .map(|(code, detail)| Warning {
            code,
            id: Some(NodeId(9)),
            detail,
        })
        .collect()
}

fn warning_index(w: &Warning) -> usize {
    match w.detail {
        WarningDetail::Moved { .. } => 0,
        WarningDetail::UnsettledRequests { .. } => 1,
        WarningDetail::OpenCommitments { .. } => 2,
        WarningDetail::OpenObjections { .. } => 3,
        WarningDetail::Agreement { .. } => 4,
        WarningDetail::ConversationHeld { .. } => 5,
        WarningDetail::HeldByOther { .. } => 6,
        WarningDetail::NoHold { .. } => 7,
        WarningDetail::UncoveredSlot { .. } => 8,
        WarningDetail::DuplicateKey { .. } => 9,
        WarningDetail::SecretMasked { .. } => 10,
        WarningDetail::UserSaturated { .. } => 11,
        WarningDetail::SessionOpened { .. } => 12,
        WarningDetail::OpenIntent { .. } => 13,
        WarningDetail::ExpectsIgnored { .. } => 14,
        WarningDetail::SeriesStopped { .. } => 15,
        WarningDetail::SelfBasisDropped { .. } => 16,
        WarningDetail::SupersededBasisDropped { .. } => 17,
        WarningDetail::ResultAnsweredNothing { .. } => 18,
        WarningDetail::Waiting { .. } => 19,
        WarningDetail::UnnamedUser { .. } => 20,
        WarningDetail::AtIgnored { .. } => 21,
        WarningDetail::NoHumanInReach { .. } => 22,
        WarningDetail::ClientContext { .. } => 23,
        WarningDetail::UnmetNeeds { .. } => 24,
        WarningDetail::NoBasis { .. } => 25,
        WarningDetail::Paused { .. } => 26,
        WarningDetail::FollowUp { .. } => 27,
        WarningDetail::NobodyServes { .. } => 28,
        WarningDetail::NoRecipient { .. } => 29,
        WarningDetail::AddresseeGone { .. } => 30,
        WarningDetail::AnsweredNothing { .. } => 31,
    }
}

fn whys() -> Vec<Why> {
    let mut v = vec![
        Why::NameTaken,
        Why::ReservedNode,
        Why::ReservedKey,
        Why::AlreadyClosed,
        Why::NotClosed,
        Why::OccurrenceReopen,
        Why::OccurrenceNotYetAvailable,
        Why::SelfLink,
        Why::SelfMerge,
        Why::MergeFromClosed,
        Why::MergeIntoOwnOccurrence,
        Why::IntentOpen,
        Why::NoIntent,
        Why::NoConversationKey,
        Why::AmbiguousConversationKey,
        Why::NoRequestOrigin,
        Why::ScheduleNeverFires,
        Why::BadTimeZone,
        Why::RunWithinZero,
        Why::DigestTooLong,
        Why::MissingDigestVersion,
        Why::SaidKindQuestion,
        Why::NotPeopleQuestion,
        Why::NotAgentQuestion,
        Why::NotUserQuestion,
        Why::NotUserEntry,
        Why::NotReportable,
        Why::DecisionNotActive,
        Why::RoleConflict,
        Why::ServerOutcome,
        Why::ClassMismatch,
        Why::VersionAhead,
        Why::AmbiguousPrincipal,
        Why::BadParams,
        Why::CheckedAtAhead,
        Why::NoPrincipal,
    ];
    for f in [
        Field::Title,
        Field::Summary,
        Field::NextStep,
        Field::Text,
        Field::Reason,
    ] {
        v.push(Why::EmptyText(f));
    }
    v
}

fn why_index(w: &Why) -> usize {
    match w {
        Why::NameTaken => 0,
        Why::ReservedNode => 1,
        Why::ReservedKey => 2,
        Why::AlreadyClosed => 3,
        Why::NotClosed => 4,
        Why::OccurrenceReopen => 5,
        Why::OccurrenceNotYetAvailable => 6,
        Why::SelfLink => 7,
        Why::SelfMerge => 8,
        Why::MergeFromClosed => 9,
        Why::IntentOpen => 10,
        Why::NoIntent => 11,
        Why::NoConversationKey => 12,
        Why::AmbiguousConversationKey => 13,
        Why::NoRequestOrigin => 14,
        Why::ScheduleNeverFires => 15,
        Why::BadTimeZone => 16,
        Why::RunWithinZero => 17,
        Why::EmptyText(_) => 18,
        Why::DigestTooLong => 19,
        Why::MissingDigestVersion => 20,
        Why::SaidKindQuestion => 21,
        Why::NotPeopleQuestion => 22,
        Why::NotAgentQuestion => 23,
        Why::NotUserQuestion => 24,
        Why::NotUserEntry => 25,
        Why::NotReportable => 26,
        Why::DecisionNotActive => 27,
        Why::RoleConflict => 28,
        Why::ServerOutcome => 29,
        Why::ClassMismatch => 30,
        Why::VersionAhead => 31,
        Why::AmbiguousPrincipal => 32,
        Why::BadParams => 33,
        Why::CheckedAtAhead => 34,
        Why::MergeIntoOwnOccurrence => 35,
        Why::NoPrincipal => 36,
    }
}

fn errors() -> Vec<Error> {
    let mut v: Vec<Error> = vec![
        Error::of(Hint::Full {
            node: NodeId(9),
            capacity: 1,
            live: 1,
        }),
        Error::of(Hint::HoldsLimit { holds: vec![tok()] }),
        Error::of(Hint::Paused {
            scope: NodeId(0),
            until: Time(9),
        }),
        Error::of(Hint::StaleToken {
            token: tok(),
            late: Eid(30),
        }),
        Error::of(Hint::StaleVersion {
            current: Version(3),
            digest_current: Version(2),
        }),
        Error::of(Hint::Cycle {
            path: vec![NodeId(9), NodeId(10), NodeId(9)],
        }),
        Error::of(Hint::Closed {
            node: NodeId(9),
            merged_into: Some(NodeId(10)),
        }),
        Error::op_id_conflict(),
    ];
    for k in [
        LimitKind::Renew,
        LimitKind::Depth,
        LimitKind::Children,
        LimitKind::WritesPerHour,
        LimitKind::Busy,
        LimitKind::TextTooLong,
    ] {
        v.push(Error::limit(k));
    }
    for w in whys() {
        v.push(Error::invalid(w));
    }
    for w in [
        What::Node,
        What::Session,
        What::Entry,
        What::Request,
        What::Question,
        What::Said,
        What::Decision,
        What::Objection,
        What::Pause,
        What::Key,
        What::UserEntry,
        What::Addressee,
    ] {
        v.push(Error::not_found(w));
    }
    v
}

fn hint_index(e: &Error) -> usize {
    match e.hint {
        Hint::Full { .. } => 0,
        Hint::HoldsLimit { .. } => 1,
        Hint::Paused { .. } => 2,
        Hint::StaleToken { .. } => 3,
        Hint::StaleVersion { .. } => 4,
        Hint::Cycle { .. } => 5,
        Hint::Closed { .. } => 6,
        Hint::OpIdConflict => 7,
        Hint::Limit(_) => 8,
        Hint::Invalid(_) => 9,
        Hint::NotFound(_) => 10,
    }
}

// ------------------------------------------------------------------------------------------------
// Query, health, and guide types

fn pause_view() -> PauseView {
    PauseView {
        scope: NodeId(0),
        by: SessionId(1),
        class: class("worker"),
        until: Time(9),
    }
}

fn brief_items() -> Vec<BriefItem> {
    let mut v = vec![
        BriefItem::Intent(Intent {
            eid: Eid(31),
            session: SessionId(2),
            pr_named: false,
        }),
        BriefItem::Said(Eid(7)),
        BriefItem::Commitment(Eid(6)),
        BriefItem::Decision(Eid(8)),
        BriefItem::Agreement { source: NodeId(0) },
        BriefItem::LateAnswer {
            question: Eid(3),
            answer: Eid(9),
            assumption: Some(Eid(8)),
        },
        BriefItem::Request(Eid(2)),
        BriefItem::Clarification(Eid(10)),
        BriefItem::Objection(Eid(9)),
        BriefItem::Digest,
        BriefItem::Checkpoint(Eid(30)),
        BriefItem::ExpiredHold(ExpiredHold {
            token: tok(),
            session: SessionId(2),
            at: Time(8),
        }),
        BriefItem::Entry(Eid(1)),
    ];
    for f in Flag::ALL {
        v.push(BriefItem::Flag(f));
    }
    for state in [
        UqState::Pending,
        UqState::Answered { by: Eid(1) },
        UqState::Defaulted,
        UqState::Lapsed,
    ] {
        v.push(BriefItem::UserQuestion {
            question: Eid(5),
            state,
        });
    }
    v
}

fn brief_index(b: &BriefItem) -> usize {
    match b {
        BriefItem::Flag(_) => 0,
        BriefItem::Intent(_) => 1,
        BriefItem::Said(_) => 2,
        BriefItem::Commitment(_) => 3,
        BriefItem::Decision(_) => 4,
        BriefItem::Agreement { .. } => 5,
        BriefItem::LateAnswer { .. } => 6,
        BriefItem::UserQuestion { .. } => 7,
        BriefItem::Request(_) => 8,
        BriefItem::Clarification(_) => 9,
        BriefItem::Objection(_) => 10,
        BriefItem::Digest => 11,
        BriefItem::Checkpoint(_) => 12,
        BriefItem::Entry(_) => 13,
        BriefItem::ExpiredHold(_) => 14,
    }
}

fn health_reasons() -> Vec<HealthReason> {
    vec![
        HealthReason::Paused {
            level: Level::Red,
            count: 1,
            top: vec![PauseExclusion {
                pause: pause_view(),
                excluded: 4,
            }],
        },
        HealthReason::NoMover {
            level: Level::Yellow,
            count: 3,
            top: vec![
                MoverGap::NodeWait {
                    node: NodeId(9),
                    target: NodeId(10),
                },
                MoverGap::Dependency {
                    node: NodeId(9),
                    target: NodeId(10),
                },
                MoverGap::FlaggedUnreachable {
                    node: NodeId(9),
                    flags: FlagSet(0x1fff),
                },
            ],
        },
        HealthReason::Unreachable {
            level: Level::Red,
            count: 2,
            by_needs: 1,
            by_horizon: 1,
            top: vec![
                (NodeId(9), UnreachableBy::Needs),
                (NodeId(10), UnreachableBy::Horizon),
                (NodeId(11), UnreachableBy::Yours { horizon: 45 }),
            ],
        },
        HealthReason::FlagsPilingUp {
            start: 50,
            now: 200,
        },
        HealthReason::Overloaded {
            level: Level::Yellow,
            demand_min: 900,
            supply_min: 300,
            idle_permille: 10,
            oldest: Some(Cand::Route(Eid(1))),
        },
        HealthReason::WriteSurge {
            class: class("worker"),
            last_hour: 3_000,
            baseline: 100,
        },
        HealthReason::Underused { idle_permille: 900 },
    ]
}

fn health_index(h: &HealthReason) -> usize {
    match h {
        HealthReason::Paused { .. } => 0,
        HealthReason::NoMover { .. } => 1,
        HealthReason::Unreachable { .. } => 2,
        HealthReason::FlagsPilingUp { .. } => 3,
        HealthReason::Overloaded { .. } => 4,
        HealthReason::WriteSurge { .. } => 5,
        HealthReason::Underused { .. } => 6,
    }
}

fn skeletons() -> Vec<Skeleton> {
    vec![
        Skeleton::Hello,
        Skeleton::LogFromUser {
            principal: PrincipalId(1),
        },
        Skeleton::ViewNode { node: NodeId(9) },
        Skeleton::Checkpoint {
            token: tok(),
            intent_done: true,
            intent: false,
        },
        Skeleton::Checkpoint {
            token: tok(),
            intent_done: false,
            intent: true,
        },
        Skeleton::CheckpointSwap { token: tok() },
        Skeleton::Release {
            token: tok(),
            wait: Some(Eid(3)),
        },
        Skeleton::Take { target: NodeId(9) },
        Skeleton::Next,
        Skeleton::CreateFor { entry: Eid(1) },
        Skeleton::Split { parent: NodeId(9) },
        Skeleton::AddOffer {
            node: NodeId(9),
            version: Version(3),
        },
        Skeleton::Close {
            node: NodeId(9),
            token: tok(),
            settles: vec![Eid(2)],
            outcome: None,
        },
        Skeleton::Close {
            node: NodeId(9),
            token: tok(),
            settles: vec![],
            outcome: Some(Outcome::Failed),
        },
        Skeleton::SayTo {
            at: NodeId(9),
            to: SessionId(2),
        },
        Skeleton::ToUser {
            principal: PrincipalId(2),
            kind: Some(ToUserKind::Result),
            re: vec![Eid(2)],
        },
        Skeleton::Said {
            at: NodeId(9),
            kind: SaidKind::Reply,
            where_: Some(key("mail:m@x")),
            where_fill: false,
        },
        Skeleton::Heard {
            at: NodeId(9),
            answers: Eid(3),
            where_: key("chat:x/1"),
        },
        Skeleton::Decide {
            at: NodeId(9),
            settles: vec![],
            supersedes: Some(Eid(8)),
        },
        Skeleton::Route {
            entry: Eid(1),
            to: RouteSk::Nowhere,
        },
        Skeleton::Route {
            entry: Eid(1),
            to: RouteSk::Fill,
        },
        Skeleton::Route {
            entry: Eid(1),
            to: RouteSk::Node(NodeId(9)),
        },
        Skeleton::Route {
            entry: Eid(1),
            to: RouteSk::Answers(Eid(5)),
        },
        Skeleton::Route {
            entry: Eid(1),
            to: RouteSk::Clarifies(Eid(62)),
        },
        Skeleton::AnswerUser {
            question: Eid(5),
            principal: None,
        },
        Skeleton::AnswerAgent { question: Eid(4) },
        Skeleton::Resolve { objection: Eid(9) },
        Skeleton::Merge {
            from: NodeId(10),
            into: NodeId(9),
        },
        Skeleton::LinkDuplicate {
            from: NodeId(10),
            to: NodeId(9),
        },
        Skeleton::AskOwners { at: NodeId(9) },
        Skeleton::Wait {
            timeout: Dur(1_000),
        },
        Skeleton::Reopen { node: NodeId(9) },
    ]
}

fn skeleton_index(s: &Skeleton) -> usize {
    match s {
        Skeleton::Hello => 0,
        Skeleton::LogFromUser { .. } => 1,
        Skeleton::ViewNode { .. } => 2,
        Skeleton::Checkpoint { .. } => 3,
        Skeleton::CheckpointSwap { .. } => 4,
        Skeleton::Release { .. } => 5,
        Skeleton::Take { .. } => 6,
        Skeleton::Next => 7,
        Skeleton::CreateFor { .. } => 8,
        Skeleton::Split { .. } => 9,
        Skeleton::AddOffer { .. } => 10,
        Skeleton::Close { .. } => 11,
        Skeleton::SayTo { .. } => 12,
        Skeleton::ToUser { .. } => 13,
        Skeleton::Said { .. } => 14,
        Skeleton::Heard { .. } => 15,
        Skeleton::Decide { .. } => 16,
        Skeleton::Route { .. } => 17,
        Skeleton::AnswerUser { .. } => 18,
        Skeleton::AnswerAgent { .. } => 19,
        Skeleton::Resolve { .. } => 20,
        Skeleton::Merge { .. } => 21,
        Skeleton::Wait { .. } => 22,
        Skeleton::LinkDuplicate { .. } => 23,
        Skeleton::AskOwners { .. } => 24,
        Skeleton::Reopen { .. } => 25,
    }
}

fn becauses() -> Vec<Because> {
    vec![
        Because::HoldEnded { late: Eid(30) },
        Because::Reread {
            version: Version(3),
            digest_version: Version(2),
        },
        Because::MergedInto { node: NodeId(10) },
        Because::CloseOpenIntent {
            by: SessionId(2),
            entry: Eid(31),
            own: false,
        },
        Because::RenewLimit,
        Because::AnswerFits { question: Eid(3) },
        Because::NoSession,
        Because::LogFirst {
            principal: PrincipalId(1),
        },
        Because::TellHolder {
            holder: SessionId(2),
        },
        Because::LeaseDue {
            lease_until: Time(9),
            at_ceiling: true,
            intent: Some(Eid(31)),
            closing: true,
        },
        Because::IntentOpen {
            by: SessionId(2),
            entry: Eid(31),
            own: false,
        },
        Because::CheckpointBy { by: Time(9) },
        Because::UncoveredSlot,
        Because::News {
            node: NodeId(9),
            count: Total::AtLeast(20),
        },
        Because::Unrouted {
            entry: Eid(1),
            related: None,
        },
        Because::Unrouted {
            entry: Eid(1),
            related: Some(NodeId(38)),
        },
        Because::AnsweredEntry {
            entry: Eid(1),
            or: Some(RouteAlt::Create),
        },
        Because::ReplyOwed {
            requests: vec![Eid(2)],
            misaddressed: Some(PrincipalId(2)),
            recorded: true,
            ready: false,
        },
        Because::DecisionOwed {
            requests: vec![Eid(2)],
            after_close: true,
            ready: false,
            recorded: true,
            held: true,
            after_release: true,
        },
        Because::RouteAnswers {
            entry: Eid(1),
            question: Eid(5),
            or: Some(RouteAlt::Node {
                node: NodeId(9),
                held: false,
            }),
        },
        Because::RouteAnswers {
            entry: Eid(1),
            question: Eid(5),
            or: Some(RouteAlt::Nowhere),
        },
        Because::RouteNode {
            entry: Eid(1),
            node: NodeId(9),
            held: true,
            or: None,
        },
        Because::RouteClarifies {
            entry: Eid(1),
            request: Eid(62),
            or: Some(RouteAlt::Node {
                node: NodeId(49),
                held: true,
            }),
        },
        Because::Linked { into: NodeId(301) },
        Because::Finished,
        Because::MergeWait {
            settles: vec![Eid(2)],
        },
        Because::MergeWait {
            settles: Vec::new(),
        },
        Because::Unproved,
        Because::Reclose {
            was: Outcome::Failed,
        },
        Because::ContainerStays { series: true },
        Because::AskWait {
            question: Eid(3),
            blocking: true,
            existing: false,
        },
        Because::AgentWait { question: Eid(4) },
        Because::JoinAnswer { question: Eid(3) },
        Because::NotOffered,
        Because::AnswersNext {
            question: Eid(5),
            principal: PrincipalId(2),
        },
        Because::StatusItem,
        Because::Review { objection: Eid(9) },
        Because::RouteItem,
        Because::Assumption {
            questions: vec![Eid(3)],
            blocking: true,
        },
        Because::StaleBasis { decision: Eid(8) },
        Because::Recheck { question: Eid(3) },
        Because::TooBig { attempts: 3 },
        Because::Duplicate { other: NodeId(10) },
        Because::Signal,
        Because::Objection {
            objection: Eid(9),
            retracted: None,
        },
        Because::Objection {
            objection: Eid(9),
            retracted: Some(Eid(8)),
        },
        Because::Inbox { count: 2 },
        Because::AgentAsked {
            by: SessionId(2),
            until: Time(9),
            question: Eid(8),
            later: true,
        },
        Because::HumanItems {
            principal: PrincipalId(2),
            questions: 1,
            digest: 3,
        },
        Because::Tidy {
            node: NodeId(9),
            flags: FlagSet::EMPTY.with(Flag::ReadyToClose),
        },
        Because::Joined,
        Because::MoreWork { at_hello: true },
        Because::StaleOffer {
            node: NodeId(9),
            cut: true,
        },
        Because::Alike {
            from: NodeId(10),
            to: NodeId(9),
            cut: false,
        },
        Because::AwaitMerge,
        Because::StaleAnswered {
            node: NodeId(9),
            question: Eid(12),
            cut: false,
        },
        Because::WorkWaiting {
            first: Cand::Node(NodeId(9)),
        },
        Because::WorkWaiting {
            first: Cand::Route(Eid(1)),
        },
    ]
}

fn because_index(b: &Because) -> usize {
    match b {
        Because::HoldEnded { .. } => 0,
        Because::Reread { .. } => 1,
        Because::MergedInto { .. } => 2,
        Because::CloseOpenIntent { .. } => 3,
        Because::RenewLimit => 4,
        Because::AnswerFits { .. } => 5,
        Because::NoSession => 6,
        Because::LogFirst { .. } => 7,
        Because::Review { .. } => 8,
        Because::TellHolder { .. } => 9,
        Because::LeaseDue { .. } => 10,
        Because::IntentOpen { .. } => 11,
        Because::CheckpointBy { .. } => 12,
        Because::News { .. } => 13,
        Because::UncoveredSlot => 14,
        Because::Unrouted { .. } => 15,
        Because::AnsweredEntry { .. } => 16,
        Because::ReplyOwed { .. } => 17,
        Because::DecisionOwed { .. } => 18,
        Because::Finished => 19,
        Because::ContainerStays { .. } => 20,
        Because::AskWait { .. } => 21,
        Because::AgentWait { .. } => 22,
        Because::JoinAnswer { .. } => 23,
        Because::NotOffered => 24,
        Because::AnswersNext { .. } => 25,
        Because::StatusItem => 26,
        Because::RouteItem => 27,
        Because::Assumption { .. } => 28,
        Because::StaleBasis { .. } => 29,
        Because::Recheck { .. } => 30,
        Because::TooBig { .. } => 31,
        Because::Duplicate { .. } => 32,
        Because::Signal => 33,
        Because::Objection { .. } => 34,
        Because::Inbox { .. } => 35,
        Because::AgentAsked { .. } => 36,
        Because::HumanItems { .. } => 37,
        Because::Tidy { .. } => 38,
        Because::Joined => 39,
        Because::MoreWork { .. } => 40,
        Because::Reclose { .. } => 41,
        Because::RouteAnswers { .. } => 42,
        Because::RouteNode { .. } => 43,
        Because::StaleOffer { .. } => 44,
        Because::Alike { .. } => 45,
        Because::AwaitMerge => 46,
        Because::StaleAnswered { .. } => 47,
        Because::WorkWaiting { .. } => 48,
        Because::RouteClarifies { .. } => 49,
        Because::Linked { .. } => 50,
        Because::MergeWait { .. } => 51,
        Because::Unproved => 52,
    }
}

const RULES: [Rule; 28] = [
    Rule::Remedy,
    Rule::LogFirst,
    Rule::TellHolder,
    Rule::Checkpoint,
    Rule::IntentDone,
    Rule::Reread,
    Rule::Route,
    Rule::Reply,
    Rule::Decide,
    Rule::Close,
    Rule::ReleaseContainer,
    Rule::AwaitMerge,
    Rule::AskWait,
    Rule::AgentWait,
    Rule::JoinAnswer,
    Rule::Offer,
    Rule::AnswersNext,
    Rule::StatusReply,
    Rule::Review,
    Rule::RouteItem,
    Rule::Unproved,
    Rule::HeldFlag,
    Rule::Inbox,
    Rule::AnswerAgent,
    Rule::HumanItems,
    Rule::Tidy,
    Rule::TakeJoined,
    Rule::MoreWork,
];

// ------------------------------------------------------------------------------------------------
// The tests

#[test]
fn primitives() {
    rt_all(&[Time(0), Time(u64::MAX)]);
    rt_all(&[Dur(0), Dur(u64::MAX)]);
    rt_all(&[When::At(Time(5)), When::In(Dur(7))]);
    rt_all(&[NodeId(u64::MAX)]);
    rt_all(&[Eid(1)]);
    rt_all(&[SessionId(4)]);
    rt_all(&[Seq(3)]);
    rt_all(&[Version(1)]);
    rt_all(&[Fence(2)]);
    rt_all(&[PrincipalId(u32::MAX)]);
    rt_all(&[NormVer(0), NormVer(1)]);
    rt(&tok());
    rt(&class("coding-agent"));
    rt(&label("team:payments"));
    rt(&name("summary-gap"));
    rt_all(&[path(&[]), path(&["a", "ü b"])]);
    rt_all(&[nref(), NodeRef::Id(NodeId(12))]);
    rt(&OpId::new("0b9e2c5e ~").unwrap());
    rt_all(&[pk("user"), pk("mail:alice@corp.example")]);
    rt(&digest());
    rt_all(&[text(""), text("ünïcode \"quoted\"")]);
    rt(&key("chat:team/thread-1/1001"));
    rt(&nkey("issue:p#1"));
    rt_all(&[
        LinkRel::Continues,
        LinkRel::DerivedFrom,
        LinkRel::ConflictsWith,
        LinkRel::Duplicate,
        LinkRel::SupersededBy,
    ]);
    rt_all(&outcomes());
    rt_all(&[
        SaidKind::Ack,
        SaidKind::Reply,
        SaidKind::Result,
        SaidKind::Correction,
        SaidKind::Question,
    ]);
    rt_all(&[ToUserKind::Ack, ToUserKind::Question, ToUserKind::Result]);
    rt_all(&[
        ReleaseReason::Handoff,
        ReleaseReason::Blocked,
        ReleaseReason::Cannot,
    ]);
    rt_all(&[ViewMode::Brief, ViewMode::Full, ViewMode::Review]);
    rt_all(&[Expects::None, Expects::Ack, Expects::Reply]);
    rt_all(&[
        Evidence::Attempt,
        Evidence::Observation,
        Evidence::Inference,
        Evidence::Reported,
    ]);
    rt_all(&[SignalKind::Duplicate, SignalKind::Conflict]);
    rt_all(&addressees());
    rt_all(&[Requester::User(PrincipalId(1)), Requester::External]);
    rt_all(&[
        EntryType::Note,
        EntryType::Request,
        EntryType::Question,
        EntryType::Answer,
        EntryType::Decision,
        EntryType::Checkpoint,
        EntryType::Said,
        EntryType::Heard,
        EntryType::FromUser,
        EntryType::ToUser,
        EntryType::Approve,
        EntryType::Object,
        EntryType::Resolve,
        EntryType::Retract,
        EntryType::System,
        EntryType::Late,
    ]);
    rt_all(&Flag::ALL);
    rt_all(&[FlagSet::EMPTY, FlagSet(0x1fff)]);
    rt_all(&[Mark::Recheck, Mark::Assume]);
    rt_all(&[Change::<u32>::Keep, Change::Clear, Change::Set(3)]);
}

#[test]
fn validated_newtypes_refuse_invalid_values() {
    assert!(serde_json::from_str::<Label>("\"Offer\"").is_err());
    assert!(serde_json::from_str::<ClassName>("\"a_b\"").is_err());
    assert!(serde_json::from_str::<Name>("\"a/b\"").is_err());
    assert!(serde_json::from_str::<Name>("\"..\"").is_err());
    assert!(serde_json::from_str::<OpId>("\"\"").is_err());
    assert!(serde_json::from_str::<PrincipalKey>("\"user:e12\"").is_err());
    assert!(serde_json::from_str::<KeyStr>("\"nokey\"").is_err());
    let bad_path = "[\"a\",\"#12\"]";
    assert!(serde_json::from_str::<Path>(bad_path).is_err());
    // As map keys too.
    assert!(serde_json::from_str::<BTreeMap<KeyStr, NormVer>>("{\"nokey\":1}").is_err());
    assert!(serde_json::from_str::<BTreeMap<KeyStr, NormVer>>("{\"chat:x\":1}").is_ok());
}

#[test]
fn params() {
    rt(&Params::default());
    rt(&Params {
        digest_max: 32,
        text_max: 64,
        conversation_schemes: BTreeSet::new(),
        ..Params::default()
    });
}

#[test]
fn state_with_every_map() {
    let s = full_state();
    rt(&s);
    rt(&State::default());
    // The maps whose keys are neither strings nor integers are lists of pairs.
    let json: serde_json::Value = serde_json::to_value(&s).unwrap();
    for field in [
        "names",
        "aliases",
        "holds",
        "approvals",
        "refusals",
        "signals",
        "ops",
        "since",
        "leaving",
    ] {
        assert!(json[field].is_array(), "{field}");
    }
    assert!(json["nodes"].is_object());
    assert!(json["ix"]["deadline_at"].is_array());
    assert!(json["ix"]["next"]["of"].is_array());
    assert!(json["stats"]["profiles"].is_array());
    assert!(json["classes"]["worker"]["caps_seen"].is_array());
}

#[test]
fn a_list_of_pairs_that_repeats_a_key_is_refused() {
    let s = State::default();
    let mut json: serde_json::Value = serde_json::to_value(&s).unwrap();
    json["names"] = serde_json::json!([[[0, "a"], 8], [[0, "a"], 9]]);
    assert!(serde_json::from_value::<State>(json).is_err());
}

#[test]
fn state_parts() {
    for role in roles() {
        for life in lives() {
            for origin in origins() {
                rt(&node(role.clone(), life.clone(), origin));
            }
        }
    }
    rt_all(&wait_ons());
    rt(&hold());
    rt(&meta());
    rt(&request());
    for state in [
        PqState::Open,
        PqState::Expired,
        PqState::Escalated { uq: Eid(5) },
        PqState::Answered { by: Eid(9) },
        PqState::Superseded { by: Eid(10) },
        PqState::Withdrawn { by: Eid(11) },
    ] {
        rt(&pquestion(state));
    }
    rt_all(&[Lapse::Expired, Lapse::Escalated]);
    rt_all(&[AqState::Open, AqState::Answered { by: Eid(3) }]);
    for state in [
        UqState::Pending,
        UqState::Answered { by: Eid(1) },
        UqState::Defaulted,
        UqState::Lapsed,
    ] {
        rt(&uquestion(UqSource::Escalation { question: Eid(3) }, state));
    }
    rt_all(&[
        commitment(CommitmentState::Open),
        commitment(CommitmentState::Fulfilled { by: Eid(7) }),
    ]);
    rt_all(&[
        said(SaidState::Pending),
        said(SaidState::Confirmed),
        said(SaidState::Unconfirmed),
        said(SaidState::Withheld),
    ]);
    rt_all(&[
        decision(DecisionState::Active),
        decision(DecisionState::Superseded { by: Eid(9) }),
        decision(DecisionState::Retracted { by: Eid(9) }),
    ]);
    rt_all(&basis_refs());
    rt_all(&[BasisTarget::Node(NodeId(1)), BasisTarget::Entry(Eid(1))]);
    for routing in routings() {
        rt(&user_entry(routing));
    }
    for item in digest_items() {
        for state in [DigestState::Pending, DigestState::Shown { by: Eid(2) }] {
            rt(&DigestEntry {
                item,
                to: PrincipalId(1),
                state,
            });
        }
    }
    rt(&note());
    rt(&session());
    rt_all(&owners());
    rt_all(&deadline_kinds());
    for owner in owners() {
        rt(&DeadlineKey {
            at: Time(5),
            kind: DeadlineKind::Lease,
            owner,
        });
    }
    rt_all(&[WaitKey::Node(NodeId(1)), WaitKey::Question(Eid(1))]);
    rt_all(&cands());
    rt_all(&entity_refs());
    covers(&restored(), 10, |r| match r {
        Restored::Request(..) => 0,
        Restored::PQuestion(..) => 1,
        Restored::UQuestion(..) => 2,
        Restored::Commitment(..) => 3,
        Restored::Said(..) => 4,
        Restored::Decision(..) => 5,
        Restored::Objection(..) => 6,
        Restored::UserEntry(..) => 7,
        Restored::Digest(..) => 8,
        Restored::Meta(..) => 9,
    });
    rt_all(&restored());
    rt(&Counters::default());
    rt(&Rollup::default());
}

/// The last value of every kind of entity that leaves memory (review E1-spec-1).
#[test]
fn departed_values() {
    let all = vec![
        Departed::Request(request()),
        Departed::PQuestion(pquestion(PqState::Open)),
        Departed::UQuestion(uquestion(
            UqSource::Escalation { question: Eid(3) },
            UqState::Pending,
        )),
        Departed::Commitment(commitment(CommitmentState::Open)),
        Departed::Said(said(SaidState::Confirmed)),
        Departed::Decision(decision(DecisionState::Active)),
        Departed::Objection(Objection {
            node: NodeId(9),
            state: ObjectionState::Open,
            moved_from: None,
            deferred: None,
        }),
        Departed::UserEntry(user_entry(Routing::NoAction)),
        Departed::Digest(DigestEntry {
            item: DigestItem::Lapsed { question: Eid(4) },
            to: PrincipalId(1),
            state: DigestState::Pending,
        }),
        Departed::Meta(meta()),
    ];
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|d| match d {
            Departed::Request(_) => 0,
            Departed::PQuestion(_) => 1,
            Departed::UQuestion(_) => 2,
            Departed::Commitment(_) => 3,
            Departed::Said(_) => 4,
            Departed::Decision(_) => 5,
            Departed::Objection(_) => 6,
            Departed::UserEntry(_) => 7,
            Departed::Digest(_) => 8,
            Departed::Meta(_) => 9,
        })
        .collect();
    assert_eq!(seen.len(), 10);
    rt_all(&all);
}

#[test]
fn commands_and_context() {
    let all = cmds();
    covers(&all, CMD_VARIANTS, cmd_index);
    rt_all(&all);
    for session in [
        SessionCtx::New,
        SessionCtx::Existing(SessionId(4)),
        SessionCtx::System,
    ] {
        rt(&Ctx {
            now: Time(9),
            class: class("worker"),
            caps: Some(labels(&["team:payments"])),
            session,
            op: Some(OpCtx {
                id: OpId::new("op").unwrap(),
                digest: digest(),
            }),
        });
    }
    rt_all(&[
        BoardState::Unrouted,
        BoardState::Answered,
        BoardState::ClosedUnanswered,
        BoardState::Decided,
        BoardState::Waiting,
        BoardState::InProgress,
    ]);
}

#[test]
fn every_change_state_round_trips_and_an_absent_change_is_keep() {
    let ps = patches();
    rt_all(&ps);
    rt_all(&node_patches());
    rt(&Patch::default());
    rt(&NodePatch::default());
    // `Keep` is the serde default: an absent field decodes as `Keep`.
    for field in ["kind", "est", "due", "repeat"] {
        let mut json: serde_json::Value = serde_json::to_value(&ps[2]).unwrap();
        json.as_object_mut().unwrap().remove(field);
        let back: Patch = serde_json::from_value(json).unwrap();
        let kept = match field {
            "kind" => back.kind == Change::Keep,
            "est" => back.est == Change::Keep,
            "due" => back.due == Change::Keep,
            _ => back.repeat == Change::Keep,
        };
        assert!(kept, "{field}");
        let mut json: serde_json::Value = serde_json::to_value(&node_patches()[2]).unwrap();
        json.as_object_mut().unwrap().remove(field);
        let back: NodePatch = serde_json::from_value(json).unwrap();
        let kept = match field {
            "kind" => back.kind == Change::Keep,
            "est" => back.est == Change::Keep,
            "due" => back.due == Change::Keep,
            _ => back.repeat == Change::Keep,
        };
        assert!(kept, "{field}");
    }
    // `Clear` and `Set` stay apart from `Keep`, where `Option<Option<T>>` would not.
    let clear = serde_json::to_string(&Change::<u32>::Clear).unwrap();
    let keep = serde_json::to_string(&Change::<u32>::Keep).unwrap();
    assert_ne!(clear, keep);
}

#[test]
fn events() {
    let all = evs();
    covers(&all, EV_VARIANTS, ev_index);
    rt_all(&all);
    let bodies = entry_bodies();
    covers(&bodies, 17, body_index);
    let notes = system_notes();
    covers(&notes, 13, system_index);
    covers(&question_bodies(), 3, |q| match q {
        QuestionBody::People { .. } => 0,
        QuestionBody::Agent { .. } => 1,
        QuestionBody::User { .. } => 2,
    });
    for (i, ev) in all.into_iter().enumerate() {
        rt(&Event {
            at: Time(u64::try_from(i).unwrap()),
            ev,
        });
    }
    rt(&genesis(Params::default(), Time(1_790_000_000_000)));
    rt(&text_all());
    rt_all(&TEXT_FIELDS);
}

#[test]
fn replies_errors_and_warnings() {
    let all = replies();
    covers(&all, REPLY_VARIANTS, reply_index);
    for r in all {
        rt(&done(r));
    }
    let w = warnings();
    covers(&w, 32, warning_index);
    rt_all(&w);
    let errs = errors();
    covers(&errs, 11, hint_index);
    let ws = whys();
    covers(&ws, 37, why_index);
    rt_all(&errs);
    for e in &errs {
        assert_eq!(e.code, Error::code_of(&e.hint));
    }
    let outcomes: Vec<Result<Done, Error>> =
        vec![Ok(done(Reply::Status)), Err(errors()[0].clone())];
    rt_all(&outcomes);
    rt_all(&[Total::Exact(0), Total::AtLeast(20)]);
    rt_all(&[
        Settled::Done,
        Settled::More,
        Settled::Stuck(DeadlineKey {
            at: Time(1),
            kind: DeadlineKind::Retire,
            owner: Owner::Session(SessionId(1)),
        }),
    ]);
    rt(&empty_why());
    rt(&EmptyWhy::default());
    rt_all(&[
        Reason::Flagged,
        Reason::Offered,
        Reason::Occurrence,
        Reason::Signal,
        Reason::Routing,
        Reason::Status,
        Reason::ClosedFlagged,
        Reason::Review,
    ]);
}

#[test]
fn queries_and_health() {
    rt(&StatusLine {
        root_pause: Some(pause_view()),
        hold_pauses: vec![pause_view()],
        now: Time(8),
        class: class("worker"),
        session: SessionId(1),
        holds: vec![HoldStatus {
            node: NodeId(9),
            lease_until: Time(9),
            checkpoint_by: Time(7),
        }],
        inbox: 2,
        user_decisions: Some(1),
    });
    let who = WhoPage {
        items: vec![Present {
            session: SessionId(1),
            class: class("worker"),
            holds: vec![NodeId(9)],
            last_seen: Time(9),
        }],
        by_class: vec![(class("worker"), 1)],
        total: Total::Exact(1),
    };
    rt(&who);
    rt(&WhoPage::default());
    rt(&HelloData {
        holds: vec![(tok(), NodeId(12), Time(9))],
        caps: labels(&["x"]),
        inbox: vec![Eid(12)],
        offers: Vec::new(),
        offers_why: Some(empty_why()),
        orientation: Orientation {
            map: text("kb/ holds what we know; work/ what we do"),
            house_rules: text("review every merge"),
        },
        present: who.clone(),
        principals: vec![PrincipalId(0)],
        user_decisions: UserItems {
            count: 0,
            items: Vec::new(),
            older: None,
        },
        user_digest: UserItems {
            count: 0,
            items: Vec::new(),
            older: Some(0),
        },
    });
    rt(&AgreementsPage {
        items: vec![AgreementItem {
            node: NodeId(0),
            approvals: 1,
            changed: Some(Time(3)),
        }],
        total: Total::Exact(1),
    });
    rt(&HelloData {
        holds: vec![(tok(), NodeId(12), Time(9))],
        caps: BTreeSet::new(),
        inbox: vec![Eid(12)],
        offers: cands()
            .into_iter()
            .map(|cand| Ranked {
                cand,
                why: why_this(),
            })
            .collect(),
        offers_why: None,
        orientation: Orientation::default(),
        present: who,
        principals: vec![PrincipalId(0)],
        user_decisions: UserItems {
            count: 7,
            items: vec![Eid(5)],
            older: None,
        },
        user_digest: UserItems {
            count: 1,
            items: vec![Eid(11)],
            older: Some(4),
        },
    });
    rt(&FindResult {
        open: Some(NodeId(9)),
        closed: vec![NodeId(3)],
    });
    rt(&ListFilter {
        under: Some(NodeId(0)),
        kind: None,
        tags: labels(&["offer"]),
        status: Some(text("s")),
        fields: fields(),
        flags: FlagSet(3),
        closed: None,
        limit: 50,
    });
    rt(&ListPage {
        nodes: vec![NodeId(9)],
        total: Total::AtLeast(2_000),
        under: Subtree::default(),
    });
    rt(&Backlinks {
        links: vec![(LinkRel::Duplicate, NodeId(9))],
        links_total: Total::Exact(1),
        dependents: vec![NodeId(10)],
        dependents_total: Total::Exact(1),
        cited_by: vec![Eid(8)],
        cited_by_total: Total::AtLeast(20),
    });
    let rows: Vec<BoardRow> = [
        BoardState::Unrouted,
        BoardState::Answered,
        BoardState::ClosedUnanswered,
        BoardState::Decided,
        BoardState::Waiting,
        BoardState::InProgress,
    ]
    .into_iter()
    .map(|state| BoardRow {
        request: Some(Eid(2)),
        entry: None,
        node: Some(NodeId(9)),
        requester: Requester::User(PrincipalId(1)),
        state,
    })
    .collect();
    rt(&BoardPage {
        rows,
        total: Total::Exact(6),
    });
    let items = brief_items();
    covers(&items, 15, brief_index);
    let sections: Vec<Section> = [
        SectionName::Flags,
        SectionName::Unfinished,
        SectionName::SaidAndCommitments,
        SectionName::DecisionsAndAgreements,
        SectionName::UnsettledRequests,
        SectionName::Objections,
        SectionName::Digest,
        SectionName::LastCheckpoint,
        SectionName::Everything,
    ]
    .into_iter()
    .map(|name| Section {
        name,
        items: items.clone(),
        total: 100,
    })
    .collect();
    rt(&BriefData {
        node: NodeId(9),
        version: Version(3),
        digest_version: Version(2),
        holders: vec![(SessionId(1), Time(9))],
        rollup: Rollup {
            open_children: 1,
            held_children: 2,
            flagged_children: 3,
            sub: Subtree {
                open: 1,
                done: 2,
                failed: 3,
                other: 4,
                flagged: 5,
            },
        },
        continues: vec![Predecessor {
            node: NodeId(3),
            unsettled: vec![Eid(2)],
        }],
        continues_total: Total::Exact(1),
        sections,
        rests_on: vec![
            RestsOn {
                node: NodeId(4),
                outcome: Outcome::Failed,
                via: RestsVia::DependsOn,
            },
            RestsOn {
                node: NodeId(5),
                outcome: Outcome::Other(label("refuted")),
                via: RestsVia::Basis(Eid(7)),
            },
        ],
    });
    let reasons = health_reasons();
    covers(&reasons, 7, health_index);
    for level in [Level::Green, Level::Yellow, Level::Red] {
        rt(&Health {
            level,
            reasons: reasons.clone(),
            capacity: CapacityReport {
                demand_min: 1,
                supply_min: 2,
                lost_min: 3,
                next_calls: 4,
                next_empty: 5,
                next_empty_for_you: 6,
                // Unknown with no `next` call in the window (§17, 1.5): absent, then a value.
                idle_permille: (level != Level::Green).then_some(7),
                backlog: 8,
                flagged_active: 9,
                median_done_min: 10,
            },
        });
    }
}

#[test]
fn next_calls() {
    let skels = skeletons();
    covers(&skels, 26, skeleton_index);
    let bs = becauses();
    covers(&bs, 46, because_index);
    // 1.11: rule 5's clarification, and rule 10's merge after a link; 1.12: rule 8's merge wait,
    // and rule 9's reopen of a claim nothing proves.
    covers(
        &bs.iter()
            .filter(|b| because_index(b) >= 49)
            .cloned()
            .collect::<Vec<_>>(),
        4,
        |b| because_index(b) - 49,
    );
    let items: Vec<NextCall> = skels
        .into_iter()
        .zip(bs.into_iter().cycle())
        .zip(RULES.into_iter().cycle())
        .enumerate()
        .map(|(i, ((call, because), rule))| NextCall {
            rule,
            when: if i % 2 == 0 {
                Moment::Now
            } else {
                Moment::Later
            },
            once: i % 3 == 0,
            call,
            because,
        })
        .collect();
    rt_all(&items);
    rt(&Guidance {
        items: items.into_iter().take(3).collect(),
        total: Total::AtLeast(20),
        end: None,
    });
    rt_all(&[
        Guidance {
            items: Vec::new(),
            total: Total::Exact(0),
            end: Some(End::AfterNext),
        },
        Guidance {
            items: Vec::new(),
            total: Total::Exact(0),
            end: Some(End::AtHello),
        },
        Guidance {
            items: Vec::new(),
            total: Total::Exact(0),
            end: Some(End::Idle),
        },
    ]);
    for b in becauses() {
        rt(&b);
    }
    rt_all(&RULES);
}

#[test]
fn the_registry_and_violations_serialize() {
    for row in INVARIANTS {
        let json = serde_json::to_value(row).unwrap();
        assert!(json["verus_todo"].as_str().unwrap().starts_with("V-"));
    }
    let v = Violation {
        inv: Inv::K8,
        detail: ViolationDetail::Deadline(DeadlineKey {
            at: Time(1),
            kind: DeadlineKind::Retire,
            owner: Owner::Session(SessionId(1)),
        }),
    };
    assert!(serde_json::to_string(&v).unwrap().contains("K8"));
    for detail in [
        ViolationDetail::Node(NodeId(1)),
        ViolationDetail::Hold(tok()),
        ViolationDetail::Session(SessionId(1)),
        ViolationDetail::Entry(Eid(1)),
        ViolationDetail::Key(key("chat:x")),
        ViolationDetail::Index("ix.next"),
        ViolationDetail::Text("x"),
    ] {
        assert!(serde_json::to_string(&detail).is_ok());
    }
    assert!(serde_json::to_string(&[InvKind::State, InvKind::Step, InvKind::Property]).is_ok());
    assert!(serde_json::to_string(&VerusStatus::Todo).is_ok());
}
