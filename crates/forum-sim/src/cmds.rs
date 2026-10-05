//! Builders of the commands the simulated agents send, already in the form the shell would hand the
//! core (D.5): normalised keys, masked texts, paths as names.

use std::collections::BTreeSet;

use forum_core::{
    AskTo, Cmd, CreateArgs, HoldCmd, Key, KeyStr, Label, Name, NextQuery, NodeCmd, NodeId, NodeRef,
    NormVer, Outcome, Patch, Path, PrincipalKey, QuestionId, ReleaseReason, ReqDigest, RequestArg,
    RequestId, SaidKind, SessionCmd, TalkCmd, Text, ToUserKind, Token, UserAsk, UserEntryId,
    Version, WaitArg,
};

/// A text.
#[must_use]
pub fn text(s: &str) -> Text {
    Text(String::from(s))
}

/// A label, which the simulation always spells validly.
///
/// # Panics
///
/// On an invalid label (a bug in the simulation).
#[must_use]
pub fn label(s: &str) -> Label {
    Label::new(s).unwrap_or_else(|| panic!("invalid label {s:?}"))
}

/// A key as the shell's normaliser would give it.
///
/// # Panics
///
/// On an invalid key (a bug in the simulation).
#[must_use]
pub fn key(s: &str) -> Key {
    Key {
        key: KeyStr::new(s).unwrap_or_else(|| panic!("invalid key {s:?}")),
        nv: NormVer(1),
    }
}

/// The key that routes a user entry (made by the core itself, version 0).
#[must_use]
pub fn user_entry_key(e: UserEntryId) -> Key {
    Key {
        key: KeyStr::of_user_entry(e),
        nv: NormVer(0),
    }
}

/// A principal key.
///
/// # Panics
///
/// On an invalid key (a bug in the simulation).
#[must_use]
pub fn principal(s: &str) -> PrincipalKey {
    PrincipalKey::new(s).unwrap_or_else(|| panic!("invalid principal {s:?}"))
}

/// A path from its steps.
///
/// # Panics
///
/// On an invalid step (a bug in the simulation).
#[must_use]
pub fn path(steps: &[&str]) -> Path {
    Path(
        steps
            .iter()
            .map(|s| Name::new(s).unwrap_or_else(|| panic!("invalid name {s:?}")))
            .collect(),
    )
}

/// `session hello {caps?, principal?}`.
#[must_use]
pub fn hello(caps: Option<BTreeSet<Label>>, principal: Option<PrincipalKey>) -> Cmd {
    Cmd::Session(SessionCmd::Hello {
        intent: None,
        caps,
        horizon: None,
        principal,
        from_user: None,
    })
}

/// `node create {path, title}` with the defaults, changed by `f`.
#[must_use]
pub fn create(p: Path, title: &str, f: impl FnOnce(&mut CreateArgs)) -> Cmd {
    let mut a = CreateArgs {
        path: p,
        keys: Vec::new(),
        kind: None,
        title: text(title),
        body: None,
        digest: None,
        fields: std::collections::BTreeMap::new(),
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
        take: Some(false),
    };
    f(&mut a);
    Cmd::Node(NodeCmd::Create(a))
}

/// A request from outside the swarm, recorded with `create` (`request: {text, requester}`).
#[must_use]
pub fn external_request(t: &str, requester: &str) -> RequestArg {
    RequestArg {
        text: text(t),
        origin: None,
        where_: None,
        requester: text(requester),
    }
}

/// `node update {path, version, …}` with a patch made by `f`.
#[must_use]
pub fn update(at: NodeId, version: Version, f: impl FnOnce(&mut Patch)) -> Cmd {
    let mut patch = Patch::default();
    f(&mut patch);
    Cmd::Node(NodeCmd::Update {
        at: NodeRef::Id(at),
        version,
        digest_version: None,
        token: None,
        patch,
    })
}

/// `hold next {}`: an open `next`, which makes the class active for E1 (CS-7).
#[must_use]
pub fn next() -> Cmd {
    Cmd::Hold(HoldCmd::Next {
        query: NextQuery::default(),
        lease: None,
    })
}

