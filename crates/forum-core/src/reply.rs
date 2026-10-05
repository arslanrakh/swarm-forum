//! Results and warnings (design D.6.1, D.6.2, D.6.4).
//!
//! §24 gives `decide` the signature `(events, Result<Done, Error>)` with `Done = {reply, warnings,
//! opened, stale_session}` (A-107). Replies carry ids and decided values; the shell renders texts
//! from the post-state with the queries of D.8.25, its own tables, and the history (D.11.6).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::cmd::{BoardState, FindBy};
use crate::state::{DeadlineKey, WaitOn};
use crate::types::{
    CommitmentId, DecisionId, DigestId, Eid, EntryType, Expects, FlagSet, KeyStr, Label, NodeId,
    NoteId, ObjectionId, Outcome, PrincipalId, QuestionId, RequestId, SaidId, Seq, SessionId, Time,
    Token, UserEntryId, Version, ViewMode,
};

/// 1.8 (§11.3, §20.7): an upheld objection's correction of a closed node, `{node, was, outcome}`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Corrected {
    /// The node.
    pub node: NodeId,
    /// Its outcome before.
    pub was: Outcome,
    /// Its outcome now.
    pub outcome: Outcome,
}

/// 1.10 (§11.3, §20.7): the verdict that resolved an objection, `{verdict, upheld}`: the
/// `resolve` entry, and whether it upheld the objection.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Verdict {
    /// The `resolve` entry.
    pub verdict: Eid,
    /// It upheld the objection.
    pub upheld: bool,
}

/// 1.10 (§11.3, §20.7): a second, opposite verdict on a resolved objection, recorded as a new open
/// objection: `{objection, disputes, verdict, upheld}`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Disputed {
    /// The new objection, with the caller's text.
    pub objection: ObjectionId,
    /// The node it is on.
    pub node: NodeId,
    /// The objection it disputes.
    pub disputes: ObjectionId,
    /// The earlier verdict.
    pub earlier: Verdict,
}

/// 1.11 (§11.3, §20.7): a deferral of an open objection, `{deferred: {until?}}`: the caller cannot
/// decide it yet. On a closed node, the review, the caller's class is backed off the node until
/// `until` (§10.3), or until a verdict on it; on an open node nothing backs off, and `until` is
/// None.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Deferred {
    /// The end of the class's backoff from the review, None on an open node.
    pub until: Option<Time>,
}

/// Whether `settle` left the state settled at its `now` (D.6.1, D.8.2): `Done`; `More`, when
/// `settle.batch` firings left due deadlines; or `Stuck(k)` when a key is still queued after its
/// own firing (its transition failed a precondition of `evolve`, a core bug): the shell then aborts
/// the process (crash-only) instead of popping the key for ever.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Settled {
    /// No deadline is due.
    Done,
    /// Due deadlines remain for the next batch.
    More,
    /// This key is still queued after its own firing.
    Stuck(DeadlineKey),
}

/// Only with the test-only feature `conformance` (D.6.1, D.10.8): the tie-break of `next` that
/// `apply_with` uses. `apply` is `apply_with(.., TieBreak::Fnv)`, the default; the server never
/// enables the feature, so production always breaks ties with FNV-1a-64 (§10.4).
#[cfg(feature = "conformance")]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TieBreak {
    /// The ring of D.2.6.
    #[default]
    Fnv,
    /// By id, as the model's slices do with `USE_FNV = false` (D.10.6).
    Id,
}

/// A successful outcome.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Done {
    /// What the command decided.
    pub reply: Reply,
    /// The warnings.
    pub warnings: Vec<Warning>,
    /// This call opened the session: prepend `hello`, bind it.
    pub opened: Option<SessionId>,
    /// The session was silent before this call (§3).
    pub stale_session: bool,
    /// The notes the response to a write delivered, at most `WRITE_NOTES_MAX`, oldest first (1.7,
    /// §13): marked read in the same step.
    #[cfg_attr(feature = "serde", serde(default))]
    pub delivered: Vec<NoteId>,
}

