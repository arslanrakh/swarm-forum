//! Next calls (design D.8.26, §20.9, G1): the types and constants are frozen at WP-D1; the
//! algorithm is WP-D7's.
//!
//! `guide` is a pure query beside `brief` and `hello`: nothing in `apply`, `settle`, or `evolve`
//! calls it, so no outcome can depend on it. Its inputs are `(&State, Option<SessionId>, Time,
//! &Cmd, &Result<Done, Error>)`, and, 1.11 (`guide_with`), the first row of the call's `related`,
//! which the shell's search found; iteration is over `BTreeMap` and `BTreeSet` in key order, with
//! ties by token and id, and no text is read but the words of titles and bodies that the tidy scan
//! compares (1.9) and whether a release's `intent_done` names a pull request (1.10, rule 8). It
//! reads the state after the call, settled at its `now` (T3): the calling session's holds, its own
//! `said` entries and unrouted user entries, its principals, the requests the call names or
//! recorded, the nodes it holds and their conversation nodes (1.12: whether a held node's work
//! waits for its merge, `Node::merge_wait`), and the reply; never the health cache.
//! 1.12 also reads `next`'s exclusions of what the session put down (rule 13, through `ranked` and
//! `leave_better`).
//!
//! `standard_fill` instantiates an item as G1 (b) does (D.9.2), for `check_guide` and the tests;
//! `same_call` and `same_skeleton` are the equality of §20.9's dedupe.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use super::{MOVED_MAX, Meter, SCAN_MAX, review_objection};
use crate::cmd::{
    AddresseeArg, AskTo, BasisArg, Cmd, Ctx, HoldCmd, NextQuery, NodeCmd, Patch, RetractArg,
    SessionCmd, SessionCtx, TalkCmd, ViewCmd, WaitArg,
};
use crate::error::{Error, Hint, LimitKind, What, Why};
use crate::helpers::conversation::conv_key;
use crate::helpers::principals::same_person;
use crate::reply::{Done, NextReply, Reply, RoutedReply, Total, WarningCode};
use crate::state::{
    AqState, Author, Hold, Node, NodeItems, Role, Routing, State, UqRank, UqSource, UserEntry, live,
};
use crate::types::{
    ClassName, DecisionId, DigestId, Dur, Eid, Fence, Flag, FlagSet, Key, KeyStr, Label, Name,
    NodeId, NodeRef, NormVer, NoteId, OFFER, ObjectionId, Outcome, Path, PrincipalId, PrincipalKey,
    QuestionId, ReleaseReason, RequestId, Requester, SECOND, SaidId, SaidKind, Seq, SessionId,
    SignalKind, Text, Time, ToUserKind, Token, UserEntryId, Version,
};

/// Items in a response (§20.9, §22 fixed values).
pub const NEXT_CALLS_MAX: usize = 3;
/// Rule 5's items per unrouted entry (1.9): the likeliest routing, with the next one named in its
/// `why` as the alternative. 1.7's two items read as two steps, and agents sent both in one call.
pub const ROUTE_ITEMS: usize = 1;
/// `total` exact below it, `≥20` from it (§18).
pub const NEXT_TOTAL_CAP: u32 = 20;
/// Ids per list in an item's arguments.
pub const NEXT_IDS_MAX: usize = 5;

/// §10.2 "about every 5 minutes", and §22's fixed value: a checkpoint is due 5 minutes after the
/// hold's issue, last checkpoint, or renewal. The same in every build: Cargo unifies features
/// within one build, and `forum-conformance` enables `conformance`, so a value chosen by that
/// feature would reach `forum-server` in every build of the whole workspace (D.1.2). `guide` always
/// computes with it; only `guide_items` uses the model's ticks, `CHECKPOINT_EVERY_TICKS`.
pub const CHECKPOINT_EVERY: Dur = Dur(300_000);

/// The margin before the lease ends: 1 minute, in every build (see `CHECKPOINT_EVERY`).
pub const LEASE_MARGIN: Dur = Dur(60_000);

/// `CHECKPOINT_EVERY` in the model's ticks as minutes, 2 (D.10.5), as the replay's params are the
/// scaled ones: what `guide_items` computes with, so that conformance compares its items with
/// `guide.qnt`'s (D.8.26, D.10.8). Never read by `guide`.
#[cfg(any(test, feature = "conformance"))]
pub const CHECKPOINT_EVERY_TICKS: Dur = Dur(120_000);

/// `LEASE_MARGIN` in the model's ticks as minutes, 1 (D.10.5): its real value too. What
/// `guide_items` computes with.
#[cfg(any(test, feature = "conformance"))]
pub const LEASE_MARGIN_TICKS: Dur = Dur(60_000);

/// The next calls of one response.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Guidance {
    /// At most `NEXT_CALLS_MAX`.
    pub items: Vec<NextCall>,
    /// Every item, shown or not, `≥20` from 20.
    pub total: Total,
    /// 1.7 (§20.9 "End"): nothing is owed or offered to the session, which should end its run.
    /// Only with no item (G1 (e)).
    #[cfg_attr(feature = "serde", serde(default))]
    pub end: Option<End>,
}

/// Why the next calls end (1.7, §20.9): the shell renders one text per variant.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum End {
    /// A `hold next` found nothing, and `leave_better` is empty: "nothing is offered to you: end
    /// your run; waiting nodes come back by themselves".
    AfterNext,
    /// A `session hello` with no offer: "nothing is offered to you; unless you came for a message
    /// or an event, end your run". 1.10: also one refused for a `from_user` that names no
    /// principal ([`hello_refused_unnamed`]), whose session is open.
    AtHello,
    /// 1.8 (§20.9 rule 13): a `hold release`, `talk resolve`, or `node reopen` by a worker that
    /// then holds nothing, and to which `next` would offer nothing: `AfterNext`'s text.
    Idle,
}

/// `once`: the item reacts to this call (§20.9's one-shot rules) and will not come back; the cut
/// keeps one such item (`finish`, D.8.26).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NextCall {
    /// The rule that produced it.
    pub rule: Rule,
    /// Now or later.
    pub when: Moment,
    /// A one-shot item.
    pub once: bool,
    /// The call.
    pub call: Skeleton,
    /// What the `why` template reads.
    pub because: Because,
}

/// §20.9's `when` (`When` is the time argument of D.2.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Moment {
    /// Make it now.
    Now,
    /// When the moment `why` names comes.
    Later,
}

/// §20.9's rules by the name the structured content carries as `rule` (each variant in snake case:
/// `Remedy` is `remedy`, `TellHolder` `tell_holder`). `Rule::number(self)` is the row of §20.9 (0
/// for E, then 1 to 13); items are emitted in ascending number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Rule {
    /// E.
    Remedy,
    /// 1.
    LogFirst,
    /// 2. 1.9: rule 2's confirm is gone (§9.2): no next call asks to confirm a `said`.
    TellHolder,
    /// 3.
    Checkpoint,
    /// 3.
    IntentDone,
    /// 4.
    Reread,
    /// 5.
    Route,
    /// 6.
    Reply,
    /// 7.
    Decide,
    /// 8.
    Close,
    /// 8.
    ReleaseContainer,
    /// 8, 1.10: after a `handoff` release whose `intent_done` names a pull request, the take that
    /// lets the agent release it again with an outside wait for the merge.
    AwaitMerge,
    /// 9.
    AskWait,
    /// 9.
    AgentWait,
    /// 9.
    JoinAnswer,
    /// 9.
    Offer,
    /// 9.
    AnswersNext,
    /// 9.
    StatusReply,
    /// 9, 1.9: the resolve of a review item (§10.4).
    Review,
    /// 9.
    RouteItem,
    /// 9, 1.12: after a `talk resolve` that returned `no_basis`, the reopen of the claim nothing
    /// proves.
    Unproved,
    /// 10.
    HeldFlag,
    /// 11.
    Inbox,
    /// 11.
    AnswerAgent,
    /// 12.
    HumanItems,
    /// 13.
    Tidy,
    /// 13.
    TakeJoined,
    /// 13.
    MoreWork,
}

impl Rule {
    /// The row of §20.9: 0 for E, then 1 to 13.
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Rule::Remedy => 0,
            Rule::LogFirst => 1,
            Rule::TellHolder => 2,
            Rule::Checkpoint | Rule::IntentDone => 3,
            Rule::Reread => 4,
            Rule::Route => 5,
            Rule::Reply => 6,
            Rule::Decide => 7,
            Rule::Close | Rule::ReleaseContainer | Rule::AwaitMerge => 8,
            Rule::AskWait
            | Rule::AgentWait
            | Rule::JoinAnswer
            | Rule::Offer
            | Rule::AnswersNext
            | Rule::StatusReply
            | Rule::Review
            | Rule::RouteItem
            | Rule::Unproved => 9,
            Rule::HeldFlag => 10,
            Rule::Inbox | Rule::AnswerAgent => 11,
            Rule::HumanItems => 12,
            Rule::Tidy | Rule::TakeJoined | Rule::MoreWork => 13,
        }
    }

    /// The name the structured content carries as `rule` (§20.9).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Rule::Remedy => "remedy",
            Rule::LogFirst => "log_first",
            Rule::TellHolder => "tell_holder",
            Rule::Checkpoint => "checkpoint",
            Rule::IntentDone => "intent_done",
            Rule::Reread => "reread",
            Rule::Route => "route",
            Rule::Reply => "reply",
            Rule::Decide => "decide",
            Rule::Close => "close",
            Rule::ReleaseContainer => "release_container",
            Rule::AwaitMerge => "await_merge",
            Rule::AskWait => "ask_wait",
            Rule::AgentWait => "agent_wait",
            Rule::JoinAnswer => "join_answer",
            Rule::Offer => "offer",
            Rule::AnswersNext => "answers_next",
            Rule::StatusReply => "status_reply",
            Rule::Review => "review",
            Rule::RouteItem => "route_item",
            Rule::Unproved => "unproved",
            Rule::HeldFlag => "held_flag",
            Rule::Inbox => "inbox",
            Rule::AnswerAgent => "answer_agent",
            Rule::HumanItems => "human_items",
            Rule::Tidy => "tidy",
            Rule::TakeJoined => "take_joined",
            Rule::MoreWork => "more_work",
        }
    }
}

/// The call as the server knows it: §20.9's `tool` and `args`, with ids, tokens, versions, and
/// principals. The shell adds `session` (except to `Hello`, which opens a new session), renders
/// the rest, and lists the action's other arguments as `fill`, which is a fixed function of the
/// variant (the comment after each), so the core holds no argument names and no text.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Skeleton {
    /// `session hello {}`.
    Hello,
    /// `talk from_user`; fill text, client.
    LogFromUser {
        /// The principal.
        principal: PrincipalId,
    },
    /// `view node {path}`.
    ViewNode {
        /// The node.
        node: NodeId,
    },
    /// `hold checkpoint {token, intent_done?}`; fill done, next_step (and the intent_done text).
    /// `intent` (1.4, R18): fill `intent` too, after a `create` that returned `uncovered_slot`.
    Checkpoint {
        /// The hold.
        token: Token,
        /// With `intent_done`.
        intent_done: bool,
        /// With `intent`.
        intent: bool,
    },
    /// The remedy of `invalid(IntentOpen)`: fill intent_done, done, next_step, and `intent?`, which
    /// the agent may leave out (1.4, §21, R6: only if its own action is still to be done).
    CheckpointSwap {
        /// The hold.
        token: Token,
    },
    /// `hold release {token, reason: blocked, waits_for: {question}}`, or `{token, reason:
    /// handoff}` without `wait`; fill done, next_step.
    Release {
        /// The hold.
        token: Token,
        /// The question to wait for.
        wait: Option<QuestionId>,
    },
    /// `hold take {target}`.
    Take {
        /// The node.
        target: NodeId,
    },
    /// `hold next {}`.
    Next,
    /// `node create {keys: [user:<e>]}`; fill path, title.
    CreateFor {
        /// The user entry.
        entry: UserEntryId,
    },
    /// `node create {path: <parent>/?, take: false, tags: [offer]}`; fill name, title. The
    /// parent's path is rendered whole, even over 64 bytes, because `create` takes a path (D.11.7).
    Split {
        /// The parent.
        parent: NodeId,
    },
    /// `node update {path, version, tags_add: [offer]}`.
    AddOffer {
        /// The node.
        node: NodeId,
        /// Its version.
        version: Version,
    },
    /// `node close`; fill summary, and `outcome` when None. 1.4 (rule 8, R4): on a node the
    /// session's take reopened, `outcome` is the one the node had, unless that is `missed`.
    Close {
        /// The node.
        node: NodeId,
        /// The hold.
        token: Token,
        /// The answered requests.
        settles: Vec<RequestId>,
        /// The outcome, when the server knows it.
        outcome: Option<Outcome>,
    },
    /// `talk say {at, to: session:<s>}`; fill text.
    SayTo {
        /// The conversation node.
        at: NodeId,
        /// Its holder.
        to: SessionId,
    },
    /// `talk to_user {principal, kind?, re}`; fill text, verbatim? (optional since 1.6), and kind
    /// when None.
    ToUser {
        /// To whom.
        principal: PrincipalId,
        /// The kind, if fixed.
        kind: Option<ToUserKind>,
        /// What it reports on.
        re: Vec<Eid>,
    },
    /// `talk said {at, kind, where?}`; fill text, and `where` when `where_fill`. `where_` is the
    /// requests' conversation key (`Request::conv`) for rule 6, or the commitments'
    /// (`Commitment::conv`) for rule 9's status reply, when the default of §9.1 would name another
    /// key, so the reply lands in their conversation (§9.2, §9.4, 1.3).
    Said {
        /// The conversation node.
        at: NodeId,
        /// The kind.
        kind: SaidKind,
        /// The conversation key, when the default would name another.
        where_: Option<KeyStr>,
        /// The agent fills `where`.
        where_fill: bool,
    },
    /// `talk heard {at, where, answers}`; fill from, text. `where_` is the question's conversation
    /// key (`PQuestion::conv`), so a node with several conversation keys never refuses it (§9.1).
    Heard {
        /// The conversation node.
        at: NodeId,
        /// The question.
        answers: QuestionId,
        /// Its conversation key.
        where_: KeyStr,
    },
    /// `talk decide`; fill text, basis?.
    Decide {
        /// The node.
        at: NodeId,
        /// The requests it settles.
        settles: Vec<RequestId>,
        /// The decision it supersedes.
        supersedes: Option<DecisionId>,
    },
    /// `talk route {entry, …}`: with `RouteSk::Fill`, fill `to` and `reason?`; with `Nowhere`,
    /// `to: none`, fill `reason?`; with `Node`, `to: <node>`; with `Answers`, `answers: <question>`
    /// (1.7).
    Route {
        /// The user entry.
        entry: UserEntryId,
        /// Where it goes.
        to: RouteSk,
    },
    /// `talk from_user {answers, principal?}`: `principal`, the question's addressee, given when
    /// the session serves several persons (§8.1); fill text, client.
    AnswerUser {
        /// The question.
        question: QuestionId,
        /// The addressee.
        principal: Option<PrincipalId>,
    },
    /// `talk answer`; fill text.
    AnswerAgent {
        /// The agent question.
        question: QuestionId,
    },
    /// `talk resolve`; fill text (`note` before 1.6) and, 1.9, `upheld`: whether the objection
    /// stands.
    Resolve {
        /// The objection.
        objection: ObjectionId,
    },
    /// `node merge`.
    Merge {
        /// The node merged away.
        from: NodeId,
        /// The node that remains.
        into: NodeId,
    },
    /// 1.10 (§16, rule 13): `talk ask {at, to: user, options: [keep, drop], default: drop}`; fill
    /// text: a stale offer's question for its requesters and owners, whose chain reaches them (§8.4).
    AskOwners {
        /// The stale offer.
        at: NodeId,
    },
    /// 1.9 (§16, rule 13): `node link {path: from, rel: duplicate, to}`, which opens a duplicate
    /// signal (§14); `from` is the node §16's merge would merge away.
    LinkDuplicate {
        /// The source, the node a merge would merge away.
        from: NodeId,
        /// The target, the node that would remain.
        to: NodeId,
    },
    /// `talk wait {timeout}`.
    Wait {
        /// The timeout.
        timeout: Dur,
    },
    /// 1.12 (rule 9): `node reopen {path}`; fill reason.
    Reopen {
        /// The node.
        node: NodeId,
    },
}

/// Where a rule-5 or rule-9 route sends its entry (1.7, §8.2, §20.9).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RouteSk {
    /// The agent writes `to`.
    Fill,
    /// `to: none`.
    Nowhere,
    /// `to: <node>`.
    Node(NodeId),
    /// `answers: <question>`.
    Answers(QuestionId),
    /// 1.11 (§8.2): `clarifies: <request>`, the newest unsettled request of the entry's person on the
    /// node of the session's only hold, as a `from_user` with no target is routed.
    Clarifies(RequestId),
}

/// The alternative a rule-5 item names in its `why` (1.9, §20.9): the next routing that applies.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RouteAlt {
    /// `to: none`, after a `to_user` that named the entry.
    Nowhere,
    /// `to: <node>`: the session's only hold (`held`), or the node of the pending question.
    Node {
        /// The node.
        node: NodeId,
        /// The session's only hold.
        held: bool,
    },
    /// `node create` by key.
    Create,
}

