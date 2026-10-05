//! The context and the commands (design D.5).
//!
//! Every operation of §20.3 to §20.8 is one variant, with every parameter. Paths and ids arrive as
//! `NodeRef`, keys arrive normalised with their normaliser version, free text arrives masked, cron
//! arrives parsed with its frozen tz table, and times arrive as `When` (the shell does all of this
//! before building the command, §24). Defaults that the specification states (`take: true`,
//! `capacity: 1`, `priority: 0`, `view: brief`) are applied by the core, not by the parser, so that
//! the logged command shows what the agent sent.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::params::Params;
use crate::types::{
    Change, ClassName, DecisionId, Dur, Eid, EntryType, Evidence, Expects, FlagSet, Key, KeyStr,
    Label, LinkRel, NodeId, NodeRef, ObjectionId, OpId, Outcome, Path, PrincipalKey, QuestionId,
    ReleaseReason, ReqDigest, RequestId, SaidId, SaidKind, Seq, SessionId, Text, ToUserKind, Token,
    UserEntryId, Version, ViewMode, When,
};

// ------------------------------------------------------------------------------------------------
// Context (D.5.1)

/// §24 gives `Ctx{now, class, caps, session, op = {id, digest}}` (A-107); `caps` is the token's
/// default (CS-26), and no principal comes from the token (FQ-5).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ctx {
    /// The shell's clock reading. The core clamps it again: `now := max(ctx.now, s.last_now)` (T1).
    pub now: crate::types::Time,
    /// Bound to the bearer token by the server configuration (§3). Never used for permissions.
    /// Ignored under `SessionCtx::System`.
    pub class: ClassName,
    /// The default caps bound to the bearer token (its `caps` field, D.11.3) (CS-26): a session's
    /// caps, to which those `hello` passes are added (1.6, `hello_caps`). None: no default. Never a
    /// permission. Ignored under `SessionCtx::System`.
    pub caps: Option<BTreeSet<Label>>,
    /// Which session the call runs in.
    pub session: SessionCtx,
    /// Retry identity (§20.1, O1). Always None under `SessionCtx::System`.
    pub op: Option<OpCtx>,
}

/// The caps `hello` gives a session (§3, §20.3, 1.6): those it passes added to the token's default
/// caps, never in their place; the token's alone when it passes none; None when neither has any.
/// In the lab's research forum (1.5), an agent's own caps replaced its token's and hid the work
/// meant for it.
#[must_use]
pub fn hello_caps(
    passed: Option<&BTreeSet<Label>>,
    token: Option<&BTreeSet<Label>>,
) -> Option<BTreeSet<Label>> {
    match (passed, token) {
        (Some(p), Some(t)) => Some(p.union(t).cloned().collect()),
        (Some(p), None) => Some(p.clone()),
        (None, t) => t.cloned(),
    }
}

/// Which session a call runs in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SessionCtx {
    /// This call opens a session (§20.1).
    New,
    /// An existing session, named by the agent or bound to its MCP session (D.11.4).
    Existing(SessionId),
    /// The server itself, at startup: only `Cmd::Admin` (D.11.8). No session is opened or touched,
    /// no `op_id` is taken, and neither `classes` nor `sessions` changes, so a parameter change
    /// never makes a class "active" for E1.
    System,
}

/// The retry identity of a call. The digest travels with the `op_id` because O1 compares "the
/// request digest", and only the shell sees the request as the agent sent it (D.8.1, A-39).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OpCtx {
    /// The client's retry id.
    pub id: OpId,
    /// SHA-256 of the masked request (D.11.5).
    pub digest: ReqDigest,
}

// ------------------------------------------------------------------------------------------------
// Commands (D.5.2)

/// One command: an operation of §20.3 to §20.8, or the shell's parameter change.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(
    clippy::large_enum_variant,
    reason = "the command shapes are the frozen contract of design D.5.2"
)]
pub enum Cmd {
    /// The `session` tool (§20.3).
    Session(SessionCmd),
    /// The `view` tool (§20.4).
    View(ViewCmd),
    /// The `node` tool (§20.5).
    Node(NodeCmd),
    /// The `hold` tool (§20.6).
    Hold(HoldCmd),
    /// The `talk` tool (§20.7).
    Talk(TalkCmd),
    /// The `control` tool (§20.8).
    Control(ControlCmd),
    /// Not an MCP tool. Issued by the shell at startup when the configured parameters differ from
    /// the logged ones (D.11.9).
    Admin(AdminCmd),
}

// ---------------------------------------------------------------- session (§20.3)

/// The actions of `session`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(
    clippy::large_enum_variant,
    reason = "hello carries its from_user, with 1.7's routing arguments, inline: one per call"
)]
pub enum SessionCmd {
    /// `hello {intent?, caps?, horizon?, principal?, from_user?: {text, client, principal?, to?,
    /// answers?, clarifies?, reason?}}` (CS-1: `principal` binds the session to a human without a
    /// message; 1.7: `from_user` routes as `talk from_user` does).
    Hello {
        /// What the session means to do.
        intent: Option<Text>,
        /// The caps it works with.
        caps: Option<BTreeSet<Label>>,
        /// Minutes.
        horizon: Option<u32>,
        /// The human it serves.
        principal: Option<PrincipalKey>,
        /// A human's message to log first.
        from_user: Option<FromUserArg>,
    },
    /// `status {text}`.
    Status {
        /// The status text.
        text: Text,
    },
}