/// What a command decided: ids and decided values (D.6.2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(
    clippy::large_enum_variant,
    reason = "the reply shapes are the frozen contract of design D.6.2"
)]
pub enum Reply {
    // session
    /// `session hello`.
    Hello {
        /// The session.
        session: SessionId,
        /// The `from_user` entry it logged.
        user_entry: Option<UserEntryReply>,
        /// The notes delivered.
        inbox: Vec<NoteId>,
    },
    /// `session status`.
    Status,
    // view
    /// `view node`; `page`: CS-22.
    ViewNodes {
        /// The nodes.
        nodes: Vec<NodeId>,
        /// The view.
        view: ViewMode,
        /// The page of what rests on each node.
        page: u32,
    },
    /// `view find`.
    ViewFind {
        /// By key or by text.
        by: FindBy,
    },
    /// The shell lists the page from its `entries` table (CS-17): with no `types`, the seqs
    /// `after + 1 ..= upto` of the node, oldest first; with `types`, the matching entries after
    /// `after`, at most `limit`.
    ViewFeed {
        /// The node.
        node: NodeId,
        /// After this seq.
        after: Seq,
        /// Up to this seq.
        upto: Seq,
        /// Only these types.
        types: BTreeSet<EntryType>,
        /// Page size.
        limit: u32,
    },
    /// `view list`.
    ViewList {
        /// The nodes.
        nodes: Vec<NodeId>,
        /// How many match.
        total: Total,
    },
    /// The shell lists the node's `event_nodes` rows with `seq < before` (all, without `before`),
    /// newest first, `limit` (default `PAGE_DEFAULT`, at most `PAGE_MAX`), with a total that is
    /// exact up to `SCAN_MAX` rows and `≥k` beyond (§18, A-78).
    ViewHistory {
        /// The node.
        node: NodeId,
        /// Before this event.
        before: Option<u64>,
        /// Page size.
        limit: u32,
    },
    /// `view requests`.
    ViewRequests {
        /// The board state.
        status: Option<BoardState>,
        /// Whose rows.
        scope: BoardScope,
    },
    /// The shell pages `@user` (CS-17).
    ViewUserLog {
        /// After this entry.
        since: Option<Eid>,
        /// Page size.
        limit: u32,
    },
    /// `view constitution`.
    ViewConstitution,
    /// `view agreements` (1.4, §20.4, D.14.5): the nodes of the subtree that have agreements, by
    /// id, at most `limit`, each with the approvals of its current content and its agreements' last
    /// change, and how many there are; the shell renders each path and text from the state.
    Agreements {
        /// The page.
        items: Vec<AgreementItem>,
        /// How many nodes of the subtree have agreements.
        total: Total,
    },
    // node
    /// `node create`, a new node. `request`: the request of the `request` argument, if one was
    /// recorded; `user_requests` (1.4, R3): the requests the routings of its `user:e<n>` keys
    /// recorded (D.8.11), ascending.
    Created {
        /// The node.
        id: NodeId,
        /// The hold, if one was issued.
        token: Option<Token>,
        /// Its lease end.
        lease_until: Option<Time>,
        /// The closed carriers it continues.
        continues: Vec<NodeId>,
        /// The request recorded with it.
        request: Option<RequestId>,
        /// The requests its `user:e<n>` keys' routings recorded.
        user_requests: Vec<RequestId>,
    },
    /// `node create`, an existing node (`{existing}`).
    Joined {
        /// The node.
        existing: NodeId,
        /// Its holders.
        holders: Vec<SessionId>,
        /// The oldest open people question on it.
        open_question: Option<QuestionId>,
        /// The keys added to it.
        added_keys: Vec<KeyStr>,
        /// The request recorded with it.
        request: Option<RequestId>,
        /// The requests the routings of its added `user:e<n>` keys recorded (1.4, R3), ascending.
        user_requests: Vec<RequestId>,
    },
    /// `node update`.
    Updated {
        /// The version after.
        version: Version,
        /// The digest version after.
        digest_version: Version,
    },
    /// `node link`.
    Linked {
        /// The link already existed.
        existing: bool,
        /// A signal it opened.
        signal: Option<NodeId>,
    },
    /// `node key {add}`.
    KeyAdded {
        /// The node already carried it.
        already: bool,
        /// 1.4 (R3): the request the routing of an added `user:e<n>` key recorded.
        user_request: Option<RequestId>,
    },
    /// `node key {add}` of a key another open node carries.
    KeyExisting {
        /// That node.
        existing: NodeId,
        /// The signal it opened.
        signal: Option<NodeId>,
    },
    /// `node key {remove}`.
    KeyRemoved,
    /// `node close`. `dropped` (1.4, §12, §20.5, R2, R9): the open occurrence that closing a series
    /// dropped, beside the `series_stopped` warning.
    Closed {
        /// The node.
        node: NodeId,
        /// The decision its `settles` recorded.
        decision: Option<DecisionId>,
        /// The requests it settled.
        settled: Vec<RequestId>,
        /// Each request the close left unsettled on the node, with why (1.7, §5).
        #[cfg_attr(feature = "serde", serde(default))]
        not_settled: Vec<NotSettled>,
        /// The occurrence a series' close dropped.
        dropped: Option<NodeId>,
        /// What to tidy next (§10.3).
        leave_better: Vec<NodeId>,
    },
    /// `node reopen`.
    Reopened {
        /// The node.
        node: NodeId,
        /// The keys it dropped (K7).
        dropped_keys: Vec<KeyStr>,
        /// 1.12 (§16): the reopen put the `offer` tag on the node, so that `next` offers it.
        #[cfg_attr(feature = "serde", serde(default))]
        offered: bool,
    },
    /// `node move`.
    Moved {
        /// The node.
        node: NodeId,
    },
    /// `node merge`.
    Merged {
        /// The merged node.
        from: NodeId,
        /// The node that remains.
        into: NodeId,
    },
    /// `node hide`.
    Hidden,
    // hold
    /// `hold take`. `reopened` (1.4, §10.1, R4): `{was}`, the outcome of the closed node this take
    /// reopened; None when the node was open, or for the existing hold.
    Taken {
        /// The node.
        node: NodeId,
        /// The hold.
        token: Token,
        /// Its lease end.
        lease_until: Time,
        /// The outcome the node had, when the take reopened it.
        reopened: Option<Outcome>,
        /// The session already held it.
        existing: bool,
    },
    /// `hold next`.
    Next(NextReply),
    /// `hold checkpoint`. 1.4 (§20.6, R9): the checkpoint's own entry, the lease's new end, the
    /// intent it opened (its entry, `entry` itself), and the intent it closed (that intent's entry),
    /// whichever session opened it.
    Checkpointed {
        /// The checkpoint's own entry.
        entry: Eid,
        /// The new lease end.
        lease_until: Time,
        /// The intent it opened.
        intent: Option<Eid>,
        /// The intent it closed.
        intent_done: Option<Eid>,
    },
    /// `hold renew`.
    Renewed {
        /// The new lease end.
        lease_until: Time,
        /// Renews left before a checkpoint.
        renews_left: u32,
    },
    /// `hold release`. `intent_done` (1.4, §10.3, R15): the entry of the intent the release closed.
    /// 1.12 (§10.3): with `settles`, the decision it recorded after the release, the requests it
    /// settled, and each other listed request with why, as `Decided` names them.
    Released {
        /// The intent it closed.
        intent_done: Option<Eid>,
        /// What to tidy next (§10.3).
        leave_better: Vec<NodeId>,
        /// 1.12: the decision its `settles` recorded.
        #[cfg_attr(feature = "serde", serde(default))]
        decision: Option<DecisionId>,
        /// 1.12: the requests it settled.
        #[cfg_attr(feature = "serde", serde(default))]
        settled: Vec<RequestId>,
        /// 1.12: the listed requests it did not settle, and why.
        #[cfg_attr(feature = "serde", serde(default))]
        not_settled: Vec<NotSettled>,
    },
    /// `hold check`.
    HoldCheck {
        /// The node.
        node: NodeId,
        /// Whether the given token is a live hold on it.
        token_current: Option<bool>,
        /// 1.8 (§10.2): the `checkpoint` entry that recorded `intent_done`.
        #[cfg_attr(feature = "serde", serde(default))]
        entry: Option<Eid>,
        /// 1.8: the intent it closed.
        #[cfg_attr(feature = "serde", serde(default))]
        intent_done: Option<Eid>,
    },
    // talk
    /// `talk from_user`. `routed` (1.4, §8.2, R3): how the entry was routed.
    FromUser {
        /// The entry.
        entry: UserEntryId,
        /// Its `user:e<n>` key.
        key: KeyStr,
        /// How it was routed.
        routed: RoutedReply,
        /// Whose words.
        principal: PrincipalId,
    },
    /// `talk to_user`.
    ToUser {
        /// The entry.
        entry: Eid,
        /// To whom.
        principal: PrincipalId,
        /// The requests it settled.
        settled: Vec<RequestId>,
        /// The digest entries it showed.
        shown_digest: Vec<DigestId>,
        /// 1.8 (§8.1): the requests that `at` named in `re`, when the call gave no `re`; 1.9: those
        /// a `result` with neither `re` nor `at` named on the nodes its session holds.
        #[cfg_attr(feature = "serde", serde(default))]
        re: Vec<RequestId>,
    },
    /// `talk route`. `routed` (1.4, R3): how the entry was routed, or, with `existing`, its first
    /// routing.
    Routed {
        /// The entry.
        entry: UserEntryId,
        /// How it is routed.
        routed: RoutedReply,
        /// It was already routed (the first routing wins, §8.2).
        existing: bool,
    },
    /// `talk request`.
    Request {
        /// The request.
        request: RequestId,
        /// Its node.
        node: NodeId,
        /// The message already had a request (Q1).
        existing: bool,
    },
    /// 1.4 (§20.7, R9): `answered`, the unsettled requests this `said` answered (a `reply`,
    /// `result`, or `correction`, of its own conversation, §9.4); `settled`, those of them it
    /// settled, as `Decided`, `ToUser`, and `Closed` carry them: rule 8 of the next calls reads
    /// "this call settled the last unsettled request" (§20.9, D.8.26); `fulfilled`, the commitments
    /// it fulfilled (§9.2). All three ascending, and empty for an `ack`.
    Said {
        /// The `said`.
        said: SaidId,
        /// Its conversation node.
        node: NodeId,
        /// The requests it answered.
        answered: Vec<RequestId>,
        /// The requests it settled.
        settled: Vec<RequestId>,
        /// The commitments it fulfilled.
        fulfilled: Vec<CommitmentId>,
        /// 1.12 (§9.1): the conversation key the call gave `at` (case 3), so that `node` is now
        /// the conversation node, and the conversation's later messages join it.
        #[cfg_attr(feature = "serde", serde(default))]
        keyed: Option<KeyStr>,
    },
    /// `talk said {said_id}`.
    SaidConfirmed {
        /// It was already confirmed.
        already: bool,
    },
    /// `talk heard`.
    Heard {
        /// The entry.
        entry: Eid,
        /// Its conversation node.
        node: NodeId,
        /// The question whose state it changed.
        answered: Option<QuestionId>,
        /// 1.12 (§9.1): the conversation key the call gave `at` (case 3).
        #[cfg_attr(feature = "serde", serde(default))]
        keyed: Option<KeyStr>,
    },
    /// `talk ask`.
    Asked {
        /// The question.
        question: QuestionId,
        /// The `said` recorded with a question to a person.
        said: Option<SaidId>,
        /// Its node.
        node: NodeId,
        /// The same question was already open (`{existing}`).
        existing: bool,
        /// 1.12 (§9.1): the conversation key a question to a person gave `at` (case 3).
        #[cfg_attr(feature = "serde", serde(default))]
        keyed: Option<KeyStr>,
    },
    /// `talk answer`.
    Answered {
        /// The entry.
        entry: Eid,
    },
    /// `talk say`.
    Say {
        /// The entry.
        entry: Eid,
    },
    /// `talk decide`. `not_settled` (1.4, §20.7, R9): each other request `settles` lists, with why
    /// it did not settle, ascending.
    Decided {
        /// The decision.
        decision: DecisionId,
        /// The requests it settled.
        settled: Vec<RequestId>,
        /// The listed requests it did not settle, and why.
        not_settled: Vec<NotSettled>,
    },
    /// Approvals of the current content (CS-34).
    Approved {
        /// The entry.
        entry: Eid,
        /// Approvals of the current content.
        approvals: u32,
    },
    /// `talk object`.
    Objected {
        /// The objection.
        objection: ObjectionId,
    },
    /// None: it was already resolved.
    ObjectionResolved {
        /// The `resolve` entry; 1.10, for a second, opposite verdict, the new objection.
        entry: Option<Eid>,
        /// 1.8 (§11.3): the correction an upheld resolve made of a closed node.
        #[cfg_attr(feature = "serde", serde(default))]
        corrected: Option<Corrected>,
        /// 1.10 (§11.3): with no entry, the verdict that resolved it, the same as the caller's.
        #[cfg_attr(feature = "serde", serde(default))]
        already: Option<Verdict>,
        /// 1.10 (§11.3): the caller's verdict was the opposite one, recorded as a new objection.
        #[cfg_attr(feature = "serde", serde(default))]
        disputed: Option<Disputed>,
        /// 1.10 (§11.3): the correction the earlier verdict made, withdrawn: `was` is the corrected
        /// outcome, `outcome` the one the node was closed with.
        #[cfg_attr(feature = "serde", serde(default))]
        withdrawn: Option<Corrected>,
        /// 1.11 (§11.3): the caller deferred the open objection; `entry` is the deferral.
        #[cfg_attr(feature = "serde", serde(default))]
        deferred: Option<Deferred>,
    },
    /// `talk retract`.
    Retracted {
        /// The entry.
        entry: Eid,
        /// 1.12 (§9.3): the open question to a person it withdrew.
        #[cfg_attr(feature = "serde", serde(default))]
        withdrawn: Option<QuestionId>,
    },
    /// Nothing: `{timeout}`.
    Waited {
        /// The notes delivered.
        delivered: Vec<NoteId>,
        /// News on the node waited for, up to this seq.
        news: Option<(NodeId, Seq)>,
    },
    /// `talk who`.
    Who {
        /// The subtree.
        at: Option<NodeId>,
    },
    // control
    /// `control pause`.
    Paused {
        /// The scope.
        scope: NodeId,
        /// Until when.
        until: Time,
        /// It renewed an active pause.
        renewed: bool,
    },
    /// `control unpause`.
    Unpaused {
        /// The scope.
        scope: NodeId,
    },
    /// `control health`.
    Health,
    /// `control constitution`.
    Constitution,
    /// `control export`.
    Export {
        /// After this event.
        after: Option<u64>,
        /// Page size.
        limit: u32,
    },
    // admin
    /// `AdminCmd::SetParams`.
    ParamsSet,
    // retry (O1): the shell returns the stored response bytes
    /// A repeat answered by the retry store.
    Replay,
}