/// What the shell's `why` template for an item reads (D.11.7): ids, times, counts, and flags only.
/// One variant per template; the shell takes texts (an intent, a principal's key) from its own
/// tables, and cuts the line to 100 bytes with "…".
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Because {
    // E: the remedy of §21's table
    /// `stale_token`: "your write is saved as late e…".
    HoldEnded {
        /// The `late` entry.
        late: Eid,
    },
    /// `stale_version`, a missing `digest_version`, a version ahead.
    Reread {
        /// The node's version.
        version: Version,
        /// Its digest version.
        digest_version: Version,
    },
    /// The target was merged.
    MergedInto {
        /// Where it lives on.
        node: NodeId,
    },
    /// A new intent while one is open. 1.4 (§21, R6): for another session's intent, the check
    /// first: "once you have checked whether s-41's e119 happened: record what you found; an intent
    /// of your own only if your action is still to be done".
    CloseOpenIntent {
        /// Whose intent is open.
        by: SessionId,
        /// Its entry.
        entry: Eid,
        /// The caller's own.
        own: bool,
    },
    /// `limit(Renew)`.
    RenewLimit,
    /// The answering call of the question's kind.
    AnswerFits {
        /// The question.
        question: QuestionId,
    },
    /// The call's own session: unknown, retired, or of another class (`not_found(Addressee)` has
    /// no remedy, 1.4, R12).
    NoSession,
    // 1 to 5
    /// Rule 1.
    LogFirst {
        /// The principal.
        principal: PrincipalId,
    },
    /// Rule 2's `say` to the holder.
    TellHolder {
        /// The holder.
        holder: SessionId,
    },
    /// Rule 3: the checkpoint is due.
    LeaseDue {
        /// The lease end.
        lease_until: Time,
        /// At `lease.ceiling`.
        at_ceiling: bool,
        /// An open intent.
        intent: Option<Eid>,
        /// 1.12's review: rule 8 closes the node now, so the checkpoint is `later`, for work that
        /// goes on: "if not done: …". Made first, it left the request the close would settle
        /// unfinished (§20.9 Terms), and nothing suggested the close again (the guided property
        /// test's seed 9).
        closing: bool,
    },
    /// Rule 3: an open intent. 1.4 (R6): for another session's intent (`own` false), the check
    /// first.
    IntentOpen {
        /// Whose.
        by: SessionId,
        /// Its entry.
        entry: Eid,
        /// The caller's own.
        own: bool,
    },
    /// Rule 3: at issue.
    CheckpointBy {
        /// The due time.
        by: Time,
    },
    /// Rule 3, 1.4 (§12, R18): "if you set up an outside trigger: record its intent first".
    UncoveredSlot,
    /// Rule 4: entries by others.
    News {
        /// The node.
        node: NodeId,
        /// How many.
        count: Total,
    },
    /// Rule 5: `node create` by key, the last routing; 1.9: words that need no work are routed to
    /// none. 1.12: `related`, the first row of the `related` of the call that logged the entry when
    /// it is an open work node (`related_target`), which the `why` names beside the create: "if new
    /// work: create by key; if only #38's work: route there; if answered: to_user, to none", where
    /// 1.11 made the route to it the item, and a text search's ranking chose where a message lives.
    Unrouted {
        /// The entry.
        entry: UserEntryId,
        /// 1.12: the open work node the call's `related` named first.
        #[cfg_attr(feature = "serde", serde(default))]
        related: Option<NodeId>,
    },
    /// Rule 5 after a `to_user` that named it; 1.9: "if nothing else in <entry> needs work".
    AnsweredEntry {
        /// The entry.
        entry: UserEntryId,
        /// 1.9: the alternative.
        or: Option<RouteAlt>,
    },
    /// Rule 5 (1.7): "if <entry>'s words answer <question>"; 1.9: with the alternative.
    RouteAnswers {
        /// The entry.
        entry: UserEntryId,
        /// The pending question.
        question: QuestionId,
        /// 1.9: the alternative.
        or: Option<RouteAlt>,
    },
    /// Rule 5 (1.7): "if its work lives there": the session's only hold (`held`), or the node the
    /// pending question is about; 1.9: with the alternative.
    RouteNode {
        /// The entry.
        entry: UserEntryId,
        /// The node.
        node: NodeId,
        /// The session's only hold.
        held: bool,
        /// 1.9: the alternative.
        or: Option<RouteAlt>,
    },
    /// Rule 5, 1.11 (§8.2): "if <entry> follows up <request>: route it as its clarification", the
    /// request the newest unsettled one of the entry's person on the node of the session's only
    /// hold; with the alternative.
    RouteClarifies {
        /// The entry.
        entry: UserEntryId,
        /// The request.
        request: RequestId,
        /// The alternative.
        or: Option<RouteAlt>,
    },
    // 6 to 8
    /// Rule 6.
    ReplyOwed {
        /// The requests.
        requests: Vec<RequestId>,
        /// A `to_user` of kind `result` went to another person.
        misaddressed: Option<PrincipalId>,
        /// 1.7: the call recorded them on a node the caller holds (`later`): "once its work is
        /// done: reply with the result, then close, which settles it".
        recorded: bool,
        /// 1.7: a node the caller holds carries `ready_to_close`, and they are unanswered.
        ready: bool,
    },
    /// Rule 7. `recorded` (1.4, R3): the call recorded these requests, on a node nobody holds:
    /// "if it needs no further work: decide, then reply; nobody holds this node"; or, `held`, on a
    /// node the caller holds (R28, M-9): "once the work it asks for is done: decide, then reply,
    /// then close".
    DecisionOwed {
        /// The requests.
        requests: Vec<RequestId>,
        /// After a close.
        after_close: bool,
        /// `ready_to_close`.
        ready: bool,
        /// The call recorded them.
        recorded: bool,
        /// The caller holds the node, a container since 1.7 (§20.9 rule 7: "on a container you
        /// hold, `later`, once that work is done").
        held: bool,
        /// 1.7: this call released the node and left answered requests there undecided.
        after_release: bool,
    },
    /// "If the work is done"; 1.4 (R11): "if it waits on someone outside: hold release
    /// {waits_for: {external}}"; 1.10: "if done; if it waits for a review or merge: release
    /// blocked, waits_for {external: 'PR #N merged'}". 1.12 (rule 8 by the node's state): on a
    /// node whose work does not wait for its merge (`Node::merge_wait`), "if done; else release
    /// handoff with the rest in next_step"; 1.11's "decide now" made workers file a pull
    /// request's status as a decision.
    Finished,
    /// 1.12 (rule 8): the node's work waits for its merge (`Node::merge_wait`): its last
    /// `intent_done`, or its open intent, names a pull request: with
    /// answered, unsettled requests, which the close settles, "if done; awaiting merge: release
    /// blocked, waits_for {external: 'PR #N merged'}, settles: [e2]", the release's
    /// `settles` (§10.3); with none, "if it is merged; waiting for its merge: release blocked,
    /// waits_for {external: 'PR #N merged'}".
    MergeWait {
        /// The answered, unsettled requests a blocked release would settle.
        settles: Vec<RequestId>,
    },
    /// 1.10 (rule 8): after a `handoff` release whose `intent_done` names a pull request: "if it
    /// waits for a review or merge: take it, release blocked, waits_for {external: 'PR #N
    /// merged'}"; 1.12: never "decide now".
    AwaitMerge,
    /// 1.4 (§10.1, R4); 1.6: "if `<was>` still stands, else failed if an objection stood: left
    /// open, it is offered as work".
    Reclose {
        /// The outcome the node had.
        was: Outcome,
    },
    /// Series: "a series stays open: release it; closing stops it"; a person or team: "a record
    /// stays open: release it; never close it".
    ContainerStays {
        /// A series, not a person or team.
        series: bool,
    },
    // 9
    /// After `talk ask`.
    AskWait {
        /// The question.
        question: QuestionId,
        /// Blocking.
        blocking: bool,
        /// Already asked.
        existing: bool,
    },
    /// After asking agents.
    AgentWait {
        /// The question.
        question: QuestionId,
    },
    /// After a join of a node with an open question.
    JoinAnswer {
        /// The question.
        question: QuestionId,
    },
    /// Split-off work nothing offers.
    NotOffered,
    /// Questions still pending for the caller's persons.
    AnswersNext {
        /// The question.
        question: QuestionId,
        /// Its addressee.
        principal: PrincipalId,
    },
    /// A status item.
    StatusItem,
    /// 1.9 (§10.4, §11.3): the review of a closed node's open objection, after a `next` that gave
    /// it, as rule 13's tidy item, or as the remedy of a take refused for it: "decide if the
    /// objection stands: upheld corrects it to refuted or failed, kept closed; false keeps it".
    Review {
        /// The objection.
        objection: ObjectionId,
    },
    /// A routing item.
    RouteItem,
    /// 1.12 (§11.3, rule 9): a resolve that returned `no_basis`: "if nothing proves it: reopen; it
    /// reads unproved, not refuted; a decide with its basis keeps it done".
    Unproved,
    // 10
    /// An assumption mark.
    Assumption {
        /// The questions.
        questions: Vec<QuestionId>,
        /// A lapsed blocking question: take no irreversible step.
        blocking: bool,
    },
    /// A stale basis.
    StaleBasis {
        /// The decision.
        decision: DecisionId,
    },
    /// A recheck mark.
    Recheck {
        /// The question.
        question: QuestionId,
    },
    /// `too_big`.
    TooBig {
        /// The attempts.
        attempts: u32,
    },
    /// A duplicate pair.
    Duplicate {
        /// The other node.
        other: NodeId,
    },
    /// 1.11 (rule 10): after a `node link {rel: duplicate}`, "if they are one piece of work: merge
    /// into <into>; its requests and keys move, the signal closes".
    Linked {
        /// The node that remains.
        into: NodeId,
    },
    /// A signal.
    Signal,
    /// An open objection. 1.9 (§20.9 rule 10): `retracted`, the newest retracted decision of the
    /// node when it has no active one: that retraction is done, and the `why` says so.
    Objection {
        /// The objection.
        objection: ObjectionId,
        /// 1.9: the node's newest retracted decision, while none is active.
        #[cfg_attr(feature = "serde", serde(default))]
        retracted: Option<DecisionId>,
    },
    // 11 to 13
    /// Unread notes.
    Inbox {
        /// How many.
        count: u32,
    },
    /// An agent asked the caller. 1.8 (§13, §20.9 rule 11): `question`, whose addressee the `why`
    /// names, since it may be many sessions; `later`, when a write delivered it: "once you know the
    /// answer".
    AgentAsked {
        /// Who asked.
        by: SessionId,
        /// Until when it can be answered.
        until: Time,
        /// The question.
        #[cfg_attr(feature = "serde", serde(default))]
        question: QuestionId,
        /// A write delivered it: the answer is `later`.
        #[cfg_attr(feature = "serde", serde(default))]
        later: bool,
    },
    /// The human's pending items.
    HumanItems {
        /// The principal.
        principal: PrincipalId,
        /// Pending questions.
        questions: u32,
        /// Pending digest entries.
        digest: u32,
    },
    /// Tidy a flagged node.
    Tidy {
        /// The node.
        node: NodeId,
        /// Its flags.
        flags: FlagSet,
    },
    /// 1.9 (§10.3, rule 13): a stale offer anywhere in the forum, for an idle worker; 1.10: the
    /// question for its owners, "stale offer: ask its owners if it is still wanted; if you know it is
    /// not, close it as dropped" (1.9's take "to refresh it with a checkpoint" let a worker refresh
    /// offers with no news). `cut`: the forum's tidy scan stopped short (§18), so `total` is `≥k`.
    StaleOffer {
        /// The node.
        node: NodeId,
        /// The scan was cut.
        cut: bool,
    },
    /// 1.10 (§16, rule 13): a stale offer whose question for the user has its answer, or its
    /// default: "e12 has its answer for this stale offer: take it; close it, or record why it
    /// stays", a take with news.
    StaleAnswered {
        /// The node.
        node: NodeId,
        /// The question, answered or defaulted.
        question: QuestionId,
        /// The scan was cut.
        cut: bool,
    },
    /// 1.9 (§16, rule 13): possible duplicates, two open nodes under one parent that share rare
    /// words (`words::shared_rare`), for an idle worker: "possible duplicates: compare them, then
    /// link them as duplicates or merge".
    Alike {
        /// The node a merge would merge away.
        from: NodeId,
        /// The node that would remain.
        to: NodeId,
        /// The scan was cut.
        cut: bool,
    },
    /// Take the node just joined.
    Joined,
    /// At hello: "unless you came for a message or an event"; elsewhere, before 1.10, "if you have
    /// time for more".
    MoreWork {
        /// At `hello`.
        at_hello: bool,
    },
    /// 1.10 (rule 13): after any call but a `hello`, what `next` would give: "work that fits your
    /// caps is waiting: #46", or, for a routing item, "a message waits for its routing: e9". 1.8's
    /// "if you have time for more" let workers end while work that fitted their caps waited.
    WorkWaiting {
        /// `next`'s first candidate.
        first: crate::state::Cand,
    },
}

impl Because {
    /// 1.9 (§18, rule 13): the item comes from the forum's tidy scan, which was cut, so the items
    /// are not all there are and `total` is `≥k`.
    #[must_use]
    pub const fn cut(&self) -> bool {
        matches!(
            self,
            Because::StaleOffer { cut: true, .. }
                | Because::StaleAnswered { cut: true, .. }
                | Because::Alike { cut: true, .. }
        )
    }
}

/// The next calls of a response (§20.9), on the state after the call and its post-settle; pure,
/// like every query: never called by `apply`, `settle`, or `evolve`. It computes with
/// `CHECKPOINT_EVERY` and `LEASE_MARGIN` (§22), whatever the features.
#[must_use]
pub fn guide<M: Meter>(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    m: &mut M,
) -> Guidance {
    guide_with(s, sess, now, cmd, out, None, m)
}

/// `guide` with the first row of the call's `related`, which the shell's search found (1.11,
/// §20.9 "Inputs"): rule 5's create names it in its `why` for the entry the call logged when it is
/// an open work node (`related_target`, 1.12), where 1.11 routed the entry there.
#[must_use]
pub fn guide_with<M: Meter>(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
    m: &mut M,
) -> Guidance {
    let items = items_with(
        s,
        sess,
        now,
        cmd,
        out,
        related,
        m,
        CHECKPOINT_EVERY,
        LEASE_MARGIN,
    );
    let mut g = finish(s, cmd, items);
    g.end = end_of(s, sess, now, cmd, out, &g, m);
    g
}

/// 1.10 (§20.9 rule E): a `session hello` refused for a `from_user` that names no principal in a
/// session that serves none (§8.1, 1.9). Its session is open, with the call's intent, caps, and
/// horizon, or was before, and only the `from_user` failed: it gets a `hello`'s next calls from
/// rule 13, and its `end`. In the lab's fifth round (it4), all six such refusals, of workers that
/// logged their scheduler's prompt, carried `next_calls: none`.
#[must_use]
pub fn hello_refused_unnamed(cmd: &Cmd, out: &Result<Done, Error>) -> bool {
    matches!(cmd, Cmd::Session(SessionCmd::Hello { .. }))
        && matches!(out, Err(e) if e.hint == Hint::Invalid(Why::NoPrincipal))
}

/// `end` (1.7, §20.9): with no item, after a `hold next` that found nothing and whose
/// `leave_better` is empty, or after a `session hello` with no offer (`next` would offer the
/// session nothing), 1.10: or one refused for a `from_user` that names no principal
/// ([`hello_refused_unnamed`]); 1.8 (rule 13): after a `hold release`, `talk resolve`, or `node
/// reopen` by a worker that holds no live hold, when `next` would offer it nothing.
pub fn end_of<M: Meter>(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    g: &Guidance,
    m: &mut M,
) -> Option<End> {
    if !g.items.is_empty() {
        return None;
    }
    let x = sess.filter(|x| s.sessions.contains_key(x))?;
    if hello_refused_unnamed(cmd, out) {
        let (first, _) = super::ranked(s, x, now, &NextQuery::default(), 1, m);
        return first.is_empty().then_some(End::AtHello);
    }
    match out.as_ref().ok().map(|d| &d.reply) {
        Some(Reply::Next(NextReply::Empty { leave_better, .. })) if leave_better.is_empty() => {
            Some(End::AfterNext)
        }
        Some(Reply::Hello { .. }) => {
            let (first, _) = super::ranked(s, x, now, &NextQuery::default(), 1, m);
            first.is_empty().then_some(End::AtHello)
        }
        Some(Reply::Released { .. } | Reply::ObjectionResolved { .. } | Reply::Reopened { .. })
            if idle_worker(s, x, now) =>
        {
            let (first, _) = super::ranked(s, x, now, &NextQuery::default(), 1, m);
            first.is_empty().then_some(End::Idle)
        }
        _ => None,
    }
}

/// A worker (§20.9: it serves no principal) that holds no live hold at `now` (1.8, rule 13).
#[must_use]
pub fn idle_worker(s: &State, x: SessionId, now: Time) -> bool {
    s.sessions.get(&x).is_some_and(|y| y.principals.is_empty())
        && crate::helpers::holds::live_holds_of(s, x, now).is_empty()
}

/// Only with the test-only feature `conformance` (D.10.8): the items of D.8.26 before the cut of
/// `finish` keeps `NEXT_CALLS_MAX`, after its dedupe, computed with the model's ticks,
/// `CHECKPOINT_EVERY_TICKS` and `LEASE_MARGIN_TICKS` (D.10.5), where `guide` uses §22's values.
#[cfg(feature = "conformance")]
#[must_use]
pub fn guide_items<M: Meter>(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    m: &mut M,
) -> Vec<NextCall> {
    let items = items_with(
        s,
        sess,
        now,
        cmd,
        out,
        None,
        m,
        CHECKPOINT_EVERY_TICKS,
        LEASE_MARGIN_TICKS,
    );
    dedupe(s, cmd, partition(items))
}

/// Every item of D.8.26 after `finish`'s partition and dedupe, before its cut: what G1 (a) reads
/// to tell whether a one-shot item was cut (`check_guide`), and what the tests count; and whether
/// an item, kept or deduped, came from a tidy scan that was cut (1.9, §18).
#[must_use]
pub fn all_items(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
) -> (Vec<NextCall>, bool) {
    let items = items_with(
        s,
        sess,
        now,
        cmd,
        out,
        related,
        &mut super::NoMeter,
        CHECKPOINT_EVERY,
        LEASE_MARGIN,
    );
    let cut = items.iter().any(|i| i.because.cut());
    (dedupe(s, cmd, partition(items)), cut)
}

// ------------------------------------------------------------------------------------------------
// finish (D.8.26)

/// The stable partition of `finish`: the `now` items, then the `later` ones, each in emission
/// order (ascending rule number).
fn partition(items: Vec<NextCall>) -> Vec<NextCall> {
    let (mut now, later): (Vec<NextCall>, Vec<NextCall>) =
        items.into_iter().partition(|i| i.when == Moment::Now);
    now.extend(later);
    now
}

/// `finish`'s dedupe: drops an item that is `same_call` as an earlier one, or as the call just
/// made.
fn dedupe(s: &State, cmd: &Cmd, items: Vec<NextCall>) -> Vec<NextCall> {
    let mut kept: Vec<NextCall> = Vec::with_capacity(items.len());
    for it in items {
        if kept.iter().any(|k| same_skeleton(&k.call, &it.call)) || same_call(s, &it.call, cmd) {
            continue;
        }
        kept.push(it);
    }
    kept
}

/// `finish` (D.8.26): the partition, the dedupe, `total`, and the cut at `NEXT_CALLS_MAX`, where a
/// one-shot item beyond the cut takes the last place when none of the kept ones is one-shot. 1.9
/// (§18): when an item comes from a tidy scan that was cut, `total` is `≥k`.
#[must_use]
pub fn finish(s: &State, cmd: &Cmd, items: Vec<NextCall>) -> Guidance {
    let cut = items.iter().any(|i| i.because.cut());
    let all = dedupe(s, cmd, partition(items));
    let total = total_of(all.len(), cut);
    let mut shown: Vec<NextCall> = all.iter().take(NEXT_CALLS_MAX).cloned().collect();
    if !shown.iter().any(|i| i.once)
        && let Some(first) = all.iter().skip(NEXT_CALLS_MAX).find(|i| i.once)
    {
        shown.pop();
        shown.push(first.clone());
    }
    Guidance {
        items: shown,
        total,
        end: None,
    }
}