/// The `from_user` of `hello`. `principal` defaults to `hello`'s own `principal` argument, then to
/// D.8.11's resolution.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FromUserArg {
    /// The message, verbatim.
    pub text: Text,
    /// The client it came through.
    pub client: Text,
    /// Whose words.
    pub principal: Option<PrincipalKey>,
    /// The question for the human these words answer, as `talk from_user {answers}` takes it (1.6,
    /// §8.2: agents logged the same words twice when `hello` could not mark an answer).
    #[cfg_attr(feature = "serde", serde(default))]
    pub answers: Option<Eid>,
    /// Where the words go, as `talk from_user {to}` takes it (1.7, §8.2).
    #[cfg_attr(feature = "serde", serde(default))]
    pub to: Option<RouteTo>,
    /// The request these words clarify, as `talk from_user {clarifies}` takes it (1.7).
    #[cfg_attr(feature = "serde", serde(default))]
    pub clarifies: Option<Eid>,
    /// Why the words need no work, with `to: none` (1.7).
    #[cfg_attr(feature = "serde", serde(default))]
    pub reason: Option<Text>,
}

/// The `to` of `from_user` and `route` (1.7, §8.2): a node, as a new request there, or `none`, no
/// action.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RouteTo {
    /// A node.
    Node(NodeRef),
    /// `none`: the words need no work.
    Nowhere,
}

impl RouteTo {
    /// The node, if the target is one.
    #[must_use]
    pub fn node(&self) -> Option<&NodeRef> {
        match self {
            RouteTo::Node(n) => Some(n),
            RouteTo::Nowhere => None,
        }
    }
}

// ---------------------------------------------------------------- view (§20.4)

/// The actions of `view`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ViewCmd {
    /// `node {path | paths, view?: brief|full|review, page?}`: `page` (default 0) pages the lists
    /// of what rests on each node in `view: full`, `BACKLINKS_MAX` per list (CS-22).
    Node {
        /// The nodes.
        targets: Vec<NodeRef>,
        /// The view, default brief.
        view: Option<ViewMode>,
        /// The page of what rests on each node.
        page: Option<u32>,
    },
    /// `find {key | text}`.
    Find {
        /// By key or by text.
        by: FindBy,
    },
    /// `feed {at, since?, types?, limit?}`.
    Feed {
        /// The node.
        at: NodeRef,
        /// After this seq.
        since: Option<Seq>,
        /// Only these types; empty: all.
        types: BTreeSet<EntryType>,
        /// Page size.
        limit: Option<u32>,
    },
    /// `list {under?, kind?, tags?, status?, fields?, flags?, closed?}`.
    List {
        /// The subtree.
        under: Option<NodeRef>,
        /// The kind.
        kind: Option<Label>,
        /// Tags every listed node carries.
        tags: BTreeSet<Label>,
        /// The status.
        status: Option<Text>,
        /// Field values.
        fields: BTreeMap<Text, Text>,
        /// Flags every listed node carries.
        flags: FlagSet,
        /// None: open + not archived (AR).
        closed: Option<bool>,
        /// An addition (A-78).
        limit: Option<u32>,
    },
    /// `history {path, before?, limit?}`: served by the shell from the history, newest first, a
    /// page of events with `seq < before` (A-78, §18).
    History {
        /// The node.
        at: NodeRef,
        /// Before this event.
        before: Option<u64>,
        /// Page size.
        limit: Option<u32>,
    },
    /// `requests {status?, requester?: me | all | <principal>}` (CS-25). None: `all` while `user` is
    /// the only interned principal; otherwise `me` for a session that serves any principal and
    /// `all` for one that serves none (§8.3).
    Requests {
        /// The board state.
        status: Option<BoardState>,
        /// Whose requests.
        requester: Option<RequesterFilter>,
    },
    /// `user_log {since?, limit?}`.
    UserLog {
        /// After this entry.
        since: Option<Eid>,
        /// Page size.
        limit: Option<u32>,
    },
    /// `constitution {}`.
    Constitution,
    /// `agreements {under?, limit?}` (1.4, §11.2, §20.4, D.14.5): the nodes of the subtree of
    /// `under` (default `/`) that have agreements, 50 by default and at most 500, with a total.
    Agreements {
        /// The subtree; ROOT when absent.
        under: Option<NodeRef>,
        /// Page size.
        limit: Option<u32>,
    },
}

/// `Text` is a shell-only search; the core only touches the session (A-40).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FindBy {
    /// By key.
    Key(KeyStr),
    /// By text (the shell's search).
    Text(Text),
}

/// The states of a row of the request board (§8.3, A-35).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum BoardState {
    /// An unrouted user entry.
    Unrouted,
    /// Answered.
    Answered,
    /// Its node closed without an answer.
    ClosedUnanswered,
    /// A decision that is not retracted lists it.
    Decided,
    /// Its node is waiting.
    Waiting,
    /// Otherwise.
    InProgress,
}

/// A view filter, never a permission (CS-25): `All` still shows every request to anyone.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RequesterFilter {
    /// The session's principals and their aliases.
    Mine,
    /// Every row.
    All,
    /// One principal's.
    Principal(PrincipalKey),
}

// ---------------------------------------------------------------- node (§20.5)