/// The `from_user` entry a `hello` logged.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserEntryReply {
    /// The entry.
    pub entry: UserEntryId,
    /// Its `user:e<n>` key.
    pub key: KeyStr,
    /// How it was routed (1.4, §8.2, R3).
    pub routed: RoutedReply,
    /// Whose words.
    pub principal: PrincipalId,
}

/// How a user entry was routed (1.4, §8.2, R3): the `routed` of `from_user`, `hello {from_user}`,
/// and `talk route`. The shell renders `NoAction`'s reason from the routing event's text.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RoutedReply {
    /// A new request, recorded on `node`; `follows` (D.14.5): the settled request a clarification
    /// of it was recorded after, which the new request follows.
    Request {
        /// The request.
        request: RequestId,
        /// Its node.
        node: NodeId,
        /// The settled request it follows, for a clarification of one (D.14.5).
        follows: Option<RequestId>,
    },
    /// An answer to a question for the user.
    Answers {
        /// The question.
        question: QuestionId,
    },
    /// A clarification of a request.
    Clarifies {
        /// The request.
        request: RequestId,
    },
    /// `{none: reason}`.
    NoAction,
    /// It stayed unrouted.
    Unrouted(UnroutedWhy),
}

/// Why an entry stayed unrouted (1.4, R3): the question it answers is no longer pending (§8.4,
/// A-37), or the call named no target and the session holds no node (`holds: 0`) or more than one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UnroutedWhy {
    /// The question it answers is no longer pending.
    NotPending {
        /// The question.
        question: QuestionId,
    },
    /// No target, and the session holds `holds` nodes, 0 or more than one.
    NoTarget {
        /// The session's live holds.
        holds: u32,
    },
}