/// `total`: exact below `NEXT_TOTAL_CAP`, `≥20` from it (§18); 1.9: `≥n` below it when a tidy
/// scan was cut.
#[must_use]
pub fn total_of(n: usize, cut: bool) -> Total {
    let cap = usize::try_from(NEXT_TOTAL_CAP).unwrap_or(usize::MAX);
    if n >= cap {
        Total::AtLeast(NEXT_TOTAL_CAP)
    } else if cut {
        Total::AtLeast(u32::try_from(n).unwrap_or(NEXT_TOTAL_CAP))
    } else {
        Total::Exact(u32::try_from(n).unwrap_or(NEXT_TOTAL_CAP))
    }
}

/// The skeleton with the call's argument names and non-text values only: the remedy of
/// `invalid(IntentOpen)` is the checkpoint that closes the open intent, whose `intent` is optional.
fn canon(sk: &Skeleton) -> Skeleton {
    match sk {
        Skeleton::CheckpointSwap { token } => Skeleton::Checkpoint {
            token: *token,
            intent_done: true,
            intent: false,
        },
        Skeleton::Hello
        | Skeleton::LogFromUser { .. }
        | Skeleton::ViewNode { .. }
        | Skeleton::Checkpoint { .. }
        | Skeleton::Release { .. }
        | Skeleton::Take { .. }
        | Skeleton::Next
        | Skeleton::CreateFor { .. }
        | Skeleton::Split { .. }
        | Skeleton::AddOffer { .. }
        | Skeleton::Close { .. }
        | Skeleton::SayTo { .. }
        | Skeleton::ToUser { .. }
        | Skeleton::Said { .. }
        | Skeleton::Heard { .. }
        | Skeleton::Decide { .. }
        | Skeleton::Route { .. }
        | Skeleton::AnswerUser { .. }
        | Skeleton::AnswerAgent { .. }
        | Skeleton::Resolve { .. }
        | Skeleton::Merge { .. }
        | Skeleton::AskOwners { .. }
        | Skeleton::LinkDuplicate { .. }
        | Skeleton::Wait { .. }
        | Skeleton::Reopen { .. } => sk.clone(),
    }
}

/// Two items describe the same call: tool, action, argument names, and non-text values (§20.9).
#[must_use]
pub fn same_skeleton(a: &Skeleton, b: &Skeleton) -> bool {
    canon(a) == canon(b)
}

/// `same_call` (D.8.26, G1 (c)): the item describes the call just made, with the same tool,
/// action, argument names, and non-text values. A `session hello` item, the remedy of a refused
/// session, carries no `session`, and the call it answers carried one, so it is never the call
/// just made. A value the item leaves to the agent (a `fill`,
/// such as a close's outcome when the skeleton has none, or a route's `to`) matches any value; an
/// optional fill (`basis?`) matches its presence or absence.
#[must_use]
pub fn same_call(s: &State, sk: &Skeleton, cmd: &Cmd) -> bool {
    let node = |r: &NodeRef| super::resolve(s, r);
    let pid = |k: &PrincipalKey| s.principal_ids.get(k).copied();
    match (canon(sk), cmd) {
        (
            Skeleton::LogFromUser { principal },
            Cmd::Talk(TalkCmd::FromUser {
                principal: Some(p),
                answers: None,
                to: None,
                clarifies: None,
                ..
            }),
        ) => pid(p) == Some(principal),
        (
            Skeleton::AnswerUser {
                question,
                principal,
            },
            Cmd::Talk(TalkCmd::FromUser {
                principal: p,
                answers: Some(q),
                to: None,
                clarifies: None,
                ..
            }),
        ) => *q == question && p.as_ref().and_then(pid) == principal,
        (
            Skeleton::ViewNode { node: n },
            Cmd::View(ViewCmd::Node {
                targets,
                view: None,
                page: None,
            }),
        ) => targets.len() == 1 && targets.first().and_then(node) == Some(n),
        (
            Skeleton::Checkpoint {
                token,
                intent_done,
                intent,
            },
            Cmd::Hold(HoldCmd::Checkpoint {
                token: t,
                intent: i,
                intent_done: d,
                files,
                ..
            }),
        ) => *t == token && i.is_some() == intent && d.is_some() == intent_done && files.is_empty(),
        (
            Skeleton::Release { token, wait },
            Cmd::Hold(HoldCmd::Release {
                token: t,
                reason,
                waits_for,
                intent_done: None,
                learnings: None,
                unsent,
                ..
            }),
        ) => {
            *t == token
                && unsent.is_empty()
                && match (wait, waits_for) {
                    (Some(q), Some(WaitArg::Question(w))) => {
                        *w == q && *reason == ReleaseReason::Blocked
                    }
                    (None, None) => *reason == ReleaseReason::Handoff,
                    (Some(_) | None, Some(_) | None) => false,
                }
        }
        (
            Skeleton::Take { target },
            Cmd::Hold(HoldCmd::Take {
                target: r,
                lease: None,
            }),
        ) => node(r) == Some(target),
        (Skeleton::Next, Cmd::Hold(HoldCmd::Next { query, lease: None })) => {
            *query == NextQuery::default()
        }
        (Skeleton::CreateFor { entry }, Cmd::Node(NodeCmd::Create(a))) => {
            plain_create(a)
                && a.take.is_none()
                && a.tags.is_empty()
                && a.keys.len() == 1
                && a.keys.first().and_then(|k| k.key.user_entry()) == Some(entry)
        }
        (Skeleton::Split { parent }, Cmd::Node(NodeCmd::Create(a))) => {
            plain_create(a)
                && a.keys.is_empty()
                && a.take == Some(false)
                && a.tags.len() == 1
                && a.tags.iter().all(|t| t.as_str() == OFFER)
                && parent_of_path(s, &a.path) == Some(parent)
        }
        (
            Skeleton::AddOffer { node: n, version },
            Cmd::Node(NodeCmd::Update {
                at,
                version: v,
                digest_version: None,
                token: None,
                patch,
            }),
        ) => {
            node(at) == Some(n)
                && *v == version
                && patch.tags_remove.is_empty()
                && patch.tags_add.len() == 1
                && patch.tags_add.iter().all(|t| t.as_str() == OFFER)
                && *patch
                    == Patch {
                        tags_add: patch.tags_add.clone(),
                        ..Patch::default()
                    }
        }
        (
            Skeleton::Close {
                node: n,
                token,
                settles,
                outcome,
            },
            Cmd::Node(NodeCmd::Close {
                at,
                outcome: o,
                verified: None,
                settles: st,
                learnings: None,
                token: Some(t),
                unsent,
                ..
            }),
        ) => {
            node(at) == Some(n)
                && *t == token
                && unsent.is_empty()
                // 1.7: an item without `settles` leaves it out, the default of §5.
                && match st {
                    None => settles.is_empty(),
                    Some(st) => !settles.is_empty() && st.iter().copied().eq(settles.iter().copied()),
                }
                && outcome.as_ref().is_none_or(|x| x == o)
        }
        (
            Skeleton::SayTo { at, to },
            Cmd::Talk(TalkCmd::Say {
                at: a,
                to: Some(AddresseeArg::Session(h)),
                expects: None,
                reply_to: None,
                until: None,
                ..
            }),
        ) => node(a) == Some(at) && *h == to,
        (
            Skeleton::ToUser {
                principal,
                kind,
                re,
            },
            Cmd::Talk(TalkCmd::ToUser {
                kind: k,
                principal: Some(p),
                re: r,
                ..
            }),
        ) => {
            pid(p) == Some(principal)
                && kind.is_none_or(|x| x == *k)
                && r.iter().copied().eq(re.iter().copied())
        }
        (
            Skeleton::Said {
                at,
                kind,
                where_,
                where_fill,
            },
            Cmd::Talk(TalkCmd::Said {
                at: a,
                where_: w,
                kind: k,
                corrects: None,
                seen: None,
                ..
            }),
        ) => {
            node(a) == Some(at)
                && *k == kind
                && (where_fill || w.as_ref().map(|x| &x.key) == where_.as_ref())
        }
        (
            Skeleton::Heard {
                at,
                answers,
                where_,
            },
            Cmd::Talk(TalkCmd::Heard {
                at: a,
                where_: Some(w),
                answers: Some(q),
                ..
            }),
        ) => node(a) == Some(at) && *q == answers && w.key == where_,
        (
            Skeleton::Decide {
                at,
                settles,
                supersedes,
            },
            Cmd::Talk(TalkCmd::Decide {
                at: a,
                settles: st,
                supersedes: sp,
                evidence: None,
                ..
            }),
        ) => {
            node(a) == Some(at)
                && st.iter().copied().eq(settles.iter().copied())
                && *sp == supersedes
        }
        (
            Skeleton::Route { entry, to },
            Cmd::Talk(TalkCmd::Route {
                entry: e,
                to: t,
                answers,
                clarifies,
                ..
            }),
        ) => {
            *e == entry
                && match to {
                    RouteSk::Fill => true,
                    RouteSk::Nowhere => {
                        answers.is_none()
                            && clarifies.is_none()
                            && matches!(t, Some(crate::cmd::RouteTo::Nowhere))
                    }
                    RouteSk::Node(n) => {
                        answers.is_none()
                            && clarifies.is_none()
                            && t.as_ref()
                                .and_then(crate::cmd::RouteTo::node)
                                .and_then(node)
                                == Some(n)
                    }
                    RouteSk::Answers(q) => *answers == Some(q),
                    RouteSk::Clarifies(r) => answers.is_none() && *clarifies == Some(r),
                }
        }
        (Skeleton::AnswerAgent { question }, Cmd::Talk(TalkCmd::Answer { question: q, .. })) => {
            *q == question
        }
        (Skeleton::Resolve { objection }, Cmd::Talk(TalkCmd::Resolve { objection: o, .. })) => {
            *o == objection
        }
        (Skeleton::Merge { from, into }, Cmd::Node(NodeCmd::Merge { from: f, into: i })) => {
            node(f) == Some(from) && node(i) == Some(into)
        }
        // 1.10: a question for the user about the stale offer, of the human form, by `user`.
        (
            Skeleton::AskOwners { at },
            Cmd::Talk(TalkCmd::Ask {
                at: a,
                to: AskTo::User(u),
                again: false,
                ..
            }),
        ) => node(a) == Some(at) && u.to.is_none() && u.due.is_none() && u.recommendation.is_none(),
        (
            Skeleton::LinkDuplicate { from, to },
            Cmd::Node(NodeCmd::Link {
                at,
                rel: crate::types::LinkRel::Duplicate,
                to: t,
            }),
        ) => node(at) == Some(from) && node(t) == Some(to),
        (
            Skeleton::Wait { timeout },
            Cmd::Talk(TalkCmd::Wait {
                timeout: t,
                for_: None,
                ..
            }),
        ) => *t == timeout,
        // 1.12 (rule 9): the reopen of a claim nothing proves.
        (Skeleton::Reopen { node: n }, Cmd::Node(NodeCmd::Reopen { at, .. })) => {
            node(at) == Some(n)
        }
        (
            Skeleton::Hello
            | Skeleton::LogFromUser { .. }
            | Skeleton::ViewNode { .. }
            | Skeleton::Checkpoint { .. }
            | Skeleton::CheckpointSwap { .. }
            | Skeleton::Release { .. }
            | Skeleton::Take { .. }
            | Skeleton::Next
            | Skeleton::CreateFor { .. }
            | Skeleton::Split { .. }
            | Skeleton::AddOffer { .. }
            | Skeleton::Close { .. }
            | Skeleton::SayTo { .. }
            | Skeleton::ToUser { .. }
            | Skeleton::Said { .. }
            | Skeleton::Heard { .. }
            | Skeleton::Decide { .. }
            | Skeleton::Route { .. }
            | Skeleton::AnswerUser { .. }
            | Skeleton::AnswerAgent { .. }
            | Skeleton::Resolve { .. }
            | Skeleton::Merge { .. }
            | Skeleton::AskOwners { .. }
            | Skeleton::LinkDuplicate { .. }
            | Skeleton::Wait { .. }
            | Skeleton::Reopen { .. },
            _,
        ) => false,
    }
}

/// A `create` with no argument but `path`, `keys`, `title`, `tags`, and `take`.
fn plain_create(a: &crate::cmd::CreateArgs) -> bool {
    a.kind.is_none()
        && a.body.is_none()
        && a.digest.is_none()
        && a.fields.is_empty()
        && a.capacity.is_none()
        && a.needs.is_none()
        && a.est.is_none()
        && a.priority.is_none()
        && a.due.is_none()
        && a.depends_on.is_empty()
        && a.owners.is_empty()
        && a.repeat.is_none()
        && a.agreements.is_none()
        && a.request.is_none()
}

/// The node a path's parent resolves to: ROOT for a one-step path.
fn parent_of_path(s: &State, p: &Path) -> Option<NodeId> {
    let mut steps = p.0.clone();
    steps.pop()?;
    super::resolve(s, &NodeRef::Path(Path(steps)))
}

// ------------------------------------------------------------------------------------------------
// The rules (D.8.26)

/// Whether a response reaches nothing that next calls follow (§20.9 "Which responses"): a repeat
/// answered from its `op_id` record (the shell returns the stored response, O1), and the answers
/// that reach nothing else, `limit` for a text over `text.max`, the writer queue, and
/// `write.per_hour` (§21).
fn quiet(out: &Result<Done, Error>) -> bool {
    match out {
        Ok(d) => matches!(d.reply, Reply::Replay),
        Err(e) => matches!(
            e.hint,
            Hint::Limit(LimitKind::TextTooLong | LimitKind::Busy | LimitKind::WritesPerHour)
        ),
    }
}

/// The refusals of the session check (§21): their remedy, `session hello`, is the only item.
fn session_refused(out: &Result<Done, Error>) -> bool {
    matches!(
        out,
        Err(Error {
            hint: Hint::NotFound(What::Session) | Hint::Invalid(Why::ClassMismatch),
            ..
        })
    )
}

/// One item.
fn item(rule: Rule, when: Moment, once: bool, call: Skeleton, because: Because) -> NextCall {
    NextCall {
        rule,
        when,
        once,
        call,
        because,
    }
}

/// The ids of a list cut at `NEXT_IDS_MAX`, the oldest kept.
fn cut<T: Copy>(xs: impl IntoIterator<Item = T>) -> Vec<T> {
    xs.into_iter().take(NEXT_IDS_MAX).collect()
}

/// One routing of rule 5, in its order of preference (1.7, §20.9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RouteChoice {
    /// `answers: <question>`.
    Answers(QuestionId),
    /// `to: none`, after a `to_user` that named the entry.
    Nowhere,
    /// `to: <node>`, the session's only hold (true) or the pending question's node.
    Node(NodeId, bool),
    /// 1.11: `clarifies: <request>`, the default's clarification on the session's only hold.
    Clarifies(RequestId),
    /// `node create` by key; 1.12: with the open work node the call's `related` named first, which
    /// its `why` names.
    Create(Option<NodeId>),
}

/// Rule 5's one item for entry `e` (1.9): the first of `choices`, with the second as the
/// alternative its `why` names; ROUTE_ITEMS is 1.
fn route_item(e: UserEntryId, choices: &[RouteChoice]) -> Option<NextCall> {
    let first = *choices.first()?;
    let or = choices.get(1).and_then(|c| match *c {
        RouteChoice::Answers(_) | RouteChoice::Clarifies(_) => None,
        RouteChoice::Nowhere => Some(RouteAlt::Nowhere),
        RouteChoice::Node(node, held) => Some(RouteAlt::Node { node, held }),
        RouteChoice::Create(_) => Some(RouteAlt::Create),
    });
    let route = |to: RouteSk, because: Because| {
        item(
            Rule::Route,
            Moment::Now,
            false,
            Skeleton::Route { entry: e, to },
            because,
        )
    };
    Some(match first {
        RouteChoice::Answers(q) => route(
            RouteSk::Answers(q),
            Because::RouteAnswers {
                entry: e,
                question: q,
                or,
            },
        ),
        RouteChoice::Nowhere => route(RouteSk::Nowhere, Because::AnsweredEntry { entry: e, or }),
        RouteChoice::Node(n, held) => route(
            RouteSk::Node(n),
            Because::RouteNode {
                entry: e,
                node: n,
                held,
                or,
            },
        ),
        RouteChoice::Clarifies(r) => route(
            RouteSk::Clarifies(r),
            Because::RouteClarifies {
                entry: e,
                request: r,
                or,
            },
        ),
        RouteChoice::Create(related) => item(
            Rule::Route,
            Moment::Now,
            false,
            Skeleton::CreateFor { entry: e },
            Because::Unrouted { entry: e, related },
        ),
    })
}

/// How many of a node's requests rules 6 and 7 read in one response, oldest first: D.8.26's
/// "every scan stops at NEXT_TOTAL_CAP items" (§18). Rule 6 reads that many of the requests owed
/// a reply on each node whose every request is in reach, and rule 7 that many of the unsettled
/// requests of a node it reads whole; `spec::guide` reads the same prefixes, so the two agree.
#[must_use]
pub(crate) fn request_scan() -> usize {
    usize::try_from(NEXT_TOTAL_CAP).unwrap_or(usize::MAX)
}

/// The items of D.8.26 in emission order, before `finish`.
#[allow(
    clippy::too_many_arguments,
    reason = "guide's inputs and the two durations the conformance feature scales"
)]
fn items_with<M: Meter>(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
    m: &mut M,
    every: Dur,
    margin: Dur,
) -> Vec<NextCall> {
    let mut v = Vec::new();
    if quiet(out) {
        return v;
    }
    if session_refused(out) {
        v.push(item(
            Rule::Remedy,
            Moment::Now,
            true,
            Skeleton::Hello,
            Because::NoSession,
        ));
        return v;
    }
    let Some(x) = sess.filter(|x| s.sessions.contains_key(x)) else {
        return v;
    };
    let mut g = G::new(s, x, now, cmd, out, every, margin);
    g.related = related;
    m.tick(u64::try_from(g.holds.len()).unwrap_or(u64::MAX));
    if let Err(e) = out {
        v.extend(g.remedy(e));
    }
    let ok = out.is_ok();
    if ok {
        v.extend(g.rule1());
    }
    v.extend(g.rule2());
    // 1.7: after a refusal too, as rule E keeps rules 2 to 8: on a held node, rule 8's close now
    // records the decision rule 7 gave before (§5). 1.12's review: decided before rule 3, whose
    // due checkpoint of a hold whose node it closes now is `later`.
    let closes = g.rule8(m);
    v.extend(g.rule3(&closes));
    v.extend(g.rule4(m));
    v.extend(g.rule5());
    let closing: BTreeSet<NodeId> = closes
        .iter()
        .filter_map(|i| {
            if let Skeleton::Close { node, .. } = &i.call {
                Some(*node)
            } else {
                None
            }
        })
        .collect();
    v.extend(g.rule6(m));
    v.extend(g.rule7(&closing, m));
    v.extend(closes);
    if ok {
        v.extend(g.rule9());
    }
    v.extend(g.rule10());
    v.extend(g.rule11());
    if ok {
        v.extend(g.rule12(m));
    }
    v.extend(g.rule13(m));
    v
}