/// The actions of `node`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NodeCmd {
    /// `create {…}`.
    Create(CreateArgs),
    /// `update {path, version, digest_version?, …, token?}`: `digest_version` is required when the
    /// patch sets `digest` (A-4, CS-11).
    Update {
        /// The node.
        at: NodeRef,
        /// The version the agent read.
        version: Version,
        /// The digest version the agent read.
        digest_version: Option<Version>,
        /// The agent's hold, if it holds the node.
        token: Option<Token>,
        /// The changes.
        patch: Patch,
    },
    /// `link {path, rel, to}`.
    Link {
        /// The source.
        at: NodeRef,
        /// The relation.
        rel: LinkRel,
        /// The target.
        to: NodeRef,
    },
    /// `key {path, add? | remove?}`.
    Key {
        /// The node.
        at: NodeRef,
        /// The change.
        change: KeyChange,
    },
    /// `close {path, outcome, summary, verified?, settles?, learnings?, token?, unsent?}`.
    /// `settles`: None settles by default the requests a result answered (1.7, §5); `Some(∅)`
    /// settles none.
    Close {
        /// The node.
        at: NodeRef,
        /// The outcome.
        outcome: Outcome,
        /// The summary.
        summary: Text,
        /// How it was verified.
        verified: Option<Text>,
        /// The requests the summary's decision settles; None: the default of §5.
        settles: Option<BTreeSet<RequestId>>,
        /// Stored as text only (§29: later).
        learnings: Option<Text>,
        /// The agent's hold, if it holds the node.
        token: Option<Token>,
        /// `said` entries the agent did not send.
        unsent: BTreeSet<SaidId>,
    },
    /// `reopen {path, reason}`.
    Reopen {
        /// The node.
        at: NodeRef,
        /// Why.
        reason: Text,
    },
    /// `move {path, to}`.
    Move {
        /// The node.
        at: NodeRef,
        /// The new path.
        to: Path,
    },
    /// `merge {from, into}`.
    Merge {
        /// The node merged away.
        from: NodeRef,
        /// The node that remains.
        into: NodeRef,
    },
    /// `hide {entry | file, reason}`.
    Hide {
        /// What to hide.
        target: HideArg,
        /// Why.
        reason: Text,
    },
}

/// `create {path, keys?, kind?, title, body?, digest?, fields?, tags?, capacity?, needs?, est?,
/// priority?, due?, depends_on?, owners?, repeat?, agreements?, request?, take?: true}`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateArgs {
    /// The path of the new node.
    pub path: Path,
    /// Deduplicated by the core.
    pub keys: Vec<Key>,
    /// The kind.
    pub kind: Option<Label>,
    /// The title.
    pub title: Text,
    /// The body.
    pub body: Option<Text>,
    /// The digest.
    pub digest: Option<Text>,
    /// No `files`: §20.5 has none (A-97).
    pub fields: BTreeMap<Text, Text>,
    /// The tags.
    pub tags: BTreeSet<Label>,
    /// Default 1.
    pub capacity: Option<u32>,
    /// None: the parent's needs (CS-26), so a team's subtree stays its team's work; `[]` given
    /// explicitly: no needs.
    pub needs: Option<BTreeSet<Label>>,
    /// Minutes.
    pub est: Option<u32>,
    /// Default 0.
    pub priority: Option<i32>,
    /// The due time.
    pub due: Option<When>,
    /// The nodes this one depends on.
    pub depends_on: Vec<NodeRef>,
    /// Person or team nodes (D.2.2, D-3); empty: inherited from the nearest owned ancestor.
    pub owners: Vec<NodeRef>,
    /// A schedule.
    pub repeat: Option<RepeatArg>,
    /// Process agreements.
    pub agreements: Option<Text>,
    /// A request to record with the node.
    pub request: Option<RequestArg>,
    /// Default true.
    pub take: Option<bool>,
}

/// `repeat: {cron, tz?, run_within}`; parsed and frozen by the shell (D.11.10).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RepeatArg {
    /// The cron as the agent wrote it.
    pub cron_text: Text,
    /// The parsed cron.
    pub cron: forum_cron::CronSpec,
    /// `UTC` when `tz` is omitted.
    pub tz: forum_cron::TzTable,
    /// The window.
    pub run_within: Dur,
}

/// `request: {text, origin?, where?, requester}` (A-52, A-61). `origin` is the message the request
/// came in, `where` its conversation (D.8.9): `where` defaults to the first conversation key among
/// `keys`, else the origin; `origin` defaults to `where`, else the first conversation key among
/// `keys`, else the first key.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RequestArg {
    /// The request.
    pub text: Text,
    /// The message it came in.
    pub origin: Option<Key>,
    /// Its conversation.
    pub where_: Option<Key>,
    /// Who asked, as text.
    pub requester: Text,
}

/// The `…` of update. None (or `Change::Keep`, or an empty set) leaves a field unchanged;
/// `Change::Clear` clears an optional field (D.2: never `Option<Option<T>>`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Patch {
    /// The title.
    pub title: Option<Text>,
    /// The body.
    pub body: Option<Text>,
    /// The digest (requires `digest_version`, CS-11).
    pub digest: Option<Text>,
    /// The fields, replaced whole.
    pub fields: Option<BTreeMap<Text, Text>>,
    /// The node's files (A-97).
    pub files: Option<BTreeMap<Text, Text>>,
    /// The status.
    pub status: Option<Text>,
    /// Empty clears.
    pub agreements: Option<Text>,
    /// The kind.
    #[cfg_attr(feature = "serde", serde(default))]
    pub kind: Change<Label>,
    /// Tags and needs change by adding and removing labels, never by replacing the set, so two
    /// patches from the same version commute and neither loses the other's labels (CS-34). Within
    /// one patch the removals apply first, then the additions.
    pub tags_add: BTreeSet<Label>,
    /// Tags to remove.
    pub tags_remove: BTreeSet<Label>,
    /// The capacity.
    pub capacity: Option<u32>,
    /// The priority.
    pub priority: Option<i32>,
    /// The estimate.
    #[cfg_attr(feature = "serde", serde(default))]
    pub est: Change<u32>,
    /// Needs to add.
    pub needs_add: BTreeSet<Label>,
    /// Needs to remove.
    pub needs_remove: BTreeSet<Label>,
    /// The due time.
    #[cfg_attr(feature = "serde", serde(default))]
    pub due: Change<When>,
    /// The dependencies, replaced whole.
    pub depends_on: Option<Vec<NodeRef>>,
    /// Replaces the owners (D-3); an empty list clears them, so the node inherits again.
    pub owners: Option<Vec<NodeRef>>,
    /// The schedule.
    #[cfg_attr(feature = "serde", serde(default))]
    pub repeat: Change<RepeatArg>,
}