/// `hold checkpoint {token, done, next_step}`.
#[must_use]
pub fn checkpoint(token: Token) -> Cmd {
    Cmd::Hold(HoldCmd::Checkpoint {
        token,
        done: text("progress"),
        next_step: text("continue"),
        intent: None,
        intent_done: None,
        files: Vec::new(),
    })
}

/// `hold release {token, reason, waits_for?}`.
#[must_use]
pub fn release(token: Token, reason: ReleaseReason, waits_for: Option<WaitArg>) -> Cmd {
    Cmd::Hold(HoldCmd::Release {
        token,
        done: text("progress so far"),
        next_step: text("continue when unblocked"),
        reason,
        waits_for,
        intent_done: None,
        learnings: None,
        unsent: BTreeSet::new(),
        settles: BTreeSet::new(),
    })
}

/// `node close {path, outcome: done, summary, settles, token}`.
#[must_use]
pub fn close(at: NodeId, token: Option<Token>, settles: BTreeSet<RequestId>) -> Cmd {
    Cmd::Node(NodeCmd::Close {
        at: NodeRef::Id(at),
        outcome: Outcome::Done,
        summary: text("done"),
        verified: None,
        settles: Some(settles),
        learnings: None,
        token,
        unsent: BTreeSet::new(),
    })
}

/// `talk from_user {text, answers?}` in a session that serves one principal.
#[must_use]
pub fn from_user(t: &str, answers: Option<QuestionId>) -> Cmd {
    Cmd::Talk(TalkCmd::FromUser {
        text: text(t),
        client: text("chat"),
        principal: None,
        answers,
        to: None,
        clarifies: None,
        reason: None,
    })
}

/// `talk route {entry, to: none}`.
#[must_use]
pub fn route_none(entry: UserEntryId) -> Cmd {
    Cmd::Talk(TalkCmd::Route {
        entry,
        to: Some(forum_core::RouteTo::Nowhere),
        answers: None,
        clarifies: None,
        reason: Some(text("nothing to do")),
    })
}

/// `talk to_user {text, kind, principal?, re}`.
#[must_use]
pub fn to_user(
    kind: ToUserKind,
    principal: Option<PrincipalKey>,
    re: BTreeSet<forum_core::Eid>,
) -> Cmd {
    Cmd::Talk(TalkCmd::ToUser {
        text: text("report"),
        kind,
        verbatim: true,
        principal,
        re,
        at: None,
    })
}

/// `talk said {at, kind: result}` in the node's own conversation.
#[must_use]
pub fn said_result(at: NodeId) -> Cmd {
    Cmd::Talk(TalkCmd::Said {
        at: NodeRef::Id(at),
        where_: None,
        text: text("result"),
        kind: SaidKind::Result,
        corrects: None,
        seen: None,
    })
}

/// `talk ask {at, to: user, options, default?}`; `digest` makes each question distinct.
#[must_use]
pub fn ask_user(at: NodeId, default: bool, digest: u64) -> Cmd {
    let mut d = [0_u8; 32];
    d[..8].copy_from_slice(&digest.to_le_bytes());
    Cmd::Talk(TalkCmd::Ask {
        at: NodeRef::Id(at),
        text: text("which way?"),
        to: AskTo::User(UserAsk {
            to: None,
            options: vec![text("a"), text("b")],
            recommendation: Some(text("a")),
            default: default.then(|| text("a")),
            due: None,
            digest: ReqDigest(d),
        }),
        again: false,
    })
}

/// `talk ask {at, text, blocking}` to a person, in the node's own conversation.
#[must_use]
pub fn ask_person(at: NodeId, blocking: bool) -> Cmd {
    Cmd::Talk(TalkCmd::Ask {
        at: NodeRef::Id(at),
        text: text("could you confirm?"),
        to: AskTo::Person {
            where_: None,
            blocking,
        },
        again: false,
    })
}

/// `talk heard {at, from, text, answers}` in the node's own conversation.
#[must_use]
pub fn heard(at: NodeId, answers: QuestionId) -> Cmd {
    Cmd::Talk(TalkCmd::Heard {
        at: NodeRef::Id(at),
        where_: None,
        from: text("reporter"),
        text: text("here is the answer"),
        answers: Some(answers),
    })
}