/// The inputs of one computation: the state, the calling session `x`, its live holds `H` in token
/// order, the call, and its outcome.
struct G<'a> {
    s: &'a State,
    x: SessionId,
    now: Time,
    cmd: &'a Cmd,
    out: &'a Result<Done, Error>,
    /// The reply, when the call succeeded.
    ok: Option<&'a Reply>,
    /// `H`: the session's live holds, in token order.
    holds: Vec<(Token, &'a Hold)>,
    every: Dur,
    margin: Dur,
    /// 1.11 (rule 5): the first row of the call's `related`.
    related: Option<NodeId>,
}

impl<'a> G<'a> {
    fn new(
        s: &'a State,
        x: SessionId,
        now: Time,
        cmd: &'a Cmd,
        out: &'a Result<Done, Error>,
        every: Dur,
        margin: Dur,
    ) -> G<'a> {
        let holds =
            s.ix.holds_by_session
                .get(&x)
                .into_iter()
                .flatten()
                .filter_map(|t| s.holds.get(t).filter(|h| live(h, now)).map(|h| (*t, h)))
                .collect();
        G {
            s,
            x,
            now,
            cmd,
            out,
            ok: out.as_ref().ok().map(|d| &d.reply),
            holds,
            every,
            margin,
            related: None,
        }
    }

    /// The open nodes of `H`.
    fn held_open(&self) -> BTreeSet<NodeId> {
        self.holds
            .iter()
            .map(|(t, _)| t.node)
            .filter(|n| self.s.nodes.get(n).is_some_and(Node::is_open))
            .collect()
    }

    /// The session's hold on `n`, if any.
    fn hold_on(&self, n: NodeId) -> Option<Token> {
        self.holds.iter().map(|(t, _)| *t).find(|t| t.node == n)
    }

    /// Whether another session holds `n` live and this one does not (1.8, §20.9 "Another's
    /// node"): its requests are not in reach, and no item closes it or merges it away.
    fn others(&self, n: NodeId) -> bool {
        self.hold_on(n).is_none()
            && crate::helpers::holds::held_by_another(self.s, n, Some(self.x), self.now)
    }

    /// The hold this call issued.
    fn issued(&self) -> Option<Token> {
        match self.ok {
            Some(Reply::Created { token, .. }) => *token,
            Some(
                Reply::Taken {
                    token,
                    existing: false,
                    ..
                }
                | Reply::Next(NextReply::Hold { token, .. }),
            ) => Some(*token),
            _ => None,
        }
    }

    /// Whether the call returned this warning.
    fn warned(&self, code: WarningCode) -> bool {
        self.out
            .as_ref()
            .is_ok_and(|d| d.warnings.iter().any(|w| w.code == code))
    }

    /// The requests the reply says this call settled (1.12: a release's `settles` too, §10.3).
    fn settled_by_out(&self) -> Vec<RequestId> {
        match self.ok {
            Some(
                Reply::Said { settled, .. }
                | Reply::ToUser { settled, .. }
                | Reply::Decided { settled, .. }
                | Reply::Closed { settled, .. }
                | Reply::Released { settled, .. },
            ) => settled.clone(),
            _ => Vec::new(),
        }
    }

    /// The node this call closed.
    fn closed_node(&self) -> Option<NodeId> {
        match self.ok {
            Some(Reply::Closed { node, .. }) => Some(*node),
            _ => None,
        }
    }

    /// The requests the call names (`settles` of decide, close, and, 1.12, release, §10.3; `re`
    /// of `to_user`, directly or through a user entry routed as a request), resident ones only.
    fn named(&self) -> BTreeSet<RequestId> {
        let s = self.s;
        let mut out = BTreeSet::new();
        if let Cmd::Talk(TalkCmd::Decide { settles, .. })
        | Cmd::Hold(HoldCmd::Release { settles, .. }) = self.cmd
        {
            out.extend(
                settles
                    .iter()
                    .copied()
                    .filter(|r| s.requests.contains_key(r)),
            );
        }
        if let Cmd::Node(NodeCmd::Close {
            settles: Some(settles),
            ..
        }) = self.cmd
        {
            out.extend(
                settles
                    .iter()
                    .copied()
                    .filter(|r| s.requests.contains_key(r)),
            );
        }
        if let Cmd::Talk(TalkCmd::ToUser { re, .. }) = self.cmd {
            for e in re {
                if s.requests.contains_key(e) {
                    out.insert(*e);
                } else if let Some(UserEntry {
                    routing: Routing::Request { request, .. },
                    ..
                }) = s.user_entries.get(e)
                    && s.requests.contains_key(request)
                {
                    out.insert(*request);
                }
            }
        }
        // 1.8 (§8.1): the requests `to_user {at}` named in `re`.
        if let Some(Reply::ToUser { re, .. }) = self.ok {
            out.extend(re.iter().copied().filter(|r| s.requests.contains_key(r)));
        }
        out
    }

    /// `recorded(out)` (D.8.26): the requests the reply names as recorded by this call.
    fn recorded(&self) -> BTreeSet<RequestId> {
        let mut out = BTreeSet::new();
        let routed = |r: &RoutedReply, out: &mut BTreeSet<RequestId>| {
            if let RoutedReply::Request { request, .. } = r {
                out.insert(*request);
            }
        };
        match self.ok {
            Some(Reply::FromUser { routed: r, .. }) => routed(r, &mut out),
            Some(Reply::Hello {
                user_entry: Some(u),
                ..
            }) => routed(&u.routed, &mut out),
            Some(Reply::Routed {
                routed: r,
                existing: false,
                ..
            }) => routed(r, &mut out),
            Some(Reply::Request {
                request,
                existing: false,
                ..
            }) => {
                out.insert(*request);
            }
            Some(
                Reply::Created {
                    request,
                    user_requests,
                    ..
                }
                | Reply::Joined {
                    request,
                    user_requests,
                    ..
                },
            ) => {
                out.extend(request.iter().copied());
                out.extend(user_requests.iter().copied());
            }
            Some(Reply::KeyAdded {
                user_request: Some(r),
                ..
            }) => {
                out.insert(*r);
            }
            _ => {}
        }
        out.retain(|r| self.s.requests.contains_key(r));
        out
    }

    /// The node `n`'s open subsets.
    fn items(&self, n: NodeId) -> Option<&'a NodeItems> {
        self.s.ix.items.get(&n)
    }

    /// The node `n`'s flags.
    fn flags(&self, n: NodeId) -> FlagSet {
        self.s.ix.node.get(&n).map_or(FlagSet::EMPTY, |i| i.flags)
    }

    /// The validation context of a call by `x` (for the merge of rule 10).
    fn ctx(&self) -> Ctx {
        Ctx {
            now: self.now,
            class: self
                .s
                .sessions
                .get(&self.x)
                .map_or_else(ClassName::unknown, |x| x.class.clone()),
            caps: None,
            session: SessionCtx::Existing(self.x),
            op: None,
        }
    }

    // --------------------------------------------------------------------------------------------
    // E

    /// The remedy of §21's table, if the session can make it now.
    fn remedy(&self, e: &Error) -> Option<NextCall> {
        let s = self.s;
        let x = self.x;
        let now = self.now;
        let view_or_take = |n: NodeId| {
            if crate::helpers::holds::can_take(s, x, n, now) {
                Skeleton::Take { target: n }
            } else {
                Skeleton::ViewNode { node: n }
            }
        };
        let reread = |n: NodeId| {
            let node = s.nodes.get(&n)?;
            Some((
                Skeleton::ViewNode { node: n },
                Because::Reread {
                    version: node.version,
                    digest_version: node.digest_version,
                },
            ))
        };
        let (call, because) = match &e.hint {
            Hint::StaleToken { token, late } => {
                let n = write_target(s, self.cmd).unwrap_or(token.node);
                if !s.nodes.contains_key(&n) {
                    return None;
                }
                (view_or_take(n), Because::HoldEnded { late: *late })
            }
            Hint::StaleVersion { .. } | Hint::Invalid(Why::MissingDigestVersion) => {
                reread(crate::apply::target(s, self.cmd).ok().flatten()?)?
            }
            Hint::Invalid(Why::VersionAhead) => reread(version_ahead(s, self.cmd)?)?,
            Hint::Closed {
                merged_into: Some(m),
                ..
            } => {
                if !s.nodes.contains_key(m) {
                    return None;
                }
                (view_or_take(*m), Because::MergedInto { node: *m })
            }
            // 1.9 (§10.1, §21): a take refused for an open objection on a closed node: its review.
            Hint::Closed {
                node,
                merged_into: None,
            } if matches!(self.cmd, Cmd::Hold(HoldCmd::Take { .. })) => {
                let o = review_objection(s, *node)?;
                (
                    Skeleton::Resolve { objection: o },
                    Because::Review { objection: o },
                )
            }
            Hint::Invalid(Why::IntentOpen) => {
                let Cmd::Hold(HoldCmd::Checkpoint { token, .. }) = self.cmd else {
                    return None;
                };
                if !crate::apply::token_live(s, *token, token.node, now) {
                    return None;
                }
                let i = s.nodes.get(&token.node)?.open_intent?;
                (
                    Skeleton::CheckpointSwap { token: *token },
                    Because::CloseOpenIntent {
                        by: i.session,
                        entry: i.eid,
                        own: i.session == x,
                    },
                )
            }
            Hint::Limit(LimitKind::Renew) => {
                let Cmd::Hold(HoldCmd::Renew { token, .. }) = self.cmd else {
                    return None;
                };
                if !crate::apply::token_live(s, *token, token.node, now) {
                    return None;
                }
                (
                    Skeleton::Checkpoint {
                        token: *token,
                        intent_done: false,
                        intent: false,
                    },
                    Because::RenewLimit,
                )
            }
            Hint::Invalid(
                Why::NotPeopleQuestion | Why::NotAgentQuestion | Why::NotUserQuestion,
            ) => {
                let q = answered_question(self.cmd)?;
                (answer_fits(s, x, q)?, Because::AnswerFits { question: q })
            }
            // The session check's refusals are answered before the session's rules (above); every
            // other refusal has no remedy: its hint names the argument or the cause (§21).
            Hint::Full { .. }
            | Hint::HoldsLimit { .. }
            | Hint::Paused { .. }
            | Hint::Cycle { .. }
            | Hint::Closed { .. }
            | Hint::OpIdConflict
            | Hint::Limit(_)
            | Hint::Invalid(_)
            | Hint::NotFound(_) => return None,
        };
        Some(item(Rule::Remedy, Moment::Now, true, call, because))
    }

    // --------------------------------------------------------------------------------------------
    // 1 to 5

    /// Rule 1: `session hello {principal}` without `from_user`.
    fn rule1(&self) -> Option<NextCall> {
        let Cmd::Session(SessionCmd::Hello {
            principal: Some(k),
            from_user: None,
            ..
        }) = self.cmd
        else {
            return None;
        };
        let p = *self.s.principal_ids.get(k)?;
        Some(item(
            Rule::LogFirst,
            Moment::Now,
            true,
            Skeleton::LogFromUser { principal: p },
            Because::LogFirst { principal: p },
        ))
    }

    /// The `said` this call recorded, if any.
    fn recorded_said(&self) -> Option<SaidId> {
        match self.ok {
            Some(
                Reply::Said { said, .. }
                | Reply::Asked {
                    said: Some(said), ..
                },
            ) => Some(*said),
            _ => None,
        }
    }

    /// Rule 2 (1.9): the `said` this call recorded, while another session holds its conversation
    /// (`conversation_held`): the `say` to that holder, who speaks by convention (§9.2; 1.10: a
    /// `talk said` recorded so counts, where 1.9 withheld it), as for an `ask`'s. No item asks to
    /// confirm a `said` (§9.2): a recorded `said` is taken as sent unless listed in `unsent`, and a
    /// close or release marks it sent with its author's last reply. In the lab's fourth round, a
    /// worker confirmed a reply it never sent on 1.8's `later` confirm. 1.11 (§9.2): a `said` its
    /// author recorded holding nothing is confirmed at once, and gets the item all the same; and
    /// none comes when no holder has the cap of the conversation's scheme and the caller has it:
    /// it is the conversation's voice.
    fn rule2(&self) -> Option<NextCall> {
        let s = self.s;
        let y = self.recorded_said()?;
        let said = s.saids.get(&y).filter(|x| {
            x.session == self.x
                && matches!(
                    x.state,
                    crate::state::SaidState::Pending
                        | crate::state::SaidState::Withheld
                        | crate::state::SaidState::Confirmed
                )
        })?;
        let c = said.node;
        let h = crate::helpers::agents::conversation_holder(s, c, self.x, &said.conv)?;
        Some(item(
            Rule::TellHolder,
            Moment::Now,
            true,
            Skeleton::SayTo { at: c, to: h },
            Because::TellHolder { holder: h },
        ))
    }

    /// Rule 3: for each hold, the first of: its checkpoint is due; its node has an open intent;
    /// this call issued it (with `uncovered_slot`, now, else later). 1.12's review: a due
    /// checkpoint is `later` while rule 8's item `closes` the hold's node now: made first, it would leave the
    /// requests that close settles unfinished (§20.9 Terms), and the close would not come back.
    fn rule3(&self, closes: &[NextCall]) -> Vec<NextCall> {
        let s = self.s;
        let issued = self.issued();
        let ok = self.ok.is_some();
        let mut v = Vec::new();
        for (t, h) in &self.holds {
            let n = t.node;
            let intent = s.nodes.get(&n).and_then(|x| x.open_intent);
            let by = due_by(h, self.every, self.margin);
            if self.now >= by {
                let closing = closes.iter().any(|c| {
                    c.when == Moment::Now
                        && matches!(c.call, Skeleton::Close { token, .. } if token == *t)
                });
                v.push(item(
                    Rule::Checkpoint,
                    if closing { Moment::Later } else { Moment::Now },
                    false,
                    Skeleton::Checkpoint {
                        token: *t,
                        intent_done: false,
                        intent: false,
                    },
                    Because::LeaseDue {
                        lease_until: h.lease_until,
                        at_ceiling: h.lease_until == h.taken_at.plus(h.terms.ceiling),
                        intent: intent.map(|i| i.eid),
                        closing,
                    },
                ));
            } else if let Some(i) = intent {
                let own = i.session == self.x;
                v.push(item(
                    Rule::IntentDone,
                    if own { Moment::Later } else { Moment::Now },
                    false,
                    Skeleton::Checkpoint {
                        token: *t,
                        intent_done: true,
                        intent: false,
                    },
                    Because::IntentOpen {
                        by: i.session,
                        entry: i.eid,
                        own,
                    },
                ));
            } else if ok && issued == Some(*t) {
                if self.warned(WarningCode::UncoveredSlot) {
                    v.push(item(
                        Rule::Checkpoint,
                        Moment::Now,
                        true,
                        Skeleton::Checkpoint {
                            token: *t,
                            intent_done: false,
                            intent: true,
                        },
                        Because::UncoveredSlot,
                    ));
                } else {
                    v.push(item(
                        Rule::Checkpoint,
                        Moment::Later,
                        true,
                        Skeleton::Checkpoint {
                            token: *t,
                            intent_done: false,
                            intent: false,
                        },
                        Because::CheckpointBy { by },
                    ));
                }
            }
        }
        v
    }

    /// Rule 4: entries by others on a held node since its take, or on its conversation node
    /// whenever recorded, past the session's `seen`.
    fn rule4<M: Meter>(&self, m: &mut M) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        for (t, h) in &self.holds {
            let n = t.node;
            let news = |node: NodeId, since: Time, m: &mut M| {
                let seen = s
                    .sessions
                    .get(&self.x)
                    .and_then(|x| x.shown.get(&node))
                    .copied()
                    .unwrap_or(Seq(0));
                news(s, node, seen, since, self.x, m)
            };
            if let Some(count) = news(n, h.taken_at, m) {
                v.push(item(
                    Rule::Reread,
                    Moment::Now,
                    false,
                    Skeleton::ViewNode { node: n },
                    Because::News { node: n, count },
                ));
            } else if let Some(c) = conversation_node_of(s, n)
                && c != n
                && let Some(count) = news(c, Time::ZERO, m)
            {
                v.push(item(
                    Rule::Reread,
                    Moment::Now,
                    false,
                    Skeleton::ViewNode { node: c },
                    Because::News { node: c, count },
                ));
            }
        }
        v
    }

    /// Rule 5: each of the session's unrouted entries, one item each (1.9), the first that applies
    /// of (1.7): its route as the answer to the question for the user pending for its person; its
    /// route to none after a `to_user` that named it, which no longer routes it (1.9); its route to
    /// the session's only hold, else to the node that question is about, open and not reserved;
    /// `node create` by key. The next one that applies is the item's alternative, named in its
    /// `why` (`RouteAlt`). 1.12: the create's `why` names the first row of the `related` of the
    /// call that logged the entry, when it is an open work node (`related_target`), beside the
    /// route to none; 1.11's route to it is gone.
    fn rule5(&self) -> Vec<NextCall> {
        let s = self.s;
        let named: BTreeSet<Eid> = match (self.ok, self.cmd) {
            (Some(_), Cmd::Talk(TalkCmd::ToUser { re, .. })) => re.clone(),
            _ => BTreeSet::new(),
        };
        let held = self.held_open();
        let only_hold = match held.iter().collect::<Vec<_>>().as_slice() {
            [n] => Some(**n),
            _ => None,
        };
        // 1.11: the first row of the `related` of the call that logged an entry; 1.12: named in the
        // create's `why`, never routed to.
        let logged = self.logged_entry();
        let related = self.related.filter(|n| related_target(s, *n));
        let mut v = Vec::new();
        for &e in s.ix.unrouted_by.get(&self.x).into_iter().flatten() {
            let principal = s.user_entries.get(&e).map(|u| u.principal);
            let pending = principal.and_then(|p| first_pending_for(s, p));
            let about = pending
                .and_then(|q| s.uquestions.get(&q))
                .and_then(|u| u.about)
                .filter(|n| !n.is_reserved() && s.nodes.get(n).is_some_and(Node::is_open));
            let mut choices: Vec<RouteChoice> = Vec::new();
            choices.extend(pending.map(RouteChoice::Answers));
            if named.contains(&e) {
                choices.push(RouteChoice::Nowhere);
            }
            match only_hold {
                Some(n) => {
                    // 1.11 (§8.2): the default's clarification, before the route to the hold.
                    let follow = principal.and_then(|p| {
                        crate::helpers::user::follow_up_of(
                            s,
                            n,
                            &crate::helpers::principals::PrincipalRef::Id(p),
                        )
                    });
                    choices.extend(follow.map(|(r, _)| RouteChoice::Clarifies(r)));
                    choices.push(RouteChoice::Node(n, true));
                }
                None => choices.extend(about.map(|n| RouteChoice::Node(n, false))),
            }
            // 1.12: the create, whose `why` names the call's first related row for the entry it
            // logged, when that is an open work node.
            choices.push(RouteChoice::Create(related.filter(|_| logged == Some(e))));
            v.extend(route_item(e, &choices));
        }
        v
    }

    /// 1.11 (rule 5): the `from_user` entry this call logged, `talk from_user`'s or `hello`'s.
    fn logged_entry(&self) -> Option<UserEntryId> {
        match self.ok {
            Some(Reply::FromUser { entry, .. }) => Some(*entry),
            Some(Reply::Hello {
                user_entry: Some(u),
                ..
            }) => Some(u.entry),
            _ => None,
        }
    }

    // --------------------------------------------------------------------------------------------
    // 6 to 8

    /// The nodes whose every request is in reach: the open nodes of `H` and the node the call
    /// closed.
    fn whole_reach(&self) -> BTreeSet<NodeId> {
        let mut out = self.held_open();
        out.extend(self.closed_node());
        out
    }

    /// The requests in reach on nodes outside `whole_reach`: named or recorded by the call, except
    /// those on a node another session holds and this one does not (1.8): that holder owes them.
    fn partial_reach(&self) -> BTreeSet<RequestId> {
        let mut out = self.named();
        out.extend(self.recorded());
        out.retain(|r| self.s.requests.get(r).is_none_or(|q| !self.others(q.node)));
        out
    }

    /// The misaddressed requests of rule 6: a `to_user {kind: result}` named them and did not
    /// answer them, because their requester is another person or outside the swarm; with the
    /// principal it went to.
    fn misaddressed(&self) -> (BTreeSet<RequestId>, Option<PrincipalId>) {
        let s = self.s;
        let (
            Some(Reply::ToUser { principal, .. }),
            Cmd::Talk(TalkCmd::ToUser {
                kind: ToUserKind::Result,
                ..
            }),
        ) = (self.ok, self.cmd)
        else {
            return (BTreeSet::new(), None);
        };
        let mis: BTreeSet<RequestId> = self
            .named()
            .into_iter()
            .filter(|r| {
                s.requests.get(r).is_some_and(|q| {
                    q.settled.is_none()
                        && q.answered_by.is_none()
                        && !self.others(q.node)
                        && match q.requester {
                            Requester::External => true,
                            Requester::User(p) => !same_person(s, p, *principal),
                        }
                })
            })
            .collect();
        let to = (!mis.is_empty()).then_some(*principal);
        (mis, to)
    }

    /// Rule 6: the requests in reach owed a reply, and the misaddressed ones: one `to_user` per
    /// requester person, one `said` per conversation. 1.7: also, `later`, the unanswered requests
    /// this call recorded on a node the caller holds, and, now, the unanswered requests no decision
    /// lists on a held node that carries `ready_to_close`: the reply, then the close of rule 8,
    /// which settles them.
    fn rule6<M: Meter>(&self, m: &mut M) -> Vec<NextCall> {
        let s = self.s;
        let held = self.held_open();
        let mut owed: BTreeSet<RequestId> = BTreeSet::new();
        // The oldest `request_scan()` of each such node: the scan stops there (D.8.26 "Bounds",
        // §18), so a hub with 10³ requests owed a reply costs no more than one with 20.
        for n in self.whole_reach() {
            if let Some(it) = self.items(n) {
                for r in it.owed_reply.iter().take(request_scan()) {
                    m.tick(1);
                    owed.insert(*r);
                }
            }
        }
        for r in self.partial_reach() {
            m.tick(1);
            if let Some(q) = s.requests.get(&r)
                && self
                    .items(q.node)
                    .is_some_and(|it| it.owed_reply.contains(&r))
            {
                owed.insert(r);
            }
        }
        let (mis, to) = self.misaddressed();
        owed.extend(mis.iter().copied());
        let mut v = reply_items(s, self.x, &owed, &held, &mis, to, ReplyWhy::Owed);
        let unanswered = |r: &RequestId| {
            s.requests
                .get(r)
                .is_some_and(|q| q.settled.is_none() && q.answered_by.is_none())
        };
        let mut ready: BTreeSet<RequestId> = BTreeSet::new();
        for &n in &held {
            if !self.flags(n).contains(Flag::ReadyToClose) {
                continue;
            }
            if let Some(it) = self.items(n) {
                for r in it.unsettled.iter().take(request_scan()) {
                    m.tick(1);
                    if unanswered(r) && !it.owed_reply.contains(r) {
                        ready.insert(*r);
                    }
                }
            }
        }
        v.extend(reply_items(
            s,
            self.x,
            &ready,
            &held,
            &BTreeSet::new(),
            None,
            ReplyWhy::Ready,
        ));
        let recorded: BTreeSet<RequestId> = self
            .recorded()
            .into_iter()
            .filter(|r| {
                unanswered(r)
                    && s.requests.get(r).is_some_and(|q| {
                        held.contains(&q.node)
                            && !ready.contains(r)
                            && self
                                .items(q.node)
                                .is_none_or(|it| !it.owed_reply.contains(r))
                    })
            })
            .collect();
        v.extend(reply_items(
            s,
            self.x,
            &recorded,
            &held,
            &BTreeSet::new(),
            None,
            ReplyWhy::Recorded,
        ));
        v
    }

    /// The node this call released, if it released one.
    fn released_node(&self) -> Option<NodeId> {
        match (self.ok, self.cmd) {
            (Some(Reply::Released { .. }), Cmd::Hold(HoldCmd::Release { token, .. })) => {
                Some(token.node)
            }
            _ => None,
        }
    }

    /// 1.10 (§20.9 Terms): request `r` is *left unfinished*: its node is open and not a container,
    /// and the node's last checkpoint, a release among them, came after the request's last answer:
    /// the entry that answered it, a later `said` of kind `reply`, `result`, or `correction` in its
    /// conversation on its node that is not withheld, or a `to_user` of kind `result` that this
    /// call recorded and that names it. Eids order entries (§5). The `said` entries are read from
    /// the node's after its last checkpoint, one visit each.
    fn left_unfinished<M: Meter>(&self, r: RequestId, m: &mut M) -> bool {
        let s = self.s;
        let Some(q) = s.requests.get(&r) else {
            return false;
        };
        let (Some(answer), Some(node)) = (q.answered_by, s.nodes.get(&q.node)) else {
            return false;
        };
        let Some(cp) = node.last_checkpoint.filter(|cp| *cp > answer) else {
            return false;
        };
        if !node.is_open() || node.is_container() {
            return false;
        }
        let fresh_result = matches!(
            self.cmd,
            Cmd::Talk(TalkCmd::ToUser {
                kind: ToUserKind::Result,
                ..
            })
        ) && self.named().contains(&r);
        if fresh_result {
            return false;
        }
        let later_said = self.items(q.node).is_some_and(|it| {
            it.saids.range(Eid(cp.0.saturating_add(1))..).any(|y| {
                m.tick(1);
                s.saids.get(y).is_some_and(|x| {
                    x.conv == q.conv
                        && matches!(
                            x.kind,
                            SaidKind::Reply | SaidKind::Result | SaidKind::Correction
                        )
                        && x.state != crate::state::SaidState::Withheld
                })
            })
        });
        !later_said
    }

    /// Rule 7: the requests in reach owed a decision, per node (1.7): on a node the session holds
    /// whose close rule 8 suggests, none, since that close records the decision (§5); on another
    /// node it holds, only those answered, now, and no `later` one for a request the call recorded
    /// (rule 6 suggests its reply); on a container it holds, which no close finishes, as before.
    /// The node a release left with answered requests undecided is in reach, now. 1.10: never a
    /// request left unfinished, whose answer was a partial report (`left_unfinished`); so after a
    /// release, only a container's.
    fn rule7<M: Meter>(&self, closing: &BTreeSet<NodeId>, m: &mut M) -> Vec<NextCall> {
        let s = self.s;
        let held = self.held_open();
        let closed = self.closed_node();
        let released = self.released_node();
        let partial = self.partial_reach();
        let recorded = self.recorded();
        let mut nodes: BTreeSet<NodeId> = self.whole_reach();
        nodes.extend(
            partial
                .iter()
                .filter_map(|r| s.requests.get(r).map(|q| q.node)),
        );
        // 1.8: a node this call released that another session holds is that holder's.
        nodes.extend(released.filter(|n| !self.others(*n)));
        let mut v = Vec::new();
        for n in nodes {
            m.tick(1);
            let is_held = held.contains(&n);
            let container = is_container_node(s, n);
            if is_held && !container && closing.contains(&n) {
                continue;
            }
            let it = self.items(n);
            let ready = is_held && container && self.flags(n).contains(Flag::ReadyToClose);
            let whole = ready || closed == Some(n);
            let after_release = released == Some(n) && !is_held;
            // The first NEXT_IDS_MAX of the requests owed a decision: the item lists no more, so
            // the scan stops there (§18); on a whole node, among its oldest `request_scan()`
            // unsettled requests, so those owed a reply (rule 6's) cannot make it run long. 1.10:
            // none left unfinished, which a whole node, closed or a container, never has.
            let main: BTreeSet<RequestId> = match it {
                None => BTreeSet::new(),
                Some(it) if whole => it
                    .unsettled
                    .iter()
                    .take(request_scan())
                    .inspect(|_| m.tick(1))
                    .copied()
                    .filter(|r| !it.owed_reply.contains(r))
                    .take(NEXT_IDS_MAX)
                    .collect(),
                Some(it) if is_held || after_release => it
                    .owed_decision
                    .iter()
                    .copied()
                    .take(request_scan())
                    .filter(|r| {
                        m.tick(1);
                        !self.left_unfinished(*r, m)
                    })
                    .take(NEXT_IDS_MAX)
                    .collect(),
                Some(it) => partial
                    .iter()
                    .copied()
                    .filter(|r| it.owed_decision.contains(r) && !self.left_unfinished(*r, m))
                    .take(NEXT_IDS_MAX)
                    .collect(),
            };
            let live = s.ix.node.get(&n).map_or(0, |i| i.live);
            let rec: BTreeSet<RequestId> = if live == 0 || (is_held && container) {
                recorded
                    .iter()
                    .copied()
                    .filter(|r| s.requests.get(r).is_some_and(|q| q.node == n))
                    .collect()
            } else {
                BTreeSet::new()
            };
            let d: BTreeSet<RequestId> = main.union(&rec).copied().collect();
            if d.is_empty() {
                continue;
            }
            let later = is_held && main.is_empty();
            v.push(item(
                Rule::Decide,
                if later { Moment::Later } else { Moment::Now },
                !is_held,
                Skeleton::Decide {
                    at: n,
                    settles: cut(d.iter().copied()),
                    supersedes: None,
                },
                Because::DecisionOwed {
                    requests: cut(d.iter().copied()),
                    after_close: closed == Some(n),
                    ready,
                    recorded: !rec.is_empty(),
                    held: is_held,
                    after_release,
                },
            ));
        }
        v
    }

    /// Whether this call cleared on `n` an item a closed `n` would be flagged for (D.8.26).
    fn cleared(&self, n: NodeId) -> bool {
        let s = self.s;
        let on_n = |r: &RequestId| s.requests.get(r).is_some_and(|q| q.node == n);
        match (self.ok, self.cmd) {
            (
                Some(Reply::Said {
                    settled, fulfilled, ..
                }),
                _,
            ) => {
                settled.iter().any(on_n)
                    || fulfilled
                        .iter()
                        .any(|c| s.commitments.get(c).is_some_and(|x| x.node == n))
            }
            (Some(Reply::ToUser { settled, .. }), _) => settled.iter().any(on_n),
            (Some(Reply::Decided { decision, .. }), _) => {
                s.decisions.get(decision).is_some_and(|d| d.node == n)
            }
            (
                Some(Reply::ObjectionResolved { entry: Some(_), .. }),
                Cmd::Talk(TalkCmd::Resolve { objection, .. }),
            ) => s.objections.get(objection).is_some_and(|o| o.node == n),
            (
                Some(Reply::HoldCheck {
                    node,
                    intent_done: Some(_),
                    ..
                }),
                _,
            ) => *node == n,
            (
                Some(Reply::SaidConfirmed { already: false }),
                Cmd::Talk(TalkCmd::SaidConfirm { said, .. }),
            ) => s.saids.get(said).is_some_and(|y| y.node == n),
            (
                Some(Reply::Checkpointed {
                    intent_done: Some(_),
                    ..
                }),
                Cmd::Hold(HoldCmd::Checkpoint { token, .. }),
            ) => token.node == n,
            (
                Some(Reply::Retracted { .. }),
                Cmd::Talk(TalkCmd::Retract {
                    target: RetractArg::Entry { node, .. },
                    ..
                }),
            ) => *node == n,
            (
                Some(Reply::Retracted { .. }),
                Cmd::Talk(TalkCmd::Retract {
                    target: RetractArg::Node(r),
                    ..
                }),
            ) => super::resolve(s, r) == Some(n),
            _ => false,
        }
    }

    /// `clean(n)` (D.8.26): no open child, commitment, or objection, no live hold or open intent of
    /// another session, and every unsettled request answered.
    fn clean(&self, n: NodeId) -> bool {
        let s = self.s;
        let open_children = s.ix.node.get(&n).map_or(0, |i| i.open_children);
        let it = self.items(n);
        let no_commitment = it.is_none_or(|it| it.open_commitments.is_empty());
        let no_objection = it.is_none_or(|it| it.open_objections.is_empty());
        let other_hold = s
            .holds
            .range(
                Token {
                    node: n,
                    fence: Fence(0),
                }..=Token {
                    node: n,
                    fence: Fence(u64::MAX),
                },
            )
            .any(|(_, h)| h.session != self.x && live(h, self.now));
        let other_intent = s
            .nodes
            .get(&n)
            .and_then(|x| x.open_intent)
            .is_some_and(|i| i.session != self.x);
        let answered = it.is_none_or(|it| {
            it.unsettled
                .iter()
                .all(|r| s.requests.get(r).is_some_and(|q| q.answered_by.is_some()))
        });
        open_children == 0
            && no_commitment
            && no_objection
            && !other_hold
            && !other_intent
            && answered
    }

    /// `leaves(n) = ∅` under `clean(n)`: no unconfirmed `said` and no stale basis on `n`.
    fn leaves_nothing(&self, n: NodeId) -> bool {
        self.items(n)
            .is_none_or(|it| it.unconfirmed.is_empty() && it.stale_basis.is_empty())
    }

    /// Rule 8, decided before rules 6 and 7 and emitted after them: the close of a held node that
    /// is clean, or the release of a held container. 1.11: its branch "all answered" counts a
    /// request left unfinished (`left_unfinished`) as not answered, as rule 7 does. 1.12: the
    /// `why` by the node's state (`Node::merge_wait`): its last `intent_done`, or its open intent,
    /// names a pull request, `MergeWait` with the answered requests a blocked release would
    /// settle, else `Finished`; and on such a node a decision this call recorded, or a last
    /// request it settled, makes the close `later`, never `now`.
    fn rule8<M: Meter>(&self, m: &mut M) -> Vec<NextCall> {
        let s = self.s;
        let issued = self.issued();
        let settled_out = self.settled_by_out();
        let mut v = Vec::new();
        for (t, h) in &self.holds {
            let n = t.node;
            let Some(node) = s.nodes.get(&n).filter(|x| x.is_open()) else {
                continue;
            };
            // A clean close settles every unsettled request in one `settles` of at most
            // NEXT_IDS_MAX ids, so more rule it out before anything is read (§18).
            let unsettled = self.items(n).map_or(0, |it| it.unsettled.len());
            let u: Vec<RequestId> = if unsettled > NEXT_IDS_MAX {
                Vec::new()
            } else {
                self.items(n)
                    .map(|it| it.unsettled.iter().copied().collect())
                    .unwrap_or_default()
            };
            let settled_here = settled_out
                .iter()
                .any(|r| s.requests.get(r).is_some_and(|q| q.node == n));
            if node.is_container() {
                let series = matches!(node.role, Role::Series(_));
                if settled_here && unsettled == 0 {
                    v.push(item(
                        Rule::ReleaseContainer,
                        Moment::Now,
                        true,
                        Skeleton::Release {
                            token: *t,
                            wait: None,
                        },
                        Because::ContainerStays { series },
                    ));
                } else if issued == Some(*t) {
                    v.push(item(
                        Rule::ReleaseContainer,
                        Moment::Later,
                        true,
                        Skeleton::Release {
                            token: *t,
                            wait: None,
                        },
                        Because::ContainerStays { series },
                    ));
                }
                continue;
            }
            if unsettled > NEXT_IDS_MAX || !self.clean(n) {
                continue;
            }
            // 1.11 (§20.9 rule 8): an "I'll look" reply followed by a checkpoint is no answer.
            let all_answered = !u.is_empty() && !u.iter().any(|r| self.left_unfinished(*r, m));
            let ready = self.flags(n).contains(Flag::ReadyToClose) && u.is_empty();
            let settled_last = settled_here && u.is_empty();
            let decided_here = u.is_empty()
                && matches!(self.cmd, Cmd::Talk(TalkCmd::Decide { at, .. })
                    if super::resolve(s, at) == Some(n));
            let intent_closed = u.is_empty()
                && matches!(
                    self.ok,
                    Some(Reply::Checkpointed {
                        intent_done: Some(_),
                        intent: None,
                        ..
                    })
                )
                && matches!(self.cmd, Cmd::Hold(HoldCmd::Checkpoint { token, .. }) if token == t);
            let reclosed = h.reopened.is_some() && self.cleared(n) && self.leaves_nothing(n);
            let standing = all_answered || ready;
            // 1.12: work waiting for its merge is not done because a decision was recorded; it
            // waits so when its last `intent_done`, or its open intent, names a pull request.
            let pr = node.merge_wait();
            let once_now = (!pr && (settled_last || decided_here)) || reclosed;
            let once_later = intent_closed || (pr && (settled_last || decided_here));
            if !(standing || once_now || once_later) {
                continue;
            }
            let later = !standing && !once_now;
            // 1.12: the answered requests a blocked release would settle, named in the `why`.
            let merge_settles = u.clone();
            // 1.7 (§5): without `settles`, the close settles what a result answered; the item
            // names them only when one was answered by a `said` of kind `reply` alone.
            let u = if u
                .iter()
                .all(|r| crate::helpers::conversation::result_answered(s, *r, &BTreeSet::new()))
            {
                Vec::new()
            } else {
                u
            };
            let outcome = match &h.reopened {
                Some(Outcome::Missed) | None => None,
                Some(o) => Some(o.clone()),
            };
            let because = match &h.reopened {
                Some(was) => Because::Reclose { was: was.clone() },
                None if pr => Because::MergeWait {
                    settles: merge_settles,
                },
                None => Because::Finished,
            };
            v.push(item(
                Rule::Close,
                if later { Moment::Later } else { Moment::Now },
                !standing,
                Skeleton::Close {
                    node: n,
                    token: *t,
                    settles: u,
                    outcome,
                },
                because,
            ));
        }
        v.extend(self.await_merge());
        v
    }

    /// 1.10 (rule 8): after a `handoff` release whose `intent_done` names a pull request
    /// (`words::names_pull_request`), the take that lets the agent release the node again, blocked,
    /// with an outside wait for the merge, when a take would succeed. The release warns nothing;
    /// only its text says what it waits for. In the lab's fifth round, a worker released as
    /// `handoff` after "opened PR #215", and another took the finished work three minutes later.
    fn await_merge(&self) -> Option<NextCall> {
        let (
            Some(Reply::Released { .. }),
            Cmd::Hold(HoldCmd::Release {
                token,
                reason: ReleaseReason::Handoff,
                waits_for: None,
                intent_done: Some(done),
                ..
            }),
        ) = (self.ok, self.cmd)
        else {
            return None;
        };
        let n = token.node;
        (!is_container_node(self.s, n)
            && crate::words::names_pull_request(&done.0)
            && crate::helpers::holds::can_take(self.s, self.x, n, self.now))
        .then(|| {
            item(
                Rule::AwaitMerge,
                Moment::Now,
                true,
                Skeleton::Take { target: n },
                Because::AwaitMerge,
            )
        })
    }

    // --------------------------------------------------------------------------------------------
    // 9

    /// Rule 9: what the call itself just made due, the first row that matches; for a status
    /// item, one reply per conversation.
    fn rule9(&self) -> Vec<NextCall> {
        if matches!(self.ok, Some(Reply::Next(NextReply::Status { .. }))) {
            return self.status_replies();
        }
        self.rule9_row().into_iter().collect()
    }

    /// Rule 9's rows other than the status item.
    fn rule9_row(&self) -> Option<NextCall> {
        let s = self.s;
        let reply = self.ok?;
        match (self.cmd, reply) {
            (
                Cmd::Talk(TalkCmd::Ask { at, to, .. }),
                Reply::Asked {
                    question, existing, ..
                },
            ) => match to {
                AskTo::Person { .. } | AskTo::User(_) => {
                    let n = super::resolve(s, at)?;
                    let t = self.hold_on(n)?;
                    let blocking = matches!(to, AskTo::Person { .. })
                        && s.pquestions.get(question).is_some_and(|q| q.blocking);
                    Some(item(
                        Rule::AskWait,
                        if blocking { Moment::Now } else { Moment::Later },
                        true,
                        Skeleton::Release {
                            token: t,
                            wait: Some(*question),
                        },
                        Because::AskWait {
                            question: *question,
                            blocking,
                            existing: *existing,
                        },
                    ))
                }
                AskTo::Agent(_) => Some(item(
                    Rule::AgentWait,
                    Moment::Later,
                    true,
                    Skeleton::Wait {
                        timeout: s.params.talk_wait_max,
                    },
                    Because::AgentWait {
                        question: *question,
                    },
                )),
            },
            (
                _,
                Reply::Joined {
                    existing,
                    open_question: Some(q),
                    ..
                },
            ) => {
                let pq = s.pquestions.get(q)?;
                Some(item(
                    Rule::JoinAnswer,
                    Moment::Now,
                    true,
                    Skeleton::Heard {
                        at: *existing,
                        answers: *q,
                        where_: pq.conv.clone(),
                    },
                    Because::JoinAnswer { question: *q },
                ))
            }
            (
                Cmd::Node(NodeCmd::Create(a)),
                Reply::Created {
                    id, token: None, ..
                },
            ) if a.take == Some(false) => {
                let node = s.nodes.get(id)?;
                let offered = node.tags.iter().any(|t| t.as_str() == OFFER);
                let nothing_offers = node.capacity >= 1
                    && !offered
                    && self.flags(*id).is_empty()
                    && !node.is_container()
                    && matches!(node.role, Role::Plain);
                nothing_offers.then(|| {
                    item(
                        Rule::Offer,
                        Moment::Now,
                        true,
                        Skeleton::AddOffer {
                            node: *id,
                            version: node.version,
                        },
                        Because::NotOffered,
                    )
                })
            }
            (
                Cmd::Talk(
                    TalkCmd::ToUser {
                        kind: ToUserKind::Question,
                        ..
                    }
                    | TalkCmd::FromUser {
                        answers: Some(_), ..
                    },
                ),
                _,
            ) => {
                let (q, to) = first_pending_uq(s, self.x)?;
                let principal = serves_several(s, self.x).then_some(to);
                Some(item(
                    Rule::AnswersNext,
                    Moment::Later,
                    true,
                    Skeleton::AnswerUser {
                        question: q,
                        principal,
                    },
                    Because::AnswersNext {
                        question: q,
                        principal: to,
                    },
                ))
            }
            // 1.9 (§10.4, §11.3): a review item asks whether its objection stands.
            (_, Reply::Next(NextReply::Review { objection, .. })) => {
                s.objections.get(objection)?;
                Some(item(
                    Rule::Review,
                    Moment::Now,
                    true,
                    Skeleton::Resolve {
                        objection: *objection,
                    },
                    Because::Review {
                        objection: *objection,
                    },
                ))
            }
            (_, Reply::Next(NextReply::Route { entry, .. })) => Some(item(
                Rule::RouteItem,
                Moment::Now,
                true,
                Skeleton::Route {
                    entry: *entry,
                    to: RouteSk::Fill,
                },
                Because::RouteItem,
            )),
            // 1.12 (§11.3): a resolve that left a closed claim reading done with nothing that
            // decides it (`no_basis`): its reopen, which makes it read unproved, not refuted. In
            // the lab's seventh round, every reviewer whose objection failed on a claim nothing
            // proved reopened it by hand, and one upheld a new objection to get there.
            (Cmd::Talk(TalkCmd::Resolve { .. }), Reply::ObjectionResolved { .. }) => {
                let n = self.no_basis()?;
                (s.nodes.get(&n).is_some_and(|x| !x.is_open()) && !s.occ_locked(n)).then(|| {
                    item(
                        Rule::Unproved,
                        Moment::Now,
                        true,
                        Skeleton::Reopen { node: n },
                        Because::Unproved,
                    )
                })
            }
            _ => None,
        }
    }

    /// 1.12 (§11.3, rule 9): the node of the call's `no_basis` warning, if it returned one.
    fn no_basis(&self) -> Option<NodeId> {
        self.out.as_ref().ok()?.warnings.iter().find_map(|w| {
            if let crate::reply::WarningDetail::NoBasis { node, .. } = w.detail {
                Some(node)
            } else {
                None
            }
        })
    }

    /// Rule 9's status replies: one `said` of kind `reply` per conversation whose commitment
    /// raises `open_commitment` (`status_convs`, D.8.26); 1.9: none while another session holds
    /// the node or work inside it, and this one holds neither: the holder speaks (§9.2).
    fn status_replies(&self) -> Vec<NextCall> {
        let s = self.s;
        let Some(Reply::Next(NextReply::Status { node: n, .. })) = self.ok else {
            return Vec::new();
        };
        let n = *n;
        let Some(it) = self.items(n) else {
            return Vec::new();
        };
        let mut convs: Vec<KeyStr> = Vec::new();
        for c in it
            .open_commitments
            .iter()
            .filter_map(|c| s.commitments.get(c))
        {
            let pq_open = it.open_pq.get(&c.conv).is_some_and(|q| !q.is_empty());
            if self.now >= c.followup_at && !pq_open && !convs.contains(&c.conv) {
                convs.push(c.conv.clone());
            }
        }
        let default = conv_key(s, n, None).ok().map(|k| k.key);
        convs
            .into_iter()
            .filter(|k| lands_on(s, n, k))
            // 1.9 (§9.2): no reply while another session holds the node or work inside it, and
            // this one holds neither: the holder speaks (1.10: a `said` would count all the same;
            // 1.11: unless no holder has the conversation's cap and this session has it).
            .filter(|k| crate::helpers::agents::conversation_holder(s, n, self.x, k).is_none())
            .map(|k| {
                let where_ = (default.as_ref() != Some(&k)).then_some(k);
                item(
                    Rule::StatusReply,
                    Moment::Now,
                    true,
                    Skeleton::Said {
                        at: n,
                        kind: SaidKind::Reply,
                        where_,
                        where_fill: false,
                    },
                    Because::StatusItem,
                )
            })
            .collect()
    }

    // --------------------------------------------------------------------------------------------
    // 10 to 13

    /// Rule 10: for each held node, the first item its flags' entities ask for.
    fn rule10(&self) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        for (t, _) in &self.holds {
            let n = t.node;
            let Some(node) = s.nodes.get(&n) else {
                continue;
            };
            let it = self.items(n);
            let mk = |call: Skeleton, because: Because, when: Moment| {
                item(Rule::HeldFlag, when, false, call, because)
            };
            // 1.9 (§9.2): an unconfirmed `said` of another session gets no item; no next call asks
            // to confirm a `said`.
            if !node.assume_marks.is_empty() {
                let blocking = node.assume_marks.iter().any(|q| {
                    s.uquestions
                        .get(q)
                        .is_some_and(|u| matches!(u.source, UqSource::Escalation { .. }))
                });
                v.push(mk(
                    Skeleton::Decide {
                        at: n,
                        settles: Vec::new(),
                        supersedes: None,
                    },
                    Because::Assumption {
                        questions: cut(node.assume_marks.iter().copied()),
                        blocking,
                    },
                    Moment::Now,
                ));
            } else if let Some(d) = it.and_then(|it| it.stale_basis.first().copied()) {
                v.push(mk(
                    Skeleton::Decide {
                        at: n,
                        settles: Vec::new(),
                        supersedes: Some(d),
                    },
                    Because::StaleBasis { decision: d },
                    Moment::Now,
                ));
            } else if let Some(q) = node.recheck_marks.first().copied() {
                v.push(mk(
                    Skeleton::Release {
                        token: *t,
                        wait: Some(q),
                    },
                    Because::Recheck { question: q },
                    Moment::Now,
                ));
            } else if node.attempts >= s.params.too_big_attempts && can_split(s, n) {
                v.push(mk(
                    Skeleton::Split { parent: n },
                    Because::TooBig {
                        attempts: node.attempts,
                    },
                    Moment::Now,
                ));
            } else if let Role::Signal(key) = &node.role {
                // 1.8: never a merge that closes a node another session holds.
                let merge = (key.kind == SignalKind::Duplicate)
                    .then(|| merge_pair(s, key.a, key.b))
                    .and_then(|(from, into)| {
                        let ok = !is_container_node(s, from)
                            && !is_container_node(s, into)
                            && !self.others(from)
                            && merge_validates(s, &self.ctx(), from, into);
                        ok.then_some((from, into))
                    });
                match merge {
                    Some((from, into)) => v.push(mk(
                        Skeleton::Merge { from, into },
                        Because::Duplicate { other: into },
                        Moment::Now,
                    )),
                    None => v.push(mk(
                        Skeleton::Decide {
                            at: n,
                            settles: Vec::new(),
                            supersedes: None,
                        },
                        Because::Signal,
                        Moment::Now,
                    )),
                }
            } else if let Some(o) = it.and_then(|it| it.open_objections.first().copied()) {
                v.push(mk(
                    Skeleton::Resolve { objection: o },
                    Because::Objection {
                        objection: o,
                        retracted: it.and_then(|it| retracted_decision(s, it)),
                    },
                    Moment::Later,
                ));
            }
        }
        v.extend(self.linked_merge());
        v
    }

    /// 1.11 (rule 10): after a `node link {rel: duplicate}`, the merge of its pair in §16's
    /// direction (`merge_pair`), now and once, when it would pass, names no container, and its
    /// `from` no other session holds: "if they are one piece of work: merge into <into>". In the
    /// lab's sixth round, a worker that linked two tasks it was working on got no item, and spent 4
    /// calls on the pair and its signal where one merge would do.
    fn linked_merge(&self) -> Option<NextCall> {
        let s = self.s;
        let (
            Some(Reply::Linked { .. }),
            Cmd::Node(NodeCmd::Link {
                at,
                rel: crate::types::LinkRel::Duplicate,
                to,
            }),
        ) = (self.ok, self.cmd)
        else {
            return None;
        };
        let (x, y) = (super::resolve(s, at)?, super::resolve(s, to)?);
        if x == y {
            return None;
        }
        let (from, into) = merge_pair(s, x.min(y), x.max(y));
        (!is_container_node(s, from)
            && !is_container_node(s, into)
            && !self.others(from)
            && merge_validates(s, &self.ctx(), from, into))
        .then(|| {
            item(
                Rule::HeldFlag,
                Moment::Now,
                true,
                Skeleton::Merge { from, into },
                Because::Linked { into },
            )
        })
    }

    /// Rule 11: unread notes, and an open agent question another session asked that this call
    /// delivered.
    fn rule11(&self) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        let unread = crate::helpers::agents::inbox(s, self.x, self.now).len();
        if unread > 0 {
            v.push(item(
                Rule::Inbox,
                Moment::Now,
                false,
                Skeleton::Wait { timeout: SECOND },
                Because::Inbox {
                    count: u32::try_from(unread).unwrap_or(u32::MAX),
                },
            ));
        }
        let delivered: &[NoteId] = match self.ok {
            Some(Reply::Hello { inbox, .. }) => inbox,
            Some(Reply::Waited { delivered, .. }) => delivered,
            _ => &[],
        };
        let mut ds: Vec<NoteId> = delivered.to_vec();
        // 1.7 (§13): the notes the response to a write delivered. 1.8: their answer is `later`,
        // once the agent knows it, after the calls to make now.
        let on_write: &[NoteId] = self.out.as_ref().map_or(&[], |d| d.delivered.as_slice());
        ds.extend(on_write.iter().copied());
        ds.sort_unstable();
        ds.dedup();
        for q in ds {
            let Some(aq) = s.aquestions.get(&q) else {
                continue;
            };
            if aq.state != AqState::Open || aq.asker == self.x {
                continue;
            }
            let Some(note) = s.notes.get(&q) else {
                continue;
            };
            let later = on_write.contains(&q) && !delivered.contains(&q);
            v.push(item(
                Rule::AnswerAgent,
                if later { Moment::Later } else { Moment::Now },
                true,
                Skeleton::AnswerAgent { question: q },
                Because::AgentAsked {
                    by: aq.asker,
                    until: note.until,
                    question: q,
                    later,
                },
            ));
        }
        v
    }

    /// Rule 12: the first principal of the session, in key order, whose person has pending items
    /// and who has not been told. It reads the pending counts as `hello` does (D.8.25): per alias,
    /// the size of its sets and the first `NEXT_IDS_MAX` digest entries, so a response costs
    /// O(principals × aliases × log n) however many items are pending (§18, D.12.8).
    fn rule12<M: Meter>(&self, m: &mut M) -> Option<NextCall> {
        let s = self.s;
        let x = s.sessions.get(&self.x)?;
        let mut ps: Vec<(&PrincipalKey, PrincipalId)> = x
            .principals
            .iter()
            .filter_map(|p| s.ix.principal_key.get(p).map(|k| (k, *p)))
            .collect();
        ps.sort();
        for (_, p) in ps {
            m.tick(1);
            if x.told.iter().any(|t| same_person(s, *t, p)) {
                continue;
            }
            let aliases = crate::helpers::principals::aliases(s, p);
            let mut questions: usize = 0;
            let mut digest: usize = 0;
            // A digest entry is pending for its one addressee (`pending_digest_by[to]`), so the
            // sets of the aliases are disjoint: their sizes add up to the person's count, and the
            // oldest NEXT_IDS_MAX of their union lie among the oldest NEXT_IDS_MAX of each.
            let mut first: BTreeSet<DigestId> = BTreeSet::new();
            for a in &aliases {
                m.tick(1);
                questions =
                    questions.saturating_add(s.ix.pending_uq_by.get(a).map_or(0, BTreeSet::len));
                if let Some(d) = s.ix.pending_digest_by.get(a) {
                    digest = digest.saturating_add(d.len());
                    first.extend(d.iter().take(NEXT_IDS_MAX).copied());
                }
            }
            if questions == 0 && digest == 0 {
                continue;
            }
            return Some(item(
                Rule::HumanItems,
                Moment::Now,
                false,
                Skeleton::ToUser {
                    principal: p,
                    kind: (questions > 0).then_some(ToUserKind::Question),
                    re: cut(first.iter().copied()),
                },
                Because::HumanItems {
                    principal: p,
                    questions: u32::try_from(questions).unwrap_or(u32::MAX),
                    digest: u32::try_from(digest).unwrap_or(u32::MAX),
                },
            ));
        }
        None
    }

    /// Rule 13: for a worker, after a close, a release, an empty next, a join, `session hello`, a
    /// `talk resolve` or a `node reopen` (1.8), or a `full`, `paused`, or `closed` refusal (without
    /// `merged_into`), the first of: take the first node `leave_better` names, take the node it
    /// joined, ask `next`. 1.8: after a release, a resolve, or a reopen, `hold next` comes whenever
    /// the worker holds nothing and `next` would offer it an item, after a release's tidy item: in
    /// the lab's third round, a tidy item for a node the worker's caps did not cover was its only
    /// item, and it stopped while `next` would have given it work. 1.9: when the first node
    /// `leave_better` names is closed with an open objection, its review (`talk resolve`) comes in
    /// place of the take, never a take of it; then, where `end` would come, a worker that holds
    /// nothing and that `next` offers nothing gets the forum's tidy items, after that review.
    /// 1.12: the tidy take is of the first node `leave_better` names that the worker's caps and
    /// horizon fit (`fits`), past a first node that is a review or a stale offer; and after any
    /// other write, a worker that holds nothing gets the `hold next` while `next` would offer it an
    /// item, where 1.11 gave `next_calls: none`.
    fn rule13<M: Meter>(&self, m: &mut M) -> Vec<NextCall> {
        let s = self.s;
        let mut v = Vec::new();
        if !s
            .sessions
            .get(&self.x)
            .is_some_and(|x| x.principals.is_empty())
        {
            return v;
        }
        let hello = matches!(self.cmd, Cmd::Session(SessionCmd::Hello { .. }));
        let refused = self.out.as_ref().err().map(|e| &e.hint);
        // `after_work`: a release, a resolve, or a reopen (1.8).
        let (lb, joined, empty_next, after_work): (&[NodeId], Option<NodeId>, bool, bool) =
            match (self.ok, refused) {
                (Some(Reply::Closed { leave_better, .. }), _) => (leave_better, None, false, false),
                (Some(Reply::Released { leave_better, .. }), _) => {
                    (leave_better, None, false, true)
                }
                (Some(Reply::ObjectionResolved { .. } | Reply::Reopened { .. }), _) => {
                    (&[], None, false, true)
                }
                (Some(Reply::Next(NextReply::Empty { leave_better, .. })), _) => {
                    (leave_better, None, true, false)
                }
                (Some(Reply::Joined { existing, .. }), _) => (&[], Some(*existing), false, false),
                (Some(Reply::Hello { .. }), _) if hello => (&[], None, false, false),
                // 1.10 (rule E): a `hello` refused only for its unnamed `from_user`, as a `hello`.
                (None, Some(_)) if hello_refused_unnamed(self.cmd, self.out) => {
                    (&[], None, false, false)
                }
                (
                    None,
                    Some(
                        Hint::Full { .. }
                        | Hint::Paused { .. }
                        | Hint::Closed {
                            merged_into: None, ..
                        },
                    ),
                ) => (&[], None, false, false),
                // 1.12: any other write, which neither ends work nor finds it: only the `hold
                // next` below. In the lab's seventh round, a reviewer that held nothing ended on
                // `next_calls: none` after a note, while reviews were offered to it.
                (Some(_), None) if writes_only(self.cmd) => (&[], None, false, false),
                _ => return v,
            };
        let per = usize::try_from(s.params.hold_per_session).unwrap_or(usize::MAX);
        let can = |n: NodeId| crate::helpers::holds::can_take(s, self.x, n, self.now);
        // 1.10 (§16): a stale offer, whose only flag is `stale`, is tidied by §16's item for it.
        let stale_only = |n: NodeId| self.flags(n) == FlagSet::EMPTY.with(Flag::Stale);
        // 1.9 (§10.3, §10.4): a closed node with an open objection is tidied by its review.
        if let Some(&first) = lb.first()
            && let Some(o) = review_objection(s, first)
        {
            v.push(item(
                Rule::Tidy,
                Moment::Now,
                true,
                Skeleton::Resolve { objection: o },
                Because::Review { objection: o },
            ));
            if !after_work {
                return v;
            }
        } else if let Some(&first) = lb.first()
            && stale_only(first)
        {
            let step = super::leave_better::stale_step(s, first);
            let asked = matches!(step, Some(super::leave_better::StaleStep::Ask));
            if let Some(step) = step
                && (asked || (self.holds.len() < per && can(first)))
            {
                v.push(stale_item(first, step, false));
                if !after_work {
                    return v;
                }
            }
        } else if let Some(&take) = lb.iter().find(|&&n| {
            review_objection(s, n).is_none() && !stale_only(n) && self.fits(n) && can(n)
        }) && self.holds.len() < per
        {
            // 1.12: the first node the worker's caps and horizon fit. In the lab's seventh round, a
            // take of a node over the worker's horizon came as its only item, and the take warned.
            v.push(item(
                Rule::Tidy,
                Moment::Now,
                true,
                Skeleton::Take { target: take },
                Because::Tidy {
                    node: take,
                    flags: self.flags(take),
                },
            ));
            if !after_work {
                return v;
            }
        }
        if let Some(j) = joined
            && can(j)
        {
            v.push(item(
                Rule::TakeJoined,
                Moment::Now,
                true,
                Skeleton::Take { target: j },
                Because::Joined,
            ));
            return v;
        }
        if self.holds.is_empty() {
            let (first, _) = super::ranked(s, self.x, self.now, &NextQuery::default(), 1, m);
            // 1.9: where `end` would come, after an empty `next`, a `hello`, a release, a
            // resolve, or a reopen, an idle worker that `next` offers nothing first gets the
            // forum's tidy items.
            let tidy_point = empty_next
                || after_work
                || matches!(self.ok, Some(Reply::Hello { .. }))
                || hello_refused_unnamed(self.cmd, self.out);
            if let Some(f) = first.first() {
                if !empty_next {
                    // 1.10: after any call but a `hello`, the why names what `next` would give.
                    let because = if hello {
                        Because::MoreWork { at_hello: true }
                    } else {
                        Because::WorkWaiting { first: f.cand }
                    };
                    v.push(item(
                        Rule::MoreWork,
                        Moment::Now,
                        true,
                        Skeleton::Next,
                        because,
                    ));
                }
            } else if tidy_point {
                v.extend(self.tidy_items(m));
            }
        }
        v
    }

    /// 1.12 (rule 13): the session's caps meet `n`'s needs and its horizon covers `n`'s `est`
    /// (either missing: covered), as `next` requires of the nodes it offers.
    fn fits(&self, n: NodeId) -> bool {
        let (Some(x), Some(node)) = (self.s.sessions.get(&self.x), self.s.nodes.get(&n)) else {
            return false;
        };
        node.needs.is_subset(&x.caps)
            && !matches!((node.est, x.horizon), (Some(e), Some(h)) if e > h)
    }

    /// 1.9 (§10.3, §16, §20.9 rule 13): the forum's tidy items for an idle worker, from
    /// `leave_better::tidy`: a take of each stale offer, and a duplicate link for each pair of
    /// possible duplicates, the node §16's merge would merge away as its source. In the lab's fourth
    /// round (l4, as in the three before), the stalest offers were cousins of every node a worker
    /// touched, so `leave_better` never named them, and two offers of the same work under `/data`
    /// carried no sign of it: no idle worker ever tidied that forum.
    fn tidy_items<M: Meter>(&self, m: &mut M) -> Vec<NextCall> {
        let t = super::leave_better::tidy(self.s, self.x, self.now, m);
        t.items
            .into_iter()
            .map(|i| match i {
                super::leave_better::TidyItem::Stale(n, step) => stale_item(n, step, t.cut),
                super::leave_better::TidyItem::Alike { from, to, .. } => item(
                    Rule::Tidy,
                    Moment::Now,
                    true,
                    Skeleton::LinkDuplicate { from, to },
                    Because::Alike {
                        from,
                        to,
                        cut: t.cut,
                    },
                ),
            })
            .collect()
    }
}