/// `key {add? | remove?}`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KeyChange {
    /// Add a key.
    Add(Key),
    /// Remove a key.
    Remove(KeyStr),
}

/// `node`: the entry's node, which the shell looks up in its `entries` table and adds, because the
/// core keeps no metadata for most entries (CS-17, D.4.5); an unknown eid is `not_found` there.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HideArg {
    /// An entry.
    Entry {
        /// The entry.
        eid: Eid,
        /// Its node, added by the shell.
        node: NodeId,
    },
    /// A file of a node.
    File {
        /// The node.
        at: NodeRef,
        /// The file's name.
        name: Text,
    },
}

// ---------------------------------------------------------------- hold (§20.6)

/// The actions of `hold`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HoldCmd {
    /// `take {target, lease?}`: `lease` is L for later extensions (A-8).
    Take {
        /// The node.
        target: NodeRef,
        /// L of §10.1.
        lease: Option<Dur>,
    },
    /// `next {query?, lease?}`.
    Next {
        /// The filter.
        query: NextQuery,
        /// L of §10.1.
        lease: Option<Dur>,
    },
    /// `checkpoint {token, done, next_step, intent?, intent_done?, files?}`.
    Checkpoint {
        /// The hold.
        token: Token,
        /// What was done.
        done: Text,
        /// What comes next.
        next_step: Text,
        /// An intent before an outside action.
        intent: Option<Text>,
        /// What happened to the open intent.
        intent_done: Option<Text>,
        /// The step's files: entry text only (A-97).
        files: Vec<Text>,
    },
    /// `renew {token, why}`.
    Renew {
        /// The hold.
        token: Token,
        /// Why.
        why: Text,
    },
    /// `release {token, done, next_step, reason, waits_for?, intent_done?, settles?, learnings?,
    /// unsent?}`.
    Release {
        /// The hold.
        token: Token,
        /// What was done.
        done: Text,
        /// What comes next.
        next_step: Text,
        /// Required (§20.6); Blocked whenever `waits_for` is given (A-98).
        reason: ReleaseReason,
        /// What the node waits for.
        waits_for: Option<WaitArg>,
        /// 1.4 (§10.3, R15): closes the node's open intent, as a checkpoint's does.
        intent_done: Option<Text>,
        /// Stored as text only.
        learnings: Option<Text>,
        /// `said` entries the agent did not send.
        unsent: BTreeSet<SaidId>,
        /// 1.12 (§10.3): the requests the release's decision lists, recorded after the release
        /// with `done` as its text; empty: no decision, and nothing settles.
        #[cfg_attr(feature = "serde", serde(default))]
        settles: BTreeSet<RequestId>,
    },
    /// `check {target, token?, intent_done?}`.
    Check {
        /// The node.
        target: NodeRef,
        /// A token to check.
        token: Option<Token>,
        /// 1.8 (§10.2): what became of the node's open intent, recorded without a hold; a write.
        #[cfg_attr(feature = "serde", serde(default))]
        intent_done: Option<Text>,
    },
}

/// `next {query?}` is a structured filter (A-20). The empty filter matches everything.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NextQuery {
    /// Ancestor-or-self.
    pub under: Option<NodeRef>,
    /// The kind.
    pub kind: Option<Label>,
    /// Tags the node carries.
    pub tags: BTreeSet<Label>,
}

/// What a release waits for.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WaitArg {
    /// A people or a user question (A-31).
    Question(QuestionId),
    /// A node.
    Node(NodeRef),
    /// A time.
    Time(When),
    /// Something outside the forum (A-79).
    External {
        /// What.
        what: Text,
        /// When to look again.
        recheck_after: When,
    },
}

// ---------------------------------------------------------------- talk (§20.7)