/// A request a decision lists that it did not settle (1.4, §20.7, R9).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NotSettled {
    /// The request.
    pub request: RequestId,
    /// Why it did not settle.
    pub why: NotSettledWhy,
}

/// Why a listed request did not settle (1.4, R9): it was settled before; or the answer it still
/// awaits (§9.4): a `to_user` of kind `result` to its requester, or a `said` of kind reply, result,
/// or correction in its conversation, by key.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NotSettledWhy {
    /// Settled before.
    SettledBefore,
    /// Awaits a `to_user` of kind `result` to its requester.
    AwaitsResult {
        /// The requester.
        requester: PrincipalId,
    },
    /// Awaits a `said` of kind reply, result, or correction in its conversation.
    AwaitsSaid {
        /// The conversation node.
        node: NodeId,
        /// The conversation key.
        conv: KeyStr,
    },
    /// Answered, and awaits a decision that lists it (1.7, §5): a close settles by default only
    /// what a result answered.
    AwaitsDecision,
}

/// One node of `view agreements` (1.4, §20.4, D.14.5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AgreementItem {
    /// The node that has agreements.
    pub node: NodeId,
    /// The approvals of its current content (CS-34).
    pub approvals: u32,
    /// When its agreements were last set (`Node::agreements_at`).
    pub changed: Option<Time>,
}