/// 1.12 (rule 13): a write that is none of the calls rule 13 reads otherwise: not a read, and not
/// `hold next`, a release, a close, a resolve, a reopen, or a join, whose replies rule 13 matches
/// first, nor `session hello`.
fn writes_only(cmd: &Cmd) -> bool {
    !cmd.is_read() && !matches!(cmd, Cmd::Hold(HoldCmd::Next { .. }))
}

/// 1.10 (§16, rule 13): the tidy item of stale offer `n` for its step: the question for its owners,
/// or, once that has its answer, a take with that news; never 1.9's take to refresh it.
fn stale_item(n: NodeId, step: super::leave_better::StaleStep, cut: bool) -> NextCall {
    match step {
        super::leave_better::StaleStep::Ask => item(
            Rule::Tidy,
            Moment::Now,
            true,
            Skeleton::AskOwners { at: n },
            Because::StaleOffer { node: n, cut },
        ),
        super::leave_better::StaleStep::Answered(question) => item(
            Rule::Tidy,
            Moment::Now,
            true,
            Skeleton::Take { target: n },
            Because::StaleAnswered {
                node: n,
                question,
                cut,
            },
        ),
    }
}

/// 1.9 (§20.9 rule 10): a node's newest retracted decision, while it has no active one: the
/// retraction an objection's `why` names as done. With none active, the newest decision is
/// retracted, since a decision is superseded only by a newer one; decisions that left memory are
/// not read, and the scan stops at `SCAN_MAX` (§18).
fn retracted_decision(s: &State, it: &NodeItems) -> Option<DecisionId> {
    if !it.active_decisions.is_empty() {
        return None;
    }
    let max = usize::try_from(SCAN_MAX).unwrap_or(usize::MAX);
    it.decisions.iter().rev().take(max).copied().find(|d| {
        s.decisions
            .get(d)
            .is_some_and(|x| matches!(x.state, crate::state::DecisionState::Retracted { .. }))
    })
}