/// The actions of `talk`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TalkCmd {
    /// `from_user {text, client, principal?, to?, answers?, clarifies?, reason?}`; precedence
    /// answers > clarifies > to (A-99); `principal`: whose words these are (CS-1, D.8.11); `to`
    /// (1.7, the shell reads `node` as `to`): a node, or none with `reason`.
    FromUser {
        /// The message, verbatim.
        text: Text,
        /// The client it came through.
        client: Text,
        /// Whose words.
        principal: Option<PrincipalKey>,
        /// A question for the user it answers.
        answers: Option<QuestionId>,
        /// Where to route it.
        to: Option<RouteTo>,
        /// A request it clarifies.
        clarifies: Option<RequestId>,
        /// Why it needs no work, with `to: none`; by default "no work needed".
        reason: Option<Text>,
    },
    /// `to_user {text, kind, verbatim?, principal?, re?, at?}`; `principal`: whom the report is to
    /// (CS-4). `verbatim` is optional since 1.6 (§8.1): the shell gives false without it. `at`
    /// (1.8, §8.1): the node whose work it reports, whose requester it is to without `principal`,
    /// and whose requests of that person it names without `re`.
    ToUser {
        /// The report.
        text: Text,
        /// Its kind.
        kind: ToUserKind,
        /// Whether the text is what the human sees.
        verbatim: bool,
        /// To whom.
        principal: Option<PrincipalKey>,
        /// The requests, user entries, and digest entries it reports on.
        re: BTreeSet<Eid>,
        /// The node whose work it reports (1.8).
        #[cfg_attr(feature = "serde", serde(default))]
        at: Option<NodeRef>,
    },
    /// `route {entry, to?, answers?, clarifies?, reason?}` (1.7, §8.2): the first of `answers`,
    /// `clarifies`, and `to` routes; the shell requires one of them.
    Route {
        /// The user entry.
        entry: UserEntryId,
        /// A node, or none.
        to: Option<RouteTo>,
        /// A question for the user it answers.
        answers: Option<QuestionId>,
        /// A request it clarifies.
        clarifies: Option<RequestId>,
        /// Why it needs no work, with `to: none`; by default "no work needed".
        reason: Option<Text>,
    },
    /// `request {at, text, origin, where?, requester}`: `origin` is the message, `where` the
    /// conversation, by default the origin (A-61, D.8.15).
    Request {
        /// The node.
        at: NodeRef,
        /// The request.
        text: Text,
        /// The message it came in.
        origin: Key,
        /// Its conversation.
        where_: Option<Key>,
        /// Who asked, as text.
        requester: Text,
    },
    /// `said {at, where?, text, kind, corrects?, seen?}`.
    Said {
        /// The node.
        at: NodeRef,
        /// The conversation.
        where_: Option<Key>,
        /// What was said.
        text: Text,
        /// `Ack | Reply | Result | Correction`.
        kind: SaidKind,
        /// The `said` it corrects.
        corrects: Option<SaidId>,
        /// The last seq the agent read.
        seen: Option<Seq>,
    },
    /// `said {said_id, confirmed: <link>}`.
    SaidConfirm {
        /// The `said`.
        said: SaidId,
        /// A link to the sent message.
        confirmed: Text,
    },
    /// `heard {at, where?, from, text, answers?}`.
    Heard {
        /// The node.
        at: NodeRef,
        /// The conversation.
        where_: Option<Key>,
        /// Who said it.
        from: Text,
        /// What they said.
        text: Text,
        /// A people question it answers.
        answers: Option<QuestionId>,
    },
    /// `ask {at, text, to? | where?, again?, blocking?}`, plus the fields of a user question
    /// (A-16); `again` also applies to a question for the user (D.8.12). `blocking` belongs to the
    /// form for a person only (1.4, §20.7, R27), so it lives in `AskTo::Person`.
    Ask {
        /// The node.
        at: NodeRef,
        /// The question.
        text: Text,
        /// Whom.
        to: AskTo,
        /// Ask again although a question is open.
        again: bool,
    },
    /// `answer {question, text}`; `node` and `asker` are added by the shell from its `entries`
    /// table (the question's node and author), because an agent question leaves memory at `until`
    /// (D.4.12).
    Answer {
        /// The agent question.
        question: QuestionId,
        /// Its node.
        node: NodeId,
        /// Its author.
        asker: Option<SessionId>,
        /// The answer.
        text: Text,
    },
    /// `say {at, text, to?, expects?, reply_to?, until?}`.
    Say {
        /// The node.
        at: NodeRef,
        /// The message.
        text: Text,
        /// The addressee; None: a plain note (A-17).
        to: Option<AddresseeArg>,
        /// Stored only.
        expects: Option<Expects>,
        /// The entry it replies to.
        reply_to: Option<Eid>,
        /// Delivered until then.
        until: Option<When>,
    },
    /// `decide {at, text, settles?, supersedes?, basis?, evidence?}`.
    Decide {
        /// The node.
        at: NodeRef,
        /// The decision.
        text: Text,
        /// The requests it settles.
        settles: BTreeSet<RequestId>,
        /// The decision it supersedes.
        supersedes: Option<DecisionId>,
        /// References (CS-22), in order.
        basis: Vec<BasisArg>,
        /// Stored only (§29: later).
        evidence: Option<Evidence>,
    },
    /// `approve {on: id@version, checked}`.
    Approve {
        /// The node.
        on: NodeRef,
        /// The version approved.
        version: Version,
        /// What was checked.
        checked: Text,
    },
    /// `object {on, text}`: the argument `text` fills `note` (1.6).
    Object {
        /// The node.
        on: NodeRef,
        /// The objection.
        note: Text,
    },
    /// `resolve {objection, text, upheld? | defer?}`: the argument `text` fills `note` (1.6).
    Resolve {
        /// The objection.
        objection: ObjectionId,
        /// How it was resolved; for a deferral, why the caller cannot decide it yet.
        note: Text,
        /// 1.8 (§11.3): the objection stands; on a closed node it corrects the node.
        #[cfg_attr(feature = "serde", serde(default))]
        upheld: bool,
        /// 1.11 (§11.3): the caller cannot decide it yet: a deferral, which decides nothing and,
        /// on a closed node, backs the caller's class off its review (§10.3). Never with `upheld`.
        #[cfg_attr(feature = "serde", serde(default))]
        defer: bool,
    },
    /// `retract {entry | path, text, reason}`.
    Retract {
        /// What is retracted.
        target: RetractArg,
        /// The statement.
        text: Text,
        /// Why.
        reason: Text,
    },
    /// `wait {timeout, for?}`: the long poll is in the shell (D.11.11); `for` per A-19. `since` is
    /// added by the shell: the last `seq` of `for` it reported to this session, from its own memory
    /// (None: the session's `shown`), so that news needs no `Shown` (D.8.16).
    Wait {
        /// How long to wait.
        timeout: Dur,
        /// A node whose news to wait for.
        for_: Option<NodeRef>,
        /// The last seq reported.
        since: Option<Seq>,
    },
    /// `who {at?}`.
    Who {
        /// A subtree.
        at: Option<NodeRef>,
    },
}