/// Whose requests `view requests` lists (CS-25): every row, or the user requests and unrouted
/// entries of these principals.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BoardScope {
    /// Every row.
    All,
    /// These principals' rows.
    Principals(BTreeSet<PrincipalId>),
}

/// §18: lists carry shown and total, or `≥k`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Total {
    /// Exactly this many.
    Exact(u32),
    /// At least this many.
    AtLeast(u32),
}

/// What `hold next` found.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NextReply {
    /// A node, now held.
    Hold {
        /// The node.
        node: NodeId,
        /// The hold.
        token: Token,
        /// Its lease end.
        lease_until: Time,
        /// Why this one.
        why: WhyThis,
        /// 1.4: `{was}`, the outcome the node had, when taking reopened it (as `Taken`'s).
        reopened: Option<Outcome>,
    },
    /// A routing item.
    Route {
        /// The entry.
        entry: UserEntryId,
        /// Why this one.
        why: WhyThis,
    },
    /// A status item.
    Status {
        /// The node.
        node: NodeId,
        /// Why this one.
        why: WhyThis,
    },
    /// 1.9 (§10.4, §11.3): a review item, a closed node with an open objection, offered with no
    /// hold and not reopened: `talk resolve {objection, upheld}` decides whether it stands.
    Review {
        /// The node.
        node: NodeId,
        /// Its first open objection.
        objection: ObjectionId,
        /// Why this one.
        why: WhyThis,
    },
    /// Nothing for this caller.
    Empty {
        /// Why.
        why: EmptyWhy,
        /// What to tidy instead (§10.3).
        leave_better: Vec<NodeId>,
    },
}

/// `why_this = {tier, flags, aged, reason, est?}` (§10.4); `aged` in hour buckets (CS-12, D-1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WhyThis {
    /// 0, 1, or 2.
    pub tier: u8,
    /// The candidate's flags.
    pub flags: FlagSet,
    /// `priority + ⌊now / 1 h⌋ − ⌊since / 1 h⌋`.
    pub aged: i64,
    /// Why it is a candidate.
    pub reason: Reason,
    /// 1.10 (§10.4): a node to take without `est`, offered to a caller with a horizon, which ranks
    /// it after the estimated candidates of its tier: `est: "no estimate"`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub no_estimate: bool,
    /// 1.12 (§10.4): a node whose `est` exceeds the horizon of every session seen within
    /// `capacity.window` whose caps meet its non-empty needs, the caller's among them, offered
    /// after every fitting candidate: "longer than any session's horizon: do a part, checkpoint,
    /// release handoff". In the lab's seventh round, every Lean lemma was longer than every
    /// session's horizon, and none was ever offered.
    #[cfg_attr(feature = "serde", serde(default))]
    pub longer: bool,
    /// 1.12 (§10.4): on the first candidate of a read, the nodes whose non-empty needs the
    /// caller's caps meet and whose `est` exceeds its horizon: how many, and the first.
    #[cfg_attr(feature = "serde", serde(default))]
    pub beyond_horizon: Option<BeyondHorizon>,
}

/// 1.12 (§10.4): the work whose needs a caller's caps meet that its horizon excludes: `{count,
/// first}`, the first as `/claims/lemma-01 est 60 > your horizon 45`. `count` reads whole `est`
/// buckets, and at most `SCAN_MAX` entries of the bucket the horizon falls in (§18), `≥k` when that
/// scan stopped there (`capped`); it counts status and review items too, and `first` is a node to
/// take.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BeyondHorizon {
    /// How many.
    pub count: u32,
    /// 1.12's review (§18): the scan of the bucket the horizon falls in stopped at `SCAN_MAX`
    /// entries, so `count` is a lower bound, shown `≥k`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub capped: bool,
    /// The first node to take, in the index's order: never a status or review item.
    pub first: NodeId,
    /// Its `est`.
    pub est: u32,
    /// The caller's horizon.
    pub horizon: u32,
}

/// Why a candidate is one: the first that applies, in this order of D.8.5.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Reason {
    /// It carries flags.
    Flagged,
    /// It is offered.
    Offered,
    /// An available occurrence.
    Occurrence,
    /// A signal node.
    Signal,
    /// A routing item.
    Routing,
    /// A status item.
    Status,
    /// A flagged closed node.
    ClosedFlagged,
    /// 1.9 (§10.4): a review item, a closed node with an open objection.
    Review,
}

