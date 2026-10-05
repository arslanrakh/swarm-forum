//! `Cmd::longest_text` (design D.5.2, L1, A-114): every `Text` of every variant counts on its own.
//! The sample below fills every text slot of every variant from one numbered source; for each slot
//! in turn, that slot alone is long, and exactly the command that holds it must report its length.
//! A command with no text, and `Admin`, measure 0.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use forum_core::*;

const LONG: usize = 100;

/// Hands out texts: slot number `long` is `LONG` bytes, every other one a short unique text.
struct Texts {
    next: Cell<usize>,
    long: Option<usize>,
}

impl Texts {
    fn t(&self) -> Text {
        let n = self.next.get();
        self.next.set(n + 1);
        if Some(n) == self.long {
            Text("L".repeat(LONG))
        } else {
            Text(format!("s{n}"))
        }
    }
    fn some(&self) -> Option<Text> {
        Some(self.t())
    }
    fn map(&self) -> BTreeMap<Text, Text> {
        let (a, b, c, d) = (self.t(), self.t(), self.t(), self.t());
        BTreeMap::from([(a, b), (c, d)])
    }
}

fn label(s: &str) -> Label {
    Label::new(s).unwrap()
}
fn nref() -> NodeRef {
    NodeRef::Id(NodeId(9))
}
fn key(s: &str) -> Key {
    Key {
        key: KeyStr::new(s).unwrap(),
        nv: NormVer(1),
    }
}
fn tok() -> Token {
    Token {
        node: NodeId(9),
        fence: Fence(1),
    }
}
fn repeat_arg(t: &Texts) -> RepeatArg {
    RepeatArg {
        cron_text: t.t(),
        cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
        tz: forum_cron::TzTable {
            name: "UTC".to_owned(),
            tzdb_version: String::new(),
            initial: 0,
            transitions: Vec::new(),
        },
        run_within: Dur(1),
    }
}