/// The node a write that carries a token targets (D.8.24 step 3): the token's node for
/// `checkpoint`, `renew`, and `release`; `at` for `update` and `close`.
fn write_target(s: &State, cmd: &Cmd) -> Option<NodeId> {
    if let Cmd::Hold(
        HoldCmd::Checkpoint { token, .. }
        | HoldCmd::Renew { token, .. }
        | HoldCmd::Release { token, .. },
    ) = cmd
    {
        return Some(token.node);
    }
    if let Cmd::Node(NodeCmd::Update { at, .. } | NodeCmd::Close { at, .. }) = cmd {
        return super::resolve(s, at);
    }
    None
}

/// The node whose version a refused call named ahead of its own (`invalid(VersionAhead)`): the
/// `on` of `approve`, or the first basis reference of `decide` at a version past its node's.
fn version_ahead(s: &State, cmd: &Cmd) -> Option<NodeId> {
    if let Cmd::Talk(TalkCmd::Approve { on, .. }) = cmd {
        return super::resolve(s, on);
    }
    let Cmd::Talk(TalkCmd::Decide { basis, .. }) = cmd else {
        return None;
    };
    basis.iter().find_map(|b| match b {
        BasisArg::Node {
            at,
            version: Some(v),
        } => super::resolve(s, at).filter(|n| s.nodes.get(n).is_some_and(|x| *v > x.version)),
        BasisArg::Node { version: None, .. } | BasisArg::Entry(_) | BasisArg::External { .. } => {
            None
        }
    })
}