/// `{empty, why}`: how many index entries each exclusion removed (§10.4), so "nothing to do" is
/// distinguishable from "nothing for you". `candidates` is the number of entries in the index; 0 is
/// a true empty (CS-31). `depends` and `paused` are the held-back counts, `needs` and `horizon` the
/// sizes of the partitions the caller cannot take, and the rest what the scan skipped (CS-16).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EmptyWhy {
    /// Entries in the index.
    pub candidates: u32,
    /// Held back by an open dependency.
    pub depends: u32,
    /// Refused by the caller's class.
    pub refused: u32,
    /// Needs the caller does not have.
    pub needs: u32,
    /// Longer than the caller's horizon.
    pub horizon: u32,
    /// Held back by a pause.
    pub paused: u32,
    /// Held by the caller already; 1.12 (§10.4): or put down by it, the review of an objection it
    /// raised, or the node it released as `handoff` while that keeps it from the caller.
    pub own: u32,
    /// Rejected by the query.
    pub query: u32,
    /// 1.4 (§10.4, R27): the nodes that would be candidates but for an unresolved wait,
    /// `|ix.next.waiting|`, whatever the caller and the query: they come back when their waits
    /// resolve, so an empty `next` never says that no work is left while some waits.
    pub waiting: u32,
    /// 1.11 (§10.4, §12): the series held back while their open occurrences wait for their slots,
    /// whatever the caller and the query.
    #[cfg_attr(feature = "serde", serde(default))]
    pub before_slot: u32,
    /// The query filter rejected `SCAN_MAX` entries and the scan stopped (CS-16): `query` is then a
    /// lower bound, rendered `≥k`, and a candidate further on may exist (§18).
    pub query_capped: bool,
    /// The scopes of the pauses that excluded candidates, by the number they excluded (CS-9):
    /// each of them, since §10.4 has an empty `next` name each pause with its source, end, and
    /// reason (D.6.2's cap of 3, which carries no total, is not the specification's).
    pub pause_scopes: Vec<NodeId>,
    /// 1.6 (§10.4): the first node each exclusion removed, with the value that excluded it, filled
    /// when nothing was found. In the lab, agents found what their own horizon excluded by trial.
    pub first: FirstExcluded,
}

/// The first node of each exclusion of an empty `next` (1.6, §10.4), with the value that excluded
/// it. "First" is the index's order, `(tier, group, fnv_id(id))`: §10.4's order with every
/// session's ring point at 0, the same for every caller, for the exclusions the caller's caps,
/// horizon, and class make; the smallest id for those that hold a node back from every caller (a
/// dependency, a wait).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FirstExcluded {
    /// A node whose needs the caller's caps do not cover, and the needs they lack.
    pub needs: Option<(NodeId, BTreeSet<Label>)>,
    /// A node whose `est` exceeds the caller's horizon: the node, its `est`, and the horizon.
    pub horizon: Option<(NodeId, u32, u32)>,
    /// A node the caller's class refused, or is backed off from (§10.3).
    pub refused: Option<NodeId>,
    /// A node held back by an open `depends_on` target, and that target.
    pub depends: Option<(NodeId, NodeId)>,
    /// A node that waits (its wait is on the node, §10.3).
    pub waiting: Option<NodeId>,
}

// ------------------------------------------------------------------------------------------------
// Warnings (D.6.4)

/// A warning: the call succeeded, and something deserves attention.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Warning {
    /// The code.
    pub code: WarningCode,
    /// The node it concerns.
    pub id: Option<NodeId>,
    /// The details.
    pub detail: WarningDetail,
}

/// The warning codes of §21.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum WarningCode {
    /// `moved`.
    Moved,
    /// `unsettled_requests`.
    UnsettledRequests,
    /// `open_commitments`.
    OpenCommitments,
    /// `open_objections`.
    OpenObjections,
    /// `agreement`.
    Agreement,
    /// `conversation_held`.
    ConversationHeld,
    /// `held_by_other`.
    HeldByOther,
    /// `no_hold`.
    NoHold,
    /// `uncovered_slot`.
    UncoveredSlot,
    /// `duplicate_key`.
    DuplicateKey,
    /// `secret_masked`.
    SecretMasked,
    /// `user_saturated`.
    UserSaturated,
    /// `session_opened`.
    SessionOpened,
    /// `open_intent` (1.4).
    OpenIntent,
    /// `expects_ignored` (1.4).
    ExpectsIgnored,
    /// `series_stopped` (1.4).
    SeriesStopped,
    /// `self_basis_dropped` (1.4).
    SelfBasisDropped,
    /// `superseded_basis_dropped` (1.4).
    SupersededBasisDropped,
    /// `result_answered_nothing` (1.4).
    ResultAnsweredNothing,
    /// `waiting` (1.6).
    Waiting,
    /// `unnamed_user` (1.6).
    UnnamedUser,
    /// `ignored_arguments`, raised by the core for `to_user {at}` (1.8); the shell raises it for
    /// the arguments the decoder dropped (1.7).
    IgnoredArguments,
    /// `no_human_in_reach` (1.8).
    NoHumanInReach,
    /// `client_context` (1.9).
    ClientContext,
    /// `unmet_needs` (1.9).
    UnmetNeeds,
    /// `no_basis` (1.10).
    NoBasis,
    /// `paused` (1.10): a warning, beside the error of the same name.
    Paused,
    /// `follow_up` (1.11): a `from_user` with no target routed by default as a clarification.
    FollowUp,
    /// `no_recipient` (1.12): a note at a closed node reached nobody.
    NoRecipient,
    /// `addressee_gone` (1.12): a note or an agent question to a silent session.
    AddresseeGone,
    /// `answered_nothing` (1.12): a `said` of kind `reply` or `result` answered nothing where no
    /// request of its conversation is.
    AnsweredNothing,
}