/// Every variant with text, each text slot filled from `t`.
fn with_texts(t: &Texts) -> Vec<Cmd> {
    vec![
        Cmd::Session(SessionCmd::Hello {
            intent: t.some(),
            caps: None,
            horizon: None,
            principal: None,
            from_user: Some(FromUserArg {
                text: t.t(),
                client: t.t(),
                principal: None,
                answers: None,
                to: None,
                clarifies: None,
                reason: None,
            }),
        }),
        Cmd::Session(SessionCmd::Status { text: t.t() }),
        Cmd::View(ViewCmd::Find {
            by: FindBy::Text(t.t()),
        }),
        Cmd::View(ViewCmd::List {
            under: None,
            kind: None,
            tags: BTreeSet::new(),
            status: t.some(),
            fields: t.map(),
            flags: FlagSet::EMPTY,
            closed: None,
            limit: None,
        }),
        Cmd::Node(NodeCmd::Create(CreateArgs {
            path: Path::default(),
            keys: vec![key("chat:x/1")],
            kind: None,
            title: t.t(),
            body: t.some(),
            digest: t.some(),
            fields: t.map(),
            tags: BTreeSet::new(),
            capacity: None,
            needs: None,
            est: None,
            priority: None,
            due: None,
            depends_on: vec![],
            owners: vec![],
            repeat: Some(repeat_arg(t)),
            agreements: t.some(),
            request: Some(RequestArg {
                text: t.t(),
                origin: None,
                where_: None,
                requester: t.t(),
            }),
            take: None,
        })),
        Cmd::Node(NodeCmd::Update {
            at: nref(),
            version: Version(1),
            digest_version: None,
            token: None,
            patch: Patch {
                title: t.some(),
                body: t.some(),
                digest: t.some(),
                fields: Some(t.map()),
                files: Some(t.map()),
                status: t.some(),
                agreements: t.some(),
                kind: Change::Set(label("x")),
                tags_add: BTreeSet::new(),
                tags_remove: BTreeSet::new(),
                capacity: None,
                priority: None,
                est: Change::Keep,
                needs_add: BTreeSet::new(),
                needs_remove: BTreeSet::new(),
                due: Change::Clear,
                depends_on: None,
                owners: None,
                repeat: Change::Set(repeat_arg(t)),
            },
        }),
        Cmd::Node(NodeCmd::Close {
            at: nref(),
            outcome: Outcome::Done,
            summary: t.t(),
            verified: t.some(),
            settles: Some(BTreeSet::new()),
            learnings: t.some(),
            token: None,
            unsent: BTreeSet::new(),
        }),
        Cmd::Node(NodeCmd::Reopen {
            at: nref(),
            reason: t.t(),
        }),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::Entry {
                eid: Eid(1),
                node: NodeId(9),
            },
            reason: t.t(),
        }),
        Cmd::Node(NodeCmd::Hide {
            target: HideArg::File {
                at: nref(),
                name: t.t(),
            },
            reason: t.t(),
        }),
        Cmd::Hold(HoldCmd::Checkpoint {
            token: tok(),
            done: t.t(),
            next_step: t.t(),
            intent: t.some(),
            intent_done: t.some(),
            files: vec![t.t(), t.t()],
        }),
        Cmd::Hold(HoldCmd::Renew {
            token: tok(),
            why: t.t(),
        }),
        Cmd::Hold(HoldCmd::Release {
            token: tok(),
            done: t.t(),
            next_step: t.t(),
            reason: ReleaseReason::Blocked,
            waits_for: Some(WaitArg::External {
                what: t.t(),
                recheck_after: When::In(Dur(1)),
            }),
            intent_done: t.some(),
            learnings: t.some(),
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
        Cmd::Talk(TalkCmd::FromUser {
            text: t.t(),
            client: t.t(),
            principal: None,
            answers: None,
            to: None,
            clarifies: None,
            reason: None,
        }),
        Cmd::Talk(TalkCmd::ToUser {
            text: t.t(),
            kind: ToUserKind::Result,
            verbatim: true,
            principal: None,
            re: BTreeSet::new(),
            at: None,
        }),
        Cmd::Talk(TalkCmd::Route {
            entry: Eid(1),
            to: Some(forum_core::RouteTo::Nowhere),
            answers: None,
            clarifies: None,
            reason: Some(t.t()),
        }),
        Cmd::Talk(TalkCmd::Request {
            at: nref(),
            text: t.t(),
            origin: key("chat:x/1/1"),
            where_: None,
            requester: t.t(),
        }),
        Cmd::Talk(TalkCmd::Said {
            at: nref(),
            where_: None,
            text: t.t(),
            kind: SaidKind::Reply,
            corrects: None,
            seen: None,
        }),
        Cmd::Talk(TalkCmd::SaidConfirm {
            said: Eid(1),
            confirmed: t.t(),
        }),
        Cmd::Talk(TalkCmd::Heard {
            at: nref(),
            where_: None,
            from: t.t(),
            text: t.t(),
            answers: None,
        }),
        Cmd::Talk(TalkCmd::Ask {
            at: nref(),
            text: t.t(),
            to: AskTo::Person {
                where_: None,
                blocking: false,
            },
            again: false,
        }),
        Cmd::Talk(TalkCmd::Ask {
            at: nref(),
            text: t.t(),
            to: AskTo::Agent(AgentAddressee::Session(SessionId(1))),
            again: false,
        }),
        Cmd::Talk(TalkCmd::Ask {
            at: nref(),
            text: t.t(),
            to: AskTo::User(UserAsk {
                to: None,
                options: vec![t.t(), t.t()],
                recommendation: t.some(),
                default: t.some(),
                due: None,
                digest: ReqDigest([0; 32]),
            }),
            again: false,
        }),
        Cmd::Talk(TalkCmd::Answer {
            question: Eid(1),
            node: NodeId(9),
            asker: None,
            text: t.t(),
        }),
        Cmd::Talk(TalkCmd::Say {
            at: nref(),
            text: t.t(),
            to: None,
            expects: None,
            reply_to: None,
            until: None,
        }),
        Cmd::Talk(TalkCmd::Decide {
            at: nref(),
            text: t.t(),
            settles: BTreeSet::new(),
            supersedes: None,
            basis: vec![
                BasisArg::Entry(Eid(1)),
                BasisArg::External {
                    what: t.t(),
                    checked_at: When::At(Time(1)),
                    recheck: None,
                },
                BasisArg::External {
                    what: t.t(),
                    checked_at: When::At(Time(1)),
                    recheck: None,
                },
            ],
            evidence: None,
        }),
        Cmd::Talk(TalkCmd::Approve {
            on: nref(),
            version: Version(1),
            checked: t.t(),
        }),
        Cmd::Talk(TalkCmd::Object {
            on: nref(),
            note: t.t(),
        }),
        Cmd::Talk(TalkCmd::Resolve {
            objection: Eid(1),
            note: t.t(),
            upheld: false,
            defer: false,
        }),
        Cmd::Talk(TalkCmd::Retract {
            target: RetractArg::Node(nref()),
            text: t.t(),
            reason: t.t(),
        }),
        Cmd::Control(ControlCmd::Pause {
            scope: nref(),
            reason: t.t(),
        }),
    ]
}