/// `ask {to? | where?}`: a person (default, no `to`), an agent (`to: session:… | holder:… |
/// holders:…`), or a principal (`to: "user"` or `to: "principal:<key>"`). The agent addressee has
/// no `User` variant, so each `to` decodes to exactly one arm. Each arm carries only the arguments
/// of its form (1.4, §20.7): the shell refuses an argument of another form with `invalid` naming
/// it (D.11.5), so `blocking` and `where` never reach a question for a human.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AskTo {
    /// A person outside the swarm.
    Person {
        /// The conversation.
        where_: Option<Key>,
        /// A blocking question escalates (§9.3).
        blocking: bool,
    },
    /// Agents.
    Agent(AgentAddressee),
    /// A human the swarm serves.
    User(UserAsk),
}

/// An agent addressee of `ask`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AgentAddressee {
    /// One session.
    Session(SessionId),
    /// Whoever holds the node.
    Holder(NodeRef),
    /// Whoever holds work in the branch.
    Holders(NodeRef),
}

/// A question for the user (§8.4, A-16).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserAsk {
    /// `principal:<key>`: the first stage of the chain; None for a bare `user`, whose chain starts
    /// with the requesters of `at` (CS-2, D.8.12).
    pub to: Option<PrincipalKey>,
    /// The options.
    pub options: Vec<Text>,
    /// The recommended option.
    pub recommendation: Option<Text>,
    /// Must be reversible (§8.4).
    pub default: Option<Text>,
    /// A-68: clamped to `>= now`.
    pub due: Option<When>,
    /// SHA-256 of the masked text and options, computed by the shell (D.11.5): an identical
    /// question pending for the same first addressee is returned instead of a new one (D.8.12).
    pub digest: ReqDigest,
}

/// The addressee of `say`. `User` is a bare `user`, resolved to the head of `at`'s chain when the
/// note is recorded (CS-3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AddresseeArg {
    /// One session.
    Session(SessionId),
    /// Whoever holds the node, or takes it next.
    Holder(NodeRef),
    /// Whoever holds work in the branch.
    Holders(NodeRef),
    /// The human nearest to `at` (CS-3).
    User,
    /// A named human.
    Principal(PrincipalKey),
}

/// One basis reference as the agent wrote it (§15, CS-22): `#12@4`, `/a/b@4`, or `#12` (a node, at
/// a version or not); `e57` or `entry:e57` (an entry); `{external: what, checked_at, recheck?}`. The
/// core resolves and validates it into a `BasisRef` (D.8.17).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BasisArg {
    /// A node, at a version or not.
    Node {
        /// The node.
        at: NodeRef,
        /// The version cited.
        version: Option<Version>,
    },
    /// An entry.
    Entry(Eid),
    /// Something outside the forum.
    External {
        /// What.
        what: Text,
        /// When it was checked.
        checked_at: When,
        /// When to check again.
        recheck: Option<When>,
    },
}

/// What `retract` strikes out. `node`: as `HideArg`'s.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RetractArg {
    /// An entry.
    Entry {
        /// The entry.
        eid: Eid,
        /// Its node, added by the shell.
        node: NodeId,
    },
    /// A node.
    Node(NodeRef),
}

// ---------------------------------------------------------------- control (§20.8)

/// The actions of `control`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ControlCmd {
    /// `pause {scope, reason}`.
    Pause {
        /// The scope.
        scope: NodeRef,
        /// Why.
        reason: Text,
    },
    /// `unpause {scope}`.
    Unpause {
        /// The scope.
        scope: NodeRef,
    },
    /// `health {}`.
    Health,
    /// `constitution {…}`: read-only in the first release (A-21).
    Constitution,
    /// `export {}`: paginated by the shell (A-78).
    Export {
        /// After this event.
        after: Option<u64>,
        /// Page size.
        limit: Option<u32>,
    },
}

/// Only under `SessionCtx::System` (D.5.1).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AdminCmd {
    /// Replace the parameters (D.11.9).
    SetParams(Params),
}

// ------------------------------------------------------------------------------------------------
// Measures and classes of commands

/// The longest of `texts` in bytes, 0 for none.
fn longest<'a>(texts: impl IntoIterator<Item = &'a Text>) -> u64 {
    texts.into_iter().map(Text::byte_len).max().unwrap_or(0)
}

/// Every name and value of a map of texts.
fn pairs(map: &BTreeMap<Text, Text>) -> impl Iterator<Item = &Text> {
    map.iter().flat_map(|(k, v)| [k, v])
}

impl Cmd {
    /// The byte length of the command's longest `Text`, the measure of `text.max` (§19, L1,
    /// A-114): every `Text` of every variant counts on its own, among them each name and value of
    /// `fields` and `files`, each option of a user question, a checkpoint's texts, a `from_user`
    /// text and its client, and a basis reference's `what`; `Admin` has none. A total function
    /// with one arm per variant, so that a new `Text` field cannot be missed silently.
    #[must_use]
    pub fn longest_text(&self) -> u64 {
        match self {
            Cmd::Session(c) => c.longest_text(),
            Cmd::View(c) => c.longest_text(),
            Cmd::Node(c) => c.longest_text(),
            Cmd::Hold(c) => c.longest_text(),
            Cmd::Talk(c) => c.longest_text(),
            Cmd::Control(c) => c.longest_text(),
            Cmd::Admin(AdminCmd::SetParams(_)) => 0,
        }
    }