/// The details of a warning.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WarningDetail {
    /// §9.2: at most `MOVED_MAX` entries, newest first, and how many there are (CS-17).
    Moved {
        /// The entries.
        entries: Vec<Eid>,
        /// How many.
        total: Total,
    },
    /// Requests left unsettled.
    UnsettledRequests {
        /// The requests.
        requests: Vec<RequestId>,
    },
    /// Commitments left open.
    OpenCommitments {
        /// The commitments.
        commitments: Vec<CommitmentId>,
    },
    /// Objections left open.
    OpenObjections {
        /// The objections.
        objections: Vec<ObjectionId>,
    },
    /// `approvals`: the approve entries at versions from `content_version` on, the approvals of the
    /// current content (A-76, CS-34).
    Agreement {
        /// The nodes whose agreements apply.
        sources: Vec<NodeId>,
        /// Approvals of the current content.
        approvals: u32,
        /// The node's content version.
        content_version: Version,
    },
    /// Another session holds the conversation. 1.10 (§9.2): the `said` counts all the same; 1.9
    /// recorded a `talk said` withheld.
    ConversationHeld {
        /// Its holders.
        holders: Vec<SessionId>,
    },
    /// Other sessions held the node.
    HeldByOther {
        /// Those sessions.
        holders: Vec<SessionId>,
        /// 1.8 (§5): for a close without `settles` by a session that did not hold the node, the
        /// requests the default would have settled, left unsettled to the holder; empty otherwise.
        #[cfg_attr(feature = "serde", serde(default))]
        left: Vec<RequestId>,
    },
    /// No hold was issued.
    NoHold {
        /// Why.
        why: NoHoldWhy,
    },
    /// Local hours, A-47. `first_start` (1.4, §12, R18): the first instant inside the next slot's
    /// window `[slot, slot + run_within)` that lies in one of the usual hours, the slot itself when
    /// its own local hour is one (D.8.18); None when no usual hour falls inside the window, or none
    /// is known.
    UncoveredSlot {
        /// Days with a covered slot.
        covered_days: u32,
        /// Days with a slot.
        days: u32,
        /// The usual local hours of session starts.
        usual_hours: Vec<u8>,
        /// The first usual start inside the next slot's window.
        first_start: Option<Time>,
    },
    /// Keys another open node carries.
    DuplicateKey {
        /// Each key and its carrier.
        keys: Vec<(KeyStr, NodeId)>,
    },
    /// Added by the shell only.
    SecretMasked {
        /// How many fields were masked.
        fields: u32,
    },
    /// `talk ask {to: user | principal:<p>}` found the first addressee already with more than
    /// `user_question.max_pending` pending questions, counted before this question is recorded
    /// (§8.4): decide on a recorded assumption instead where you can (CS-33). The question is
    /// recorded either way. `pending`: that count, over the person's aliases.
    UserSaturated {
        /// The addressee.
        principal: PrincipalId,
        /// The pending count.
        pending: u32,
    },
    /// A command other than `session hello` opened a new session (CS-29): "pass `session: s-N` on
    /// every call". The call succeeded.
    SessionOpened {
        /// The new session.
        session: SessionId,
    },
    /// 1.4 (§10.3, R15): a `hold release` left the session's own intent `intent` open on the node:
    /// if the action happened, `intent_done` says so; otherwise the next holder must check it.
    OpenIntent {
        /// The intent's entry.
        intent: Eid,
    },
    /// 1.4 (§13, R17): `say {expects}` with a value other than `none`; `expects` is a later release
    /// (§29), nothing waits for the answer, and `talk ask {to}` asks agents for one.
    ExpectsIgnored {
        /// The value given.
        expects: Expects,
    },
    /// 1.4 (§12, R2): a `close` of a series, or a `merge` from one, dropped the open occurrence
    /// `occurrence`, and no further occurrence opens until the series is reopened. A warning only:
    /// the close or merge is applied.
    SeriesStopped {
        /// The occurrence dropped.
        occurrence: NodeId,
    },
    /// 1.4 (§15, R14): the decision's basis named its own node (directly or through
    /// `merged_into`); those references were dropped, and the decision was recorded with the
    /// others.
    SelfBasisDropped {
        /// How many references were dropped.
        dropped: u32,
    },
    /// 1.4 (§15, re-run R33): the decision's basis named the decision it supersedes; those
    /// references were dropped.
    SupersededBasisDropped {
        /// The superseded decision.
        decision: DecisionId,
        /// How many references were dropped.
        dropped: u32,
    },
    /// 1.4 (§21, re-run R34): a `to_user` of kind `result` answered no request: it named none in
    /// `re` (`requests` empty), or each request it named has another principal as its requester,
    /// or a requester outside the swarm, who is answered by a `said` in their conversation; never
    /// when `re` names a `from_user` entry of the addressee, to which it replies (1.5).
    ResultAnsweredNothing {
        /// The requests `re` named, none of which it answered.
        requests: Vec<RequestId>,
    },
    /// 1.6 (§10.1, §21): a take held a node that waits (§10.3), and the wait stays until the
    /// taker's first checkpoint (1.10): what it waits on, and when it was set (the release that
    /// set it is the brief's last checkpoint, with its author and what it waits for). In the lab,
    /// a take of a node that waited on another session's action in flight said nothing, and the
    /// taker redid that work.
    Waiting {
        /// What the wait waits on.
        on: WaitOn,
        /// When it was set.
        set_at: Time,
        /// 1.10: the release that set it, the node's last checkpoint (§10.2: a checkpoint ends a
        /// wait, so none came after it). The shell tells its author, if that is the taker, that
        /// its next checkpoint ends the wait: in the lab's fifth round (l1), the warning told a
        /// worker that took its own waiting node back to ask the release's author, itself.
        #[cfg_attr(feature = "serde", serde(default))]
        release: Option<Eid>,
    },
    /// 1.6 (§8.1, §21): a `from_user` filed for `user`, the person the client did not name, in a
    /// session that did not serve `user` before the call. In 1.6 to 1.8 that was a `from_user`
    /// with no principal in a session that served none, and in the lab workers logged a
    /// scheduler's prompt and a tool's output this way, which became requests from a human who
    /// never wrote; since 1.9 such a call is `invalid(NoPrincipal)`, and the warning comes with
    /// `principal: user`, which a client that serves a person it does not name passes.
    UnnamedUser {
        /// The entry.
        entry: UserEntryId,
    },
    /// 1.8 (§8.1, §21): `to_user {at}` named a node with no unsettled user request whose requester
    /// is the report's addressee's person, so `at` was ignored and the report filed as without it.
    /// In the lab's third round, a worker's report was filed for `user`, whom nobody served.
    AtIgnored {
        /// The node `at` named.
        node: NodeId,
    },
    /// 1.8 (§8.1, §8.4, §21): a `to_user`, or an `ask {to: user}` whose chain is `user` alone, went
    /// to `user` from a session that serves no principal, while no live session serves `user` and
    /// `user` is no alias of another principal: nobody will read it. For a question, its due, and
    /// whether it then takes its default rather than lapsing.
    NoHumanInReach {
        /// The question's due, for an `ask`.
        due: Option<Time>,
        /// The question has a default.
        default: bool,
    },
    /// 1.9 (§8.1, §21): a `from_user` whose text looks like what a client adds to the human's
    /// turn: it starts with `# AGENTS.md instructions`, or carries `<environment_context>` or
    /// `<INSTRUCTIONS>`. The entry is recorded as given. In the lab's fourth round, a Codex agent
    /// logged its client's instruction file and environment block as its human's words.
    ClientContext {
        /// The entry.
        entry: UserEntryId,
    },
    /// 1.11 (§8.2, §21): a `from_user` with none of `to`, `answers`, and `clarifies`, from a
    /// session whose only live hold is on `node`, routed by default as a clarification of
    /// `request`, the newest unsettled request there of the entry's person. In the lab's sixth
    /// round, a human's confirmation of the plan became a second request beside the first.
    FollowUp {
        /// The entry.
        entry: UserEntryId,
        /// The request it clarifies.
        request: RequestId,
        /// Its node.
        node: NodeId,
        /// Its requester.
        requester: PrincipalId,
    },
    /// 1.9 (§10.1, §20.5, §21): needs that nobody can meet. With `take` false, labels of a
    /// `create`'s `needs` or an `update`'s `needs_add` that no session seen within
    /// `capacity.window` has, nor the caller: no session can take the node now. With `take` true,
    /// the needs of the node a `take` held that the taker's caps lack. In the lab, labels that said
    /// what the work lacked left claims offered to nobody, and a session took work it could not do.
    UnmetNeeds {
        /// The labels, ascending.
        needs: Vec<Label>,
        /// A take's: the taker's caps lack them.
        take: bool,
    },
    /// 1.10 (§11.3, §21): a resolve kept the outcome of a claim closed `done` that has no active
    /// decision and a retracted one, the newest of which is `decision`. In the lab's fifth round
    /// (l5), three reviewed lemmas whose deciding decisions were retracted read as done.
    NoBasis {
        /// The claim.
        node: NodeId,
        /// Its newest retracted decision.
        decision: DecisionId,
    },
    /// 1.10 (§10.2, §19, §21): a checkpoint's intent was recorded on a node under an active
    /// pause, set at `scope` and ending at `until`; the shell adds its source and reason.
    Paused {
        /// The paused node: the intent's node or an ancestor.
        scope: NodeId,
        /// When the pause ends.
        until: Time,
    },
    /// 1.12 (§8.4, §21), code `no_human_in_reach`: no stage of question `question`'s chain is a
    /// person some session serves (`chain`, its stages), so nobody will put it to them. In the
    /// lab's seventh round (l1), a question for a commit's author, whom no session served, was
    /// filed with no warning.
    NobodyServes {
        /// The question.
        question: QuestionId,
        /// Its chain of addressees.
        chain: Vec<PrincipalId>,
    },
    /// 1.12 (§13, §21): a note by `holder:`, or without `to`, or an agent question by `holder:`,
    /// at closed node `node` reached nobody; the shell renders the closure (its outcome, closer,
    /// time, and entry) and, while the closer is silent and not retired, how to reach it. In the
    /// lab's seventh round (l6), two notes to the holder of an incident closed seconds before
    /// reached nobody.
    NoRecipient {
        /// The closed node.
        node: NodeId,
        /// 1.12's review: an agent question, "this question reaches nobody", not a note.
        #[cfg_attr(feature = "serde", serde(default))]
        question: bool,
    },
    /// 1.12 (§13, §21): a note or an agent question to `session`, silent since its last call at
    /// `since`: it will not read it unless it calls again. In the lab's seventh round (l6), an
    /// incident lead asked a session silent for 36 minutes why it had paused a subtree.
    AddresseeGone {
        /// The addressee.
        session: SessionId,
        /// Its last call (`Session::last_seen`).
        since: Time,
    },
    /// 1.12 (§9.2, §21): a `said` of kind `reply` or `result` answered nothing, on conversation
    /// node `node`, which carries no request of its conversation `conv`. In the lab's seventh round
    /// (l3), a chat worker answered two questions in a thread with no request recorded for either.
    AnsweredNothing {
        /// The conversation node.
        node: NodeId,
        /// The conversation.
        conv: KeyStr,
    },
}

/// Why `create` issued no hold. `NoCapacity`: A-65.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum NoHoldWhy {
    /// The session holds `hold.per_session` already.
    HoldsLimit,
    /// The node is paused.
    Paused,
    /// The node has capacity 0.
    NoCapacity,
}