/// Every variant without a text.
fn without_texts() -> Vec<Cmd> {
    vec![
        Cmd::View(ViewCmd::Node {
            targets: vec![nref()],
            view: None,
            page: None,
        }),
        Cmd::View(ViewCmd::Find {
            by: FindBy::Key(KeyStr::new("chat:x").unwrap()),
        }),
        Cmd::View(ViewCmd::Feed {
            at: nref(),
            since: None,
            types: BTreeSet::new(),
            limit: None,
        }),
        Cmd::View(ViewCmd::History {
            at: nref(),
            before: None,
            limit: None,
        }),
        Cmd::View(ViewCmd::Requests {
            status: None,
            requester: None,
        }),
        Cmd::View(ViewCmd::UserLog {
            since: None,
            limit: None,
        }),
        Cmd::View(ViewCmd::Constitution),
        Cmd::Node(NodeCmd::Link {
            at: nref(),
            rel: LinkRel::Duplicate,
            to: nref(),
        }),
        Cmd::Node(NodeCmd::Key {
            at: nref(),
            change: KeyChange::Add(key("chat:x/1")),
        }),
        Cmd::Node(NodeCmd::Move {
            at: nref(),
            to: Path::default(),
        }),
        Cmd::Node(NodeCmd::Merge {
            from: nref(),
            into: nref(),
        }),
        Cmd::Hold(HoldCmd::Take {
            target: nref(),
            lease: None,
        }),
        Cmd::Hold(HoldCmd::Next {
            query: NextQuery::default(),
            lease: None,
        }),
        Cmd::Hold(HoldCmd::Check {
            target: nref(),
            token: None,
            intent_done: None,
        }),
        Cmd::Talk(TalkCmd::Wait {
            timeout: Dur(1),
            for_: None,
            since: None,
        }),
        Cmd::Talk(TalkCmd::Who { at: None }),
        Cmd::Control(ControlCmd::Unpause { scope: nref() }),
        Cmd::Control(ControlCmd::Health),
        Cmd::Control(ControlCmd::Constitution),
        Cmd::Control(ControlCmd::Export {
            after: None,
            limit: None,
        }),
        Cmd::Admin(AdminCmd::SetParams(Params::default())),
    ]
}

#[test]
fn every_text_slot_counts_on_its_own() {
    let probe = Texts {
        next: Cell::new(0),
        long: None,
    };
    let short = with_texts(&probe);
    let slots = probe.next.get();
    assert!(slots >= 80, "the sample has {slots} text slots");
    for c in &short {
        let len = c.longest_text();
        assert!((2..=3).contains(&len), "{c:?} measures {len}");
    }
    for k in 0..slots {
        let t = Texts {
            next: Cell::new(0),
            long: Some(k),
        };
        let cmds = with_texts(&t);
        let hits: Vec<&Cmd> = cmds
            .iter()
            .filter(|c| c.longest_text() == u64::try_from(LONG).unwrap())
            .collect();
        assert_eq!(hits.len(), 1, "slot {k}: {hits:?}");
    }
}

#[test]
fn commands_without_text_measure_zero() {
    for c in without_texts() {
        assert_eq!(c.longest_text(), 0, "{c:?}");
    }
}

#[test]
fn reads_and_writes() {
    for c in without_texts() {
        let read = matches!(
            c,
            Cmd::View(_)
                | Cmd::Hold(HoldCmd::Check { .. })
                | Cmd::Talk(TalkCmd::Who { .. } | TalkCmd::Wait { .. })
                | Cmd::Control(
                    ControlCmd::Health | ControlCmd::Constitution | ControlCmd::Export { .. }
                )
        );
        assert_eq!(c.is_read(), read, "{c:?}");
    }
    let t = Texts {
        next: Cell::new(0),
        long: None,
    };
    for c in with_texts(&t) {
        let read = matches!(c, Cmd::View(_) | Cmd::Session(SessionCmd::Hello { .. }));
        assert_eq!(c.is_read(), read, "{c:?}");
    }
}