    /// Whether the command only reads, for `write.per_hour` (D.11.15): `View(_)`, `Hold(Check)`
    /// without `intent_done` (1.8), `Talk(Who)`, `Talk(Wait)`, `Session(Hello)`, and
    /// `Control(Health | Constitution | Export)`; every other command is a write.
    #[must_use]
    pub fn is_read(&self) -> bool {
        match self {
            Cmd::Hold(HoldCmd::Check { intent_done, .. }) => intent_done.is_none(),
            Cmd::View(_)
            | Cmd::Talk(TalkCmd::Who { .. } | TalkCmd::Wait { .. })
            | Cmd::Session(SessionCmd::Hello { .. })
            | Cmd::Control(
                ControlCmd::Health | ControlCmd::Constitution | ControlCmd::Export { .. },
            ) => true,
            Cmd::Session(SessionCmd::Status { .. })
            | Cmd::Node(_)
            | Cmd::Hold(
                HoldCmd::Take { .. }
                | HoldCmd::Next { .. }
                | HoldCmd::Checkpoint { .. }
                | HoldCmd::Renew { .. }
                | HoldCmd::Release { .. },
            )
            | Cmd::Talk(
                TalkCmd::FromUser { .. }
                | TalkCmd::ToUser { .. }
                | TalkCmd::Route { .. }
                | TalkCmd::Request { .. }
                | TalkCmd::Said { .. }
                | TalkCmd::SaidConfirm { .. }
                | TalkCmd::Heard { .. }
                | TalkCmd::Ask { .. }
                | TalkCmd::Answer { .. }
                | TalkCmd::Say { .. }
                | TalkCmd::Decide { .. }
                | TalkCmd::Approve { .. }
                | TalkCmd::Object { .. }
                | TalkCmd::Resolve { .. }
                | TalkCmd::Retract { .. },
            )
            | Cmd::Control(ControlCmd::Pause { .. } | ControlCmd::Unpause { .. })
            | Cmd::Admin(_) => false,
        }
    }
}

impl SessionCmd {
    fn longest_text(&self) -> u64 {
        match self {
            SessionCmd::Hello {
                intent,
                caps: _,
                horizon: _,
                principal: _,
                from_user,
            } => {
                let from_user = from_user.iter().flat_map(|f| {
                    let FromUserArg {
                        text,
                        client,
                        principal: _,
                        answers: _,
                        to: _,
                        clarifies: _,
                        reason,
                    } = f;
                    [text, client].into_iter().chain(reason)
                });
                longest(intent.iter().chain(from_user))
            }
            SessionCmd::Status { text } => text.byte_len(),
        }
    }
}

impl ViewCmd {
    fn longest_text(&self) -> u64 {
        match self {
            ViewCmd::Find { by } => match by {
                FindBy::Text(t) => t.byte_len(),
                FindBy::Key(_) => 0,
            },
            ViewCmd::List {
                under: _,
                kind: _,
                tags: _,
                status,
                fields,
                flags: _,
                closed: _,
                limit: _,
            } => longest(status.iter().chain(pairs(fields))),
            ViewCmd::Node {
                targets: _,
                view: _,
                page: _,
            }
            | ViewCmd::Feed {
                at: _,
                since: _,
                types: _,
                limit: _,
            }
            | ViewCmd::History {
                at: _,
                before: _,
                limit: _,
            }
            | ViewCmd::Requests {
                status: _,
                requester: _,
            }
            | ViewCmd::UserLog { since: _, limit: _ }
            | ViewCmd::Agreements { under: _, limit: _ }
            | ViewCmd::Constitution => 0,
        }
    }
}

impl NodeCmd {
    fn longest_text(&self) -> u64 {
        match self {
            NodeCmd::Create(a) => a.longest_text(),
            NodeCmd::Update {
                at: _,
                version: _,
                digest_version: _,
                token: _,
                patch,
            } => patch.longest_text(),
            NodeCmd::Close {
                at: _,
                outcome: _,
                summary,
                verified,
                settles: _,
                learnings,
                token: _,
                unsent: _,
            } => longest([summary].into_iter().chain(verified).chain(learnings)),
            NodeCmd::Reopen { at: _, reason } => reason.byte_len(),
            NodeCmd::Hide { target, reason } => match target {
                HideArg::File { at: _, name } => longest([name, reason]),
                HideArg::Entry { eid: _, node: _ } => reason.byte_len(),
            },
            NodeCmd::Link {
                at: _,
                rel: _,
                to: _,
            }
            | NodeCmd::Key { at: _, change: _ }
            | NodeCmd::Move { at: _, to: _ }
            | NodeCmd::Merge { from: _, into: _ } => 0,
        }
    }
}

impl CreateArgs {
    fn longest_text(&self) -> u64 {
        let CreateArgs {
            path: _,
            keys: _,
            kind: _,
            title,
            body,
            digest,
            fields,
            tags: _,
            capacity: _,
            needs: _,
            est: _,
            priority: _,
            due: _,
            depends_on: _,
            owners: _,
            repeat,
            agreements,
            request,
            take: _,
        } = self;
        let request = request.iter().flat_map(|r| {
            let RequestArg {
                text,
                origin: _,
                where_: _,
                requester,
            } = r;
            [text, requester]
        });
        longest(
            [title]
                .into_iter()
                .chain(body)
                .chain(digest)
                .chain(pairs(fields))
                .chain(repeat.iter().map(RepeatArg::text))
                .chain(agreements)
                .chain(request),
        )
    }
}