/// The question a refused answer named: `heard {answers}`, `talk answer`, `from_user {answers}`, or
/// `route {answers}` (1.7).
fn answered_question(cmd: &Cmd) -> Option<QuestionId> {
    if let Cmd::Talk(
        TalkCmd::Heard {
            answers: Some(q), ..
        }
        | TalkCmd::FromUser {
            answers: Some(q), ..
        }
        | TalkCmd::Route {
            answers: Some(q), ..
        }
        | TalkCmd::Answer { question: q, .. },
    ) = cmd
    {
        Some(*q)
    } else {
        None
    }
}

/// The answering call that fits question `q` (§21): `talk heard` for a person's, with its
/// conversation key; `talk answer` for an agent's; `from_user {answers}` for the user's, with its
/// addressee when the session serves several persons.
fn answer_fits(s: &State, x: SessionId, q: QuestionId) -> Option<Skeleton> {
    if let Some(pq) = s.pquestions.get(&q) {
        Some(Skeleton::Heard {
            at: pq.node,
            answers: q,
            where_: pq.conv.clone(),
        })
    } else if s.aquestions.contains_key(&q) {
        Some(Skeleton::AnswerAgent { question: q })
    } else if let Some(uq) = s.uquestions.get(&q) {
        let to = uq
            .to
            .get(usize::from(uq.stage))
            .copied()
            .unwrap_or(crate::types::USER_PRINCIPAL);
        Some(Skeleton::AnswerUser {
            question: q,
            principal: serves_several(s, x).then_some(to),
        })
    } else {
        None
    }
}

/// Whether the session serves principals of several persons (§8.1, D.8.11).
fn serves_several(s: &State, x: SessionId) -> bool {
    let Some(ps) = s.sessions.get(&x).map(|x| &x.principals) else {
        return false;
    };
    let Some(&first) = ps.first() else {
        return false;
    };
    !ps.iter().all(|&p| same_person(s, p, first))
}

/// The first pending user question for `p`'s person, under any alias, in `UqRank` order (1.7, rule
/// 5).
fn first_pending_for(s: &State, p: PrincipalId) -> Option<QuestionId> {
    crate::helpers::principals::aliases(s, p)
        .into_iter()
        .filter_map(|a| s.ix.pending_uq_by.get(&a).and_then(|set| set.first()))
        .min()
        .map(|r| r.q)
}

/// The first pending user question for the session's persons, in `UqRank` order, with its
/// current addressee.
fn first_pending_uq(s: &State, x: SessionId) -> Option<(QuestionId, PrincipalId)> {
    let served = super::reads::served(s, x);
    let mut best: Option<(UqRank, PrincipalId)> = None;
    for a in &served {
        if let Some(r) = s.ix.pending_uq_by.get(a).and_then(|set| set.first())
            && best.as_ref().is_none_or(|(b, _)| r < b)
        {
            best = Some((*r, *a));
        }
    }
    best.map(|(r, a)| (r.q, a))
}

/// When a hold's checkpoint is due (§20.9): `max(last_extended, min(last_extended + every,
/// lease_until − margin))`; with §22's values, `query::checkpoint_by`.
fn due_by(h: &Hold, every: Dur, margin: Dur) -> Time {
    h.last_extended
        .max(h.last_extended.plus(every).min(h.lease_until.minus(margin)))
}

/// `news(m, since)` of rule 4 (D.8.26): the people entries on `m` past `seen`, by another session,
/// recorded after `since`; None when there are none, else their count, `≥k` from `MOVED_MAX` on,
/// or when the scan stops at `SCAN_MAX` entries. Read newest first, so the scan stops at the first
/// entry at or before `since`.
fn news<M: Meter>(
    s: &State,
    m: NodeId,
    seen: Seq,
    since: Time,
    x: SessionId,
    meter: &mut M,
) -> Option<Total> {
    let lo = (m, Seq(seen.0.saturating_add(1)));
    let hi = (m, Seq(u64::MAX));
    if lo > hi {
        return None;
    }
    let max = u32::try_from(MOVED_MAX).unwrap_or(u32::MAX);
    let mut count: u32 = 0;
    let mut visited: u32 = 0;
    // Newest first: a node's entries are appended in time order (T1), so the scan stops at the
    // first entry recorded at or before `since`, and reads only what came after the take.
    for (_, e) in s.ix.people.range(lo..=hi).rev() {
        if visited >= SCAN_MAX {
            return (count > 0).then_some(Total::AtLeast(count));
        }
        visited = visited.saturating_add(1);
        meter.tick(1);
        let Some(meta) = s.entries.get(e) else {
            continue;
        };
        if meta.at <= since {
            break;
        }
        if meta.by == Author::Session(x) {
            continue;
        }
        if count >= max {
            return Some(Total::AtLeast(max));
        }
        count = count.saturating_add(1);
    }
    (count > 0).then_some(Total::Exact(count))
}

/// The nearest node from `n` up that carries a conversation key, if any (§9.1).
fn conversation_node_of(s: &State, n: NodeId) -> Option<NodeId> {
    let schemes = &s.params.conversation_schemes;
    core::iter::once(n)
        .chain(crate::index::ancestors(s, n))
        .find(|m| {
            s.nodes
                .get(m)
                .is_some_and(|x| x.keys.keys().any(|k| k.is_conversation(schemes)))
        })
}

/// Which of rule 6's branches a group of replies comes from (1.7).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplyWhy {
    /// Owed a reply, or misaddressed: now.
    Owed,
    /// Unanswered on a held node that carries `ready_to_close`: now, standing.
    Ready,
    /// Recorded by this call on a node the caller holds: `later`, once.
    Recorded,
}

/// Rule 6's items over the owed requests `owed`: user requests grouped by requester person, then
/// external ones by conversation node and key, each group in the order of its oldest request. 1.9
/// (§9.2): no `said` of session `x` where another session holds the conversation node or work
/// inside it, and `x` holds neither, since the holder speaks.
fn reply_items(
    s: &State,
    x: SessionId,
    owed: &BTreeSet<RequestId>,
    held: &BTreeSet<NodeId>,
    mis: &BTreeSet<RequestId>,
    to: Option<PrincipalId>,
    why: ReplyWhy,
) -> Vec<NextCall> {
    // The groups in the order of their oldest request (`owed` is read in id order), each found
    // through a map: `same_person` is equality of the person, or of the principal for one with
    // no person, so a person's group is keyed by its person node.
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum Who {
        Person(NodeId),
        Principal(PrincipalId),
    }
    let mut users: Vec<(PrincipalId, Vec<RequestId>)> = Vec::new();
    let mut user_at: BTreeMap<Who, usize> = BTreeMap::new();
    let mut convs: Vec<((NodeId, KeyStr), Vec<RequestId>)> = Vec::new();
    let mut conv_at: BTreeMap<(NodeId, KeyStr), usize> = BTreeMap::new();
    for &r in owed {
        let Some(q) = s.requests.get(&r) else {
            continue;
        };
        match q.requester {
            Requester::User(p) => {
                let who =
                    s.ix.person_of
                        .get(&p)
                        .map_or(Who::Principal(p), |m| Who::Person(*m));
                match user_at.get(&who).and_then(|i| users.get_mut(*i)) {
                    Some((_, rs)) => rs.push(r),
                    None => {
                        user_at.insert(who, users.len());
                        users.push((p, alloc::vec![r]));
                    }
                }
            }
            Requester::External => {
                let key = (q.node, q.conv.clone());
                match conv_at.get(&key).and_then(|i| convs.get_mut(*i)) {
                    Some((_, rs)) => rs.push(r),
                    None => {
                        conv_at.insert(key.clone(), convs.len());
                        convs.push((key, alloc::vec![r]));
                    }
                }
            }
        }
    }
    let standing = |rs: &[RequestId]| {
        why != ReplyWhy::Recorded
            && rs
                .iter()
                .any(|r| s.requests.get(r).is_some_and(|q| held.contains(&q.node)))
    };
    let when = if why == ReplyWhy::Recorded {
        Moment::Later
    } else {
        Moment::Now
    };
    let because = |rs: &[RequestId]| Because::ReplyOwed {
        requests: cut(rs.iter().copied()),
        misaddressed: if rs.iter().any(|r| mis.contains(r)) {
            to
        } else {
            None
        },
        recorded: why == ReplyWhy::Recorded,
        ready: why == ReplyWhy::Ready,
    };
    let mut v = Vec::new();
    for (p, rs) in users {
        v.push(item(
            Rule::Reply,
            when,
            !standing(&rs),
            Skeleton::ToUser {
                principal: p,
                kind: Some(ToUserKind::Result),
                re: cut(rs.iter().copied()),
            },
            because(&rs),
        ));
    }
    for ((n, k), rs) in convs {
        // A `said` in conversation `k` at `n` lands on `n` only while `n` (or the nearest of its
        // ancestors) carries `k`, or no open node does: otherwise no `said` answers these requests
        // there, and suggesting one would record it in another conversation (see the package
        // report).
        if !lands_on(s, n, &k) {
            continue;
        }
        // 1.9 (§9.2): while another session holds `n` or work inside it, and `x` holds neither,
        // the holder speaks (1.10: a `said` there would count all the same; 1.11: unless no
        // holder has the conversation's cap and `x` has it, §9.1).
        if crate::helpers::agents::conversation_holder(s, n, x, &k).is_some() {
            continue;
        }
        let default = conv_key(s, n, None).ok().map(|d| d.key);
        let where_ = (default.as_ref() != Some(&k)).then_some(k);
        v.push(item(
            Rule::Reply,
            when,
            !standing(&rs),
            Skeleton::Said {
                at: n,
                kind: SaidKind::Result,
                where_,
                where_fill: false,
            },
            because(&rs),
        ));
    }
    v
}