impl RepeatArg {
    /// The one text of a schedule: the cron as the agent wrote it.
    fn text(&self) -> &Text {
        let RepeatArg {
            cron_text,
            cron: _,
            tz: _,
            run_within: _,
        } = self;
        cron_text
    }
}

impl Patch {
    fn longest_text(&self) -> u64 {
        let Patch {
            title,
            body,
            digest,
            fields,
            files,
            status,
            agreements,
            kind: _,
            tags_add: _,
            tags_remove: _,
            capacity: _,
            priority: _,
            est: _,
            needs_add: _,
            needs_remove: _,
            due: _,
            depends_on: _,
            owners: _,
            repeat,
        } = self;
        let cron = match repeat {
            Change::Set(r) => Some(r.text()),
            Change::Keep | Change::Clear => None,
        };
        longest(
            title
                .iter()
                .chain(body)
                .chain(digest)
                .chain(fields.iter().flat_map(pairs))
                .chain(files.iter().flat_map(pairs))
                .chain(status)
                .chain(agreements)
                .chain(cron),
        )
    }
}

impl HoldCmd {
    fn longest_text(&self) -> u64 {
        match self {
            HoldCmd::Checkpoint {
                token: _,
                done,
                next_step,
                intent,
                intent_done,
                files,
            } => longest(
                [done, next_step]
                    .into_iter()
                    .chain(intent)
                    .chain(intent_done)
                    .chain(files),
            ),
            HoldCmd::Renew { token: _, why } => why.byte_len(),
            HoldCmd::Release {
                token: _,
                done,
                next_step,
                reason: _,
                waits_for,
                intent_done,
                learnings,
                unsent: _,
                settles: _,
            } => {
                let what = match waits_for {
                    Some(WaitArg::External {
                        what,
                        recheck_after: _,
                    }) => Some(what),
                    Some(WaitArg::Question(_) | WaitArg::Node(_) | WaitArg::Time(_)) | None => None,
                };
                longest(
                    [done, next_step]
                        .into_iter()
                        .chain(what)
                        .chain(intent_done)
                        .chain(learnings),
                )
            }
            HoldCmd::Take {
                target: _,
                lease: _,
            }
            | HoldCmd::Next { query: _, lease: _ } => 0,
            HoldCmd::Check {
                target: _,
                token: _,
                intent_done,
            } => intent_done.as_ref().map_or(0, Text::byte_len),
        }
    }
}

impl TalkCmd {
    fn longest_text(&self) -> u64 {
        match self {
            TalkCmd::FromUser {
                text,
                client,
                principal: _,
                answers: _,
                to: _,
                clarifies: _,
                reason,
            } => longest([text, client].into_iter().chain(reason)),
            TalkCmd::ToUser {
                text,
                kind: _,
                verbatim: _,
                principal: _,
                re: _,
                at: _,
            }
            | TalkCmd::Said {
                at: _,
                where_: _,
                text,
                kind: _,
                corrects: _,
                seen: _,
            }
            | TalkCmd::Answer {
                question: _,
                node: _,
                asker: _,
                text,
            }
            | TalkCmd::Say {
                at: _,
                text,
                to: _,
                expects: _,
                reply_to: _,
                until: _,
            } => text.byte_len(),
            TalkCmd::Route {
                entry: _,
                to: _,
                answers: _,
                clarifies: _,
                reason,
            } => longest(reason),
            TalkCmd::Request {
                at: _,
                text,
                origin: _,
                where_: _,
                requester,
            } => longest([text, requester]),
            TalkCmd::SaidConfirm { said: _, confirmed } => confirmed.byte_len(),
            TalkCmd::Heard {
                at: _,
                where_: _,
                from,
                text,
                answers: _,
            } => longest([from, text]),
            TalkCmd::Ask {
                at: _,
                text,
                to,
                again: _,
            } => match to {
                AskTo::User(UserAsk {
                    to: _,
                    options,
                    recommendation,
                    default,
                    due: _,
                    digest: _,
                }) => longest(
                    [text]
                        .into_iter()
                        .chain(options)
                        .chain(recommendation)
                        .chain(default),
                ),
                AskTo::Person {
                    where_: _,
                    blocking: _,
                }
                | AskTo::Agent(_) => text.byte_len(),
            },
            TalkCmd::Decide {
                at: _,
                text,
                settles: _,
                supersedes: _,
                basis,
                evidence: _,
            } => {
                let whats = basis.iter().filter_map(|b| match b {
                    BasisArg::External {
                        what,
                        checked_at: _,
                        recheck: _,
                    } => Some(what),
                    BasisArg::Node { at: _, version: _ } | BasisArg::Entry(_) => None,
                });
                longest([text].into_iter().chain(whats))
            }
            TalkCmd::Approve {
                on: _,
                version: _,
                checked,
            } => checked.byte_len(),
            TalkCmd::Object { on: _, note }
            | TalkCmd::Resolve {
                objection: _,
                note,
                upheld: _,
                defer: _,
            } => note.byte_len(),
            TalkCmd::Retract {
                target: _,
                text,
                reason,
            } => longest([text, reason]),
            TalkCmd::Wait {
                timeout: _,
                for_: _,
                since: _,
            }
            | TalkCmd::Who { at: _ } => 0,
        }
    }
}

impl ControlCmd {
    fn longest_text(&self) -> u64 {
        match self {
            ControlCmd::Pause { scope: _, reason } => reason.byte_len(),
            ControlCmd::Unpause { scope: _ }
            | ControlCmd::Health
            | ControlCmd::Constitution
            | ControlCmd::Export { after: _, limit: _ } => 0,
        }
    }
}