/// Whether a `said` at `n` in conversation `k` is recorded on `n` itself (§9.1, `conv_node`).
fn lands_on(s: &State, n: NodeId, k: &KeyStr) -> bool {
    crate::helpers::conversation::conv_node(s, n, k).is_ok_and(|c| c.node == n)
}

/// Whether a child can be created under `n` (rule 10's split): its depth and its open agent-made
/// children leave room (L1).
fn can_split(s: &State, n: NodeId) -> bool {
    let i = s.ix.node.get(&n).copied().unwrap_or_default();
    i.depth < s.params.tree_max_depth && i.open_agent_children < s.params.tree_max_children
}

/// 1.11 (§20.9 rule 5): whether `n`, the first row of a call's `related`, is an open work node,
/// which the `why` of rule 5's create names for the entry the call logged (1.12; 1.11 routed the
/// entry there): an open node, not reserved, not a signal node, and not under `@people` or
/// `@teams`; 1.12: not a container (a series, a person or team) and with no open child, so that
/// an area such as a team's, whose title and body share the most words with any question about
/// the team, is never named. In the lab's seventh round, a newcomer's question went to a data
/// team's area as a request on 1.11's route.
pub(crate) fn related_target(s: &State, n: NodeId) -> bool {
    let Some(node) = s.nodes.get(&n) else {
        return false;
    };
    node.is_open()
        && !n.is_reserved()
        && !node.is_container()
        && s.ix.node.get(&n).is_none_or(|i| i.open_children == 0)
        && !matches!(node.role, Role::Signal(_))
        && !crate::index::ancestors(s, n)
            .iter()
            .any(|a| *a == crate::types::PEOPLE || *a == crate::types::TEAMS)
}

/// Whether `n` is a container (§4): reserved, a series, or a person or team.
fn is_container_node(s: &State, n: NodeId) -> bool {
    s.nodes.get(&n).is_none_or(Node::is_container)
}

/// The merge rule 10 suggests for a duplicate pair `a < b`, by §16's convention: the newer into the
/// older, unless the newer is held, then the older into the newer, so a held node survives (D.8.26).
pub(crate) fn merge_pair(s: &State, a: NodeId, b: NodeId) -> (NodeId, NodeId) {
    let newer_held = s.ix.node.get(&b).is_some_and(|i| i.live > 0);
    if newer_held { (a, b) } else { (b, a) }
}

/// Whether `validate` accepts `node merge {from, into}` from the session.
fn merge_validates(s: &State, ctx: &Ctx, from: NodeId, into: NodeId) -> bool {
    let cmd = Cmd::Node(NodeCmd::Merge {
        from: NodeRef::Id(from),
        into: NodeRef::Id(into),
    });
    let SessionCtx::Existing(x) = ctx.session else {
        return false;
    };
    let cx = crate::dispatch::Vcx {
        s,
        now: ctx.now,
        ctx,
        sess: Some(x),
    };
    crate::dispatch::validate(&cx, &cmd).is_ok()
}

// ------------------------------------------------------------------------------------------------
// The standard fill of G1 (b) (D.9.2)

/// A name not taken under `parent`, for a path the agent writes (`CreateFor`, `Split`).
fn fresh_name(s: &State, parent: NodeId) -> Option<Name> {
    let tries = u64::try_from(s.names.len().saturating_add(s.aliases.len()))
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    (0..=tries).find_map(|i| {
        let mut text = String::from("next-call-");
        text.push_str(&Eid(i).render());
        Name::new(&text).filter(|name| {
            !s.names.contains_key(&(parent, name.clone()))
                && !s.aliases.contains_key(&(parent, name.clone()))
        })
    })
}

/// The path of a node, from the root (none for a node that does not exist).
fn path_to(s: &State, n: NodeId) -> Option<Path> {
    let mut steps = Vec::new();
    let mut cur = n;
    let mut guard = 0_usize;
    while let Some(p) = s.nodes.get(&cur)?.place.as_ref() {
        steps.push(p.name.clone());
        cur = p.parent;
        guard = guard.saturating_add(1);
        if guard > s.nodes.len() {
            return None;
        }
    }
    steps.reverse();
    Some(Path(steps))
}

/// A key as a command carries it: the node's normaliser version when the node carries it.
fn key_on(s: &State, n: NodeId, k: &KeyStr) -> Key {
    let nv = s
        .nodes
        .get(&n)
        .and_then(|x| x.keys.get(k).copied())
        .unwrap_or(NormVer(1));
    Key { key: k.clone(), nv }
}

/// The standard fill of G1 (b) (D.9.2): the call an item describes, made by session `sess` (a
/// `Hello` as a new session), with texts `"x"`, outcome `done` where the skeleton leaves it to the
/// agent, a fresh name, `verbatim: true`, `where` as the skeleton gives it, `to: none`, `kind: ack`
/// where the kind is left to the agent, and every optional fill omitted. None when an id the item
/// names does not exist.
#[must_use]
pub fn standard_fill(
    s: &State,
    sess: Option<SessionId>,
    sk: &Skeleton,
) -> Option<(SessionCtx, Cmd)> {
    let x = || Text(String::from("x"));
    let node = |n: NodeId| s.nodes.contains_key(&n).then_some(NodeRef::Id(n));
    let pkey = |p: PrincipalId| s.ix.principal_key.get(&p).cloned();
    let existing = || sess.map(SessionCtx::Existing);
    let cmd = match sk {
        Skeleton::Hello => {
            return Some((
                SessionCtx::New,
                Cmd::Session(SessionCmd::Hello {
                    intent: None,
                    caps: None,
                    horizon: None,
                    principal: None,
                    from_user: None,
                }),
            ));
        }
        Skeleton::LogFromUser { principal } => Cmd::Talk(TalkCmd::FromUser {
            text: x(),
            client: x(),
            principal: Some(pkey(*principal)?),
            answers: None,
            to: None,
            clarifies: None,
            reason: None,
        }),
        Skeleton::ViewNode { node: n } => Cmd::View(ViewCmd::Node {
            targets: alloc::vec![node(*n)?],
            view: None,
            page: None,
        }),
        Skeleton::Checkpoint {
            token,
            intent_done,
            intent,
        } => Cmd::Hold(HoldCmd::Checkpoint {
            token: *token,
            done: x(),
            next_step: x(),
            intent: intent.then(x),
            intent_done: intent_done.then(x),
            files: Vec::new(),
        }),
        Skeleton::CheckpointSwap { token } => Cmd::Hold(HoldCmd::Checkpoint {
            token: *token,
            done: x(),
            next_step: x(),
            intent: None,
            intent_done: Some(x()),
            files: Vec::new(),
        }),
        Skeleton::Release { token, wait } => Cmd::Hold(HoldCmd::Release {
            token: *token,
            done: x(),
            next_step: x(),
            reason: if wait.is_some() {
                ReleaseReason::Blocked
            } else {
                ReleaseReason::Handoff
            },
            waits_for: wait.map(WaitArg::Question),
            intent_done: None,
            learnings: None,
            unsent: BTreeSet::new(),
            settles: BTreeSet::new(),
        }),
        Skeleton::Take { target } => Cmd::Hold(HoldCmd::Take {
            target: node(*target)?,
            lease: None,
        }),
        Skeleton::Next => Cmd::Hold(HoldCmd::Next {
            query: NextQuery::default(),
            lease: None,
        }),
        Skeleton::CreateFor { entry } => Cmd::Node(NodeCmd::Create(create_args(
            Path(alloc::vec![fresh_name(s, crate::types::ROOT)?]),
            alloc::vec![Key {
                key: KeyStr::of_user_entry(*entry),
                nv: NormVer(0),
            }],
            BTreeSet::new(),
            None,
        ))),
        Skeleton::Split { parent } => {
            let mut path = path_to(s, *parent)?;
            path.0.push(fresh_name(s, *parent)?);
            let offer = Label::new(OFFER)?;
            Cmd::Node(NodeCmd::Create(create_args(
                path,
                Vec::new(),
                [offer].into_iter().collect(),
                Some(false),
            )))
        }
        Skeleton::AddOffer { node: n, version } => Cmd::Node(NodeCmd::Update {
            at: node(*n)?,
            version: *version,
            digest_version: None,
            token: None,
            patch: Patch {
                tags_add: [Label::new(OFFER)?].into_iter().collect(),
                ..Patch::default()
            },
        }),
        Skeleton::Close {
            node: n,
            token,
            settles,
            outcome,
        } => Cmd::Node(NodeCmd::Close {
            at: node(*n)?,
            outcome: outcome.clone().unwrap_or(Outcome::Done),
            summary: x(),
            verified: None,
            // 1.7 (§5): an item without `settles` leaves it to the default.
            settles: (!settles.is_empty()).then(|| settles.iter().copied().collect()),
            learnings: None,
            token: Some(*token),
            unsent: BTreeSet::new(),
        }),
        Skeleton::SayTo { at, to } => Cmd::Talk(TalkCmd::Say {
            at: node(*at)?,
            text: x(),
            to: Some(AddresseeArg::Session(*to)),
            expects: None,
            reply_to: None,
            until: None,
        }),
        Skeleton::ToUser {
            principal,
            kind,
            re,
        } => Cmd::Talk(TalkCmd::ToUser {
            text: x(),
            kind: kind.unwrap_or(ToUserKind::Ack),
            verbatim: true,
            principal: Some(pkey(*principal)?),
            re: re.iter().copied().collect(),
            at: None,
        }),
        Skeleton::Said {
            at,
            kind,
            where_,
            where_fill,
        } => {
            let w = match where_ {
                Some(k) => Some(key_on(s, *at, k)),
                None if *where_fill => {
                    let schemes = &s.params.conversation_schemes;
                    s.nodes
                        .get(at)?
                        .keys
                        .iter()
                        .find(|(k, _)| k.is_conversation(schemes))
                        .map(|(k, nv)| Key {
                            key: k.clone(),
                            nv: *nv,
                        })
                }
                None => None,
            };
            Cmd::Talk(TalkCmd::Said {
                at: node(*at)?,
                where_: w,
                text: x(),
                kind: *kind,
                corrects: None,
                seen: None,
            })
        }
        Skeleton::Heard {
            at,
            answers,
            where_,
        } => Cmd::Talk(TalkCmd::Heard {
            at: node(*at)?,
            where_: Some(key_on(s, *at, where_)),
            from: x(),
            text: x(),
            answers: Some(*answers),
        }),
        Skeleton::Decide {
            at,
            settles,
            supersedes,
        } => Cmd::Talk(TalkCmd::Decide {
            at: node(*at)?,
            text: x(),
            settles: settles.iter().copied().collect(),
            supersedes: *supersedes,
            basis: Vec::new(),
            evidence: None,
        }),
        Skeleton::Route { entry, to } => {
            let (to, answers, clarifies) = match to {
                RouteSk::Fill | RouteSk::Nowhere => {
                    (Some(crate::cmd::RouteTo::Nowhere), None, None)
                }
                RouteSk::Node(n) => (Some(crate::cmd::RouteTo::Node(node(*n)?)), None, None),
                RouteSk::Answers(q) => (None, Some(*q), None),
                RouteSk::Clarifies(r) => (None, None, Some(*r)),
            };
            Cmd::Talk(TalkCmd::Route {
                entry: *entry,
                to,
                answers,
                clarifies,
                reason: None,
            })
        }
        Skeleton::AnswerUser {
            question,
            principal,
        } => Cmd::Talk(TalkCmd::FromUser {
            text: x(),
            client: x(),
            principal: match principal {
                Some(p) => Some(pkey(*p)?),
                None => None,
            },
            answers: Some(*question),
            to: None,
            clarifies: None,
            reason: None,
        }),
        Skeleton::AnswerAgent { question } => {
            let q = s.aquestions.get(question)?;
            Cmd::Talk(TalkCmd::Answer {
                question: *question,
                node: q.node,
                asker: Some(q.asker),
                text: x(),
            })
        }
        Skeleton::Resolve { objection } => Cmd::Talk(TalkCmd::Resolve {
            objection: *objection,
            note: x(),
            upheld: false,
            defer: false,
        }),
        Skeleton::Merge { from, into } => Cmd::Node(NodeCmd::Merge {
            from: node(*from)?,
            into: node(*into)?,
        }),
        Skeleton::LinkDuplicate { from, to } => Cmd::Node(NodeCmd::Link {
            at: node(*from)?,
            rel: crate::types::LinkRel::Duplicate,
            to: node(*to)?,
        }),
        Skeleton::AskOwners { at } => Cmd::Talk(TalkCmd::Ask {
            at: node(*at)?,
            text: x(),
            to: AskTo::User(stale_ask()),
            again: false,
        }),
        Skeleton::Wait { timeout } => Cmd::Talk(TalkCmd::Wait {
            timeout: *timeout,
            for_: None,
            since: None,
        }),
        Skeleton::Reopen { node: n } => Cmd::Node(NodeCmd::Reopen {
            at: node(*n)?,
            reason: x(),
        }),
    };
    Some((existing()?, cmd))
}

/// The options of a stale offer's question for its owners (1.10, §16): keep it, or drop it.
pub const STALE_OPTIONS: [&str; 2] = ["keep", "drop"];

/// The default of a stale offer's question (1.10, §16): an offer nobody answers for is dropped by
/// the next taker; a default must be reversible (§8.4), and `node reopen` reverts a drop.
pub const STALE_DEFAULT: &str = "drop";

/// The human form of a stale offer's question (1.10, §16), as the shell decodes the item: `to:
/// user`, `options: [keep, drop]`, `default: drop`; the digest is the shell's, and any digest makes
/// the same call (D.11.5).
#[must_use]
pub fn stale_ask() -> crate::cmd::UserAsk {
    crate::cmd::UserAsk {
        to: None,
        options: STALE_OPTIONS
            .iter()
            .map(|o| Text(String::from(*o)))
            .collect(),
        recommendation: None,
        default: Some(Text(String::from(STALE_DEFAULT))),
        due: None,
        digest: crate::types::ReqDigest([0; 32]),
    }
}

/// A `create` with a path, keys, tags, and `take`, and title `"x"`.
fn create_args(
    path: Path,
    keys: Vec<Key>,
    tags: BTreeSet<Label>,
    take: Option<bool>,
) -> crate::cmd::CreateArgs {
    crate::cmd::CreateArgs {
        path,
        keys,
        kind: None,
        title: Text(String::from("x")),
        body: None,
        digest: None,
        fields: alloc::collections::BTreeMap::new(),
        tags,
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
        take,
    }
}

/// The unit tests of D.12.1 for the rules (WP-D7).
#[cfg(test)]
#[path = "guide_tests.rs"]
mod rule_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_number_in_the_order_of_section_20_9() {
        let all = [
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
            Rule::AskWait,
            Rule::AgentWait,
            Rule::JoinAnswer,
            Rule::Offer,
            Rule::AnswersNext,
            Rule::StatusReply,
            Rule::Review,
            Rule::RouteItem,
            Rule::HeldFlag,
            Rule::Inbox,
            Rule::AnswerAgent,
            Rule::HumanItems,
            Rule::Tidy,
            Rule::TakeJoined,
            Rule::MoreWork,
        ];
        // Declaration order is ascending number, so the derived order sorts items by rule.
        assert!(all.windows(2).all(|w| w.first() < w.get(1)));
        assert!(
            all.windows(2)
                .all(|w| w.first().map(|r| r.number()) <= w.get(1).map(|r| r.number()))
        );
        assert_eq!(Rule::Remedy.number(), 0);
        assert_eq!(Rule::MoreWork.number(), 13);
        let names: Vec<&str> = all.iter().map(|r| r.name()).collect();
        assert_eq!(
            names,
            [
                "remedy",
                "log_first",
                "tell_holder",
                "checkpoint",
                "intent_done",
                "reread",
                "route",
                "reply",
                "decide",
                "close",
                "release_container",
                "ask_wait",
                "agent_wait",
                "join_answer",
                "offer",
                "answers_next",
                "status_reply",
                "review",
                "route_item",
                "held_flag",
                "inbox",
                "answer_agent",
                "human_items",
                "tidy",
                "take_joined",
                "more_work",
            ]
        );
    }

    #[test]
    fn the_fixed_values_of_section_22() {
        assert_eq!(NEXT_CALLS_MAX, 3);
        assert_eq!(NEXT_TOTAL_CAP, 20);
        assert_eq!(NEXT_IDS_MAX, 5);
        // 1.9: rule 5's one item per entry; 1.7: the notes a write delivers, and `Underused`'s
        // floor.
        assert_eq!(ROUTE_ITEMS, 1);
        assert_eq!(crate::query::WRITE_NOTES_MAX, 2);
        assert_eq!(crate::health::UNDERUSED_NEXT_CALLS, 20);
        assert_eq!(crate::health::UNDERUSED_SESSIONS, 3);
        // §22's values in every build, the feature `conformance` included: a workspace build
        // unifies features, so `cargo test --workspace` runs this with it on, and the server it
        // links must see the same values.
        assert_eq!(LEASE_MARGIN, crate::types::MINUTE);
        assert_eq!(CHECKPOINT_EVERY, Dur(5 * crate::types::MINUTE.0));
        // The model's ticks as minutes (D.10.5), for `guide_items` only.
        assert_eq!(CHECKPOINT_EVERY_TICKS, Dur(2 * crate::types::MINUTE.0));
        assert_eq!(LEASE_MARGIN_TICKS, crate::types::MINUTE);
        // The orders D.10.5 keeps: the margin below the checkpoint period, below `lease.first`.
        let first = crate::params::Params::default().lease_first;
        assert!(LEASE_MARGIN < CHECKPOINT_EVERY && CHECKPOINT_EVERY < first);
        assert!(LEASE_MARGIN_TICKS < CHECKPOINT_EVERY_TICKS);
        assert!(CHECKPOINT_EVERY_TICKS < Dur(3 * crate::types::MINUTE.0));
    }
}
