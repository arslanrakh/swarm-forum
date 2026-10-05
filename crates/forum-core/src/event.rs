//! Events (design D.7): the envelope, the event enum, entry bodies and texts.
//!
//! `at` is the command's clamped `now`, or the deadline's own time for every event a firing emits.
//! `evolve` uses `at` for every time it stores and for flag recomputation; it never reads a clock
//! and never allocates an id: every id an event introduces is inside the event, and `evolve` checks
//! that it is the counter's next value (D.7.5).
//!
//! Encoding (D.7.6): canonical JSON, struct fields in declaration order, enums externally tagged.
//! For later releases, enum variants are only appended and struct fields only appended with
//! `#[serde(default)]`.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::cmd::Cmd;
use crate::error::Error;
use crate::params::Params;
use crate::reply::Done;
use crate::state::{
    Author, BasisRef, BasisTarget, DeadlineKind, DigestItem, Intent, LeaseTerms, Owner, Place,
    Repeat, Restored, Role, Routing, SignalKey, UqSource, WaitOn,
};
use crate::types::{
    Addressee, Change, ClassName, DecisionId, Dur, Eid, Evidence, Expects, Key, KeyStr, Label,
    LinkRel, Mark, Name, NodeId, NormVer, NoteId, ObjectionId, OpId, Outcome, PrincipalId,
    PrincipalKey, QuestionId, ReleaseReason, ReqDigest, RequestId, Requester, SaidId, SaidKind,
    Seq, SessionId, Text, Time, ToUserKind, Token, UserEntryId, Version,
};

// ------------------------------------------------------------------------------------------------
// Envelope (D.7.1)

/// One event: a time and what happened.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Event {
    /// The command's clamped `now`, or the deadline's own time.
    pub at: Time,
    /// What happened.
    pub ev: Ev,
}

/// The first event of every log: the eight reserved nodes and the parameters (D.6.1).
#[must_use]
pub fn genesis(params: Params, at: Time) -> Event {
    Event {
        at,
        ev: Ev::Genesis { params },
    }
}

// ------------------------------------------------------------------------------------------------
// The event enum (D.7.2)

/// Every event. Each is one §25 transition of one entity, one creation, or one field change of one
/// node, plus the bookkeeping owned by that one node (D.0.2, principle 2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(
    clippy::large_enum_variant,
    reason = "the event shapes are the frozen contract of design D.7.2"
)]
pub enum Ev {
    // ---- time and configuration
    /// The first event: the parameters and the reserved nodes.
    Genesis {
        /// The parameters.
        params: Params,
    },
    /// The parameters changed (D.11.9).
    ParamsChanged {
        /// The new parameters.
        params: Params,
    },
    /// A deadline fired at `at` (T4). For six kinds it is also the transition: Wait, Commitment,
    /// OccurrenceWindow, Due, and Silence set their marker, and RefusalExpiry of a Class removes the
    /// class's caps profiles whose window ended (CS-7).
    Fired {
        /// The kind.
        kind: DeadlineKind,
        /// The owner.
        owner: Owner,
    },

    // ---- sessions (§3, §13)
    /// `caps`: the token's default caps with hello's added (CS-26, 1.6), else none. Opening a
    /// session does not make its class active for E1; an open `next` does (CS-7). `retire_at = at
    /// + session.retain` (CS-8).
    SessionOpened {
        /// The new session.
        session: SessionId,
        /// Its class.
        class: ClassName,
        /// Its caps.
        caps: BTreeSet<Label>,
        /// Minutes.
        horizon: Option<u32>,
        /// Hello's intent.
        intent: Option<Text>,
        /// `at + presence.silence`.
        silence_at: Time,
        /// `at + session.retain`.
        retire_at: Time,
        /// Hello's principal (CS-1).
        principal: Option<PrincipalId>,
    },
    /// `hello` on an existing session sets what is given; it cannot clear a horizon, and a
    /// `principal` is added to the session's principals, never replacing them (CS-1).
    SessionHello {
        /// The session.
        session: SessionId,
        /// New caps.
        caps: Option<BTreeSet<Label>>,
        /// New horizon.
        horizon: Option<u32>,
        /// New intent.
        intent: Option<Text>,
        /// A principal to add.
        principal: Option<PrincipalId>,
    },
    /// A principal key used for the first time (CS-1). `id = next_principal`.
    PrincipalSeen {
        /// The new id.
        id: PrincipalId,
        /// The key.
        key: PrincipalKey,
    },
    /// At most once per `presence.resolution`, unless the session was silent or the command issues
    /// a hold (CS-14, CS-8). `retire_at = max(old, at + session.retain)`.
    Touched {
        /// The session.
        session: SessionId,
        /// `at + presence.silence`.
        silence_at: Time,
        /// The new retire time.
        retire_at: Time,
    },
    /// The session leaves the state at its Retire deadline, holding nothing (CS-8).
    SessionRetired {
        /// The session.
        session: SessionId,
    },
    /// `session status`.
    SessionStatus {
        /// The session.
        session: SessionId,
        /// The status text.
        text: Text,
    },
    /// Notes delivered to a session.
    InboxRead {
        /// The session.
        session: SessionId,
        /// The notes.
        notes: BTreeSet<NoteId>,
    },
    /// Only when `seq` raises the session's `shown[node]` (CS-14).
    Shown {
        /// The session.
        session: SessionId,
        /// The node.
        node: NodeId,
        /// The seq shown.
        seq: Seq,
    },
    /// `open`: the call had no query, so the session asked for any work it can do; it records the
    /// session's caps as a profile of its class until `class_window_end` (CS-7).
    NextCalled {
        /// The session.
        session: SessionId,
        /// What `next` found.
        outcome: NextOutcome,
        /// The call had no query.
        open: bool,
        /// `at + refusal.window`.
        class_window_end: Time,
    },

    // ---- nodes (§4, §5, §12, §16)
    /// A node is created.
    NodeCreated {
        /// The new id.
        id: NodeId,
        /// Its parent.
        parent: NodeId,
        /// Its name.
        name: Name,
        /// Who made it.
        origin: crate::state::Origin,
        /// What it starts with.
        init: NodeInit,
    },
    /// Fields of a node change.
    NodeUpdated {
        /// The node.
        node: NodeId,
        /// The changed fields.
        patch: NodePatch,
    },
    /// A key is added.
    KeyAdded {
        /// The node.
        node: NodeId,
        /// The key.
        key: Key,
    },
    /// A key is removed.
    KeyRemoved {
        /// The node.
        node: NodeId,
        /// The key.
        key: KeyStr,
    },
    /// A link is added.
    LinkAdded {
        /// The source.
        node: NodeId,
        /// The relation.
        rel: LinkRel,
        /// The target.
        to: NodeId,
    },
    /// A node moves.
    NodeMoved {
        /// The node.
        node: NodeId,
        /// The new parent.
        parent: NodeId,
        /// The new name.
        name: Name,
    },
    /// A node closes.
    NodeClosed {
        /// The node.
        node: NodeId,
        /// The outcome.
        outcome: Outcome,
        /// The node it was merged into.
        merged_into: Option<NodeId>,
        /// 1.11 (§13): the session that closed it (`Closure::by`), None for the server. Appended
        /// with a default (D.7.6): a log written before 1.11 replays to the state it wrote, with
        /// none.
        #[cfg_attr(feature = "serde", serde(default))]
        by: Option<SessionId>,
    },
    /// A node reopens.
    NodeReopened {
        /// The node.
        node: NodeId,
    },
    /// The `offer` tag is set or removed (never `version`, A-2).
    OfferSet {
        /// The node.
        node: NodeId,
        /// On or off.
        on: bool,
    },
    /// A node's wait is set or cleared.
    WaitSet {
        /// The node.
        node: NodeId,
        /// The wait, or None.
        wait: Option<WaitOn>,
    },
    /// A dependency is redirected by a merge.
    DependsRedirected {
        /// The dependent node.
        node: NodeId,
        /// The old target.
        from: NodeId,
        /// The new target, or None for a self-edge dropped.
        to: Option<NodeId>,
    },
    /// A mark is set or cleared.
    MarkSet {
        /// The node.
        node: NodeId,
        /// The mark.
        mark: Mark,
        /// The question.
        question: QuestionId,
        /// On or off.
        on: bool,
    },
    /// An intent opens.
    IntentOpened {
        /// The node.
        node: NodeId,
        /// The intent.
        intent: Intent,
    },
    /// An intent closes.
    IntentClosed {
        /// The node.
        node: NodeId,
        /// How.
        end: IntentEnd,
    },
    /// Merge (A-94).
    IntentMoved {
        /// The merged node.
        from: NodeId,
        /// The node that remains.
        to: NodeId,
    },
    /// A series records whether its last occurrence was missed.
    SeriesOutcome {
        /// The series.
        series: NodeId,
        /// Missed or not.
        missed: bool,
    },
    /// An occurrence reaches its slot.
    OccurrenceAvailable {
        /// The occurrence.
        node: NodeId,
    },
    /// An occurrence becomes an ordinary node (A-13, A-75).
    OccurrenceDetached {
        /// The node.
        node: NodeId,
    },
    /// A signal node becomes an ordinary node (A-89).
    SignalDetached {
        /// The node.
        node: NodeId,
    },

    // ---- holds (§10)
    /// A hold is issued.
    HoldIssued {
        /// The token.
        token: Token,
        /// The taker.
        session: SessionId,
        /// The first lease end.
        lease_until: Time,
        /// The lease terms in force.
        terms: LeaseTerms,
        /// L of §10.1.
        l: Dur,
        /// 1.4 (§10.1, R4): the outcome of the closed node this take or `next` reopened, None when
        /// the node was open; the hold keeps it (`Hold::reopened`).
        reopened: Option<Outcome>,
    },
    /// A hold is extended.
    HoldExtended {
        /// The token.
        token: Token,
        /// The new lease end.
        lease_until: Time,
        /// By a checkpoint or a renew.
        by: Extension,
    },
    /// A hold ends.
    HoldEnded {
        /// The token.
        token: Token,
        /// How.
        end: HoldEnd,
    },

    // ---- feed (§6): one entry, and the entity its body creates
    /// One feed entry.
    Appended {
        /// The new entry id.
        eid: Eid,
        /// The node.
        node: NodeId,
        /// Its seq there.
        seq: Seq,
        /// Who wrote it.
        by: Author,
        /// Its structural part.
        body: EntryBody,
        /// Its text.
        text: EntryText,
    },

    // ---- entity transitions (§25)
    /// A request is answered.
    RequestAnswered {
        /// The request.
        request: RequestId,
        /// The answer.
        by: Eid,
    },
    /// A request settles.
    RequestSettled {
        /// The request.
        request: RequestId,
        /// The decision.
        decision: DecisionId,
        /// The answer.
        answer: Eid,
    },
    /// A request moves with a merge.
    RequestMoved {
        /// The request.
        request: RequestId,
        /// The new node.
        to: NodeId,
    },
    /// A people question's recheck is rescheduled.
    PqRecheckAt {
        /// The question.
        q: QuestionId,
        /// The new recheck time.
        at: Time,
    },
    /// A people question is rechecked: open(k) → open(k+1).
    PqRechecked {
        /// The question.
        q: QuestionId,
        /// The new count.
        rechecks: u32,
        /// The next recheck.
        next: Time,
    },
    /// A people question expires.
    PqExpired {
        /// The question.
        q: QuestionId,
    },
    /// A people question escalates.
    PqEscalated {
        /// The question.
        q: QuestionId,
        /// The question for the user.
        uq: QuestionId,
    },
    /// A people question is answered.
    PqAnswered {
        /// The question.
        q: QuestionId,
        /// The answer.
        by: Eid,
    },
    /// A people question is superseded.
    PqSuperseded {
        /// The question.
        q: QuestionId,
        /// The new question.
        by: QuestionId,
    },
    /// 1.12 (§9.3): an open people question is withdrawn by a `talk retract` of it.
    PqWithdrawn {
        /// The question.
        q: QuestionId,
        /// The `retract` entry.
        by: Eid,
    },
    /// An expired people question's assumption is recorded.
    PqAssumed {
        /// The question.
        q: QuestionId,
        /// The decision.
        decision: DecisionId,
    },
    /// A people question moves with a merge.
    PqMoved {
        /// The question.
        q: QuestionId,
        /// The new node.
        to: NodeId,
    },
    /// An agent question is answered.
    AqAnswered {
        /// The question.
        q: QuestionId,
        /// The answer.
        by: Eid,
    },
    /// A user question is answered.
    UqAnswered {
        /// The question.
        q: QuestionId,
        /// The answer.
        by: Eid,
    },
    /// A user question takes its default.
    UqDefaulted {
        /// The question.
        q: QuestionId,
    },
    /// pending(k) -> pending(k+1), CS-2.
    UqReaddressed {
        /// The question.
        q: QuestionId,
        /// The new stage.
        stage: u8,
        /// The new stage's due.
        due: Time,
    },
    /// CS-2.
    UqLapsed {
        /// The question.
        q: QuestionId,
    },
    /// The lapse's assumption, CS-2.
    UqAssumed {
        /// The question.
        q: QuestionId,
        /// The decision.
        decision: DecisionId,
    },
    /// A commitment opens.
    CommitmentOpened {
        /// The commitment: the eid of the opening `ack`.
        c: crate::types::CommitmentId,
        /// The conversation node.
        node: NodeId,
        /// The conversation key.
        conv: KeyStr,
        /// `at + commitment.followup`.
        followup_at: Time,
    },
    /// A commitment is refreshed by an ack.
    CommitmentRefreshed {
        /// The commitment.
        c: crate::types::CommitmentId,
        /// The new follow-up.
        followup_at: Time,
    },
    /// A commitment is fulfilled.
    CommitmentFulfilled {
        /// The commitment.
        c: crate::types::CommitmentId,
        /// The fulfilling `said`.
        by: SaidId,
    },
    /// A commitment moves with a merge.
    CommitmentMoved {
        /// The commitment.
        c: crate::types::CommitmentId,
        /// The new node.
        to: NodeId,
    },
    /// A `said` is confirmed.
    SaidConfirmed {
        /// The `said`.
        said: SaidId,
        /// How.
        via: ConfirmVia,
        /// A link to the sent message.
        link: Option<Text>,
    },
    /// A `said` becomes unconfirmed.
    SaidUnconfirmed {
        /// The `said`.
        said: SaidId,
    },
    /// 1.9 (§9.2): a pending `said` is withheld: recorded by a session that held neither its
    /// conversation node nor work inside it while another did, or listed in `unsent` after a later
    /// `said` of its conversation replaced it.
    SaidWithheld {
        /// The `said`.
        said: SaidId,
    },
    /// Merge (A-94).
    SaidMoved {
        /// The `said`.
        said: SaidId,
        /// The new node.
        to: NodeId,
    },
    /// A user entry is routed.
    EntryRouted {
        /// The entry.
        entry: UserEntryId,
        /// How.
        routing: Routing,
        /// Why, for `to: none`.
        reason: Option<Text>,
    },
    /// A decision is superseded.
    DecisionSuperseded {
        /// The decision.
        d: DecisionId,
        /// The new decision.
        by: DecisionId,
    },
    /// A decision is retracted.
    DecisionRetracted {
        /// The decision.
        d: DecisionId,
        /// The `retract` entry.
        by: Eid,
        /// 1.11 (§11.3, §24): the decision's node, which keeps the decision as its newest retracted
        /// one (`Node::retracted_decision`), so that `no_basis` names it after it leaves memory.
        /// Appended with a default (D.7.6): a log written before 1.11 replays to the state it wrote,
        /// with none.
        #[cfg_attr(feature = "serde", serde(default))]
        node: Option<NodeId>,
    },
    /// An objection is resolved, or upheld (1.8, §11.3).
    ObjectionResolved {
        /// The objection.
        o: ObjectionId,
        /// The `resolve` entry.
        by: Eid,
        /// 1.8: the objection stood (`ObjectionState::Upheld`). Appended with a default (D.7.6).
        #[cfg_attr(feature = "serde", serde(default))]
        upheld: bool,
    },
    /// Merge (A-94).
    ObjectionMoved {
        /// The objection.
        o: ObjectionId,
        /// The new node.
        to: NodeId,
    },
    /// A digest entry is shown.
    DigestShown {
        /// The digest entry.
        entry: crate::types::DigestId,
        /// The `to_user` entry.
        by: Eid,
    },
    /// One session's `cannot` (CS-7): its entry in the class's record; `quorum` is used only when
    /// the record is new.
    Refused {
        /// The node.
        node: NodeId,
        /// The class.
        class: ClassName,
        /// The refusing session.
        session: SessionId,
        /// `at + refusal.backoff`.
        backoff_until: Time,
        /// `at + refusal.ttl`.
        expires_at: Time,
        /// `refusal.quorum`.
        quorum: u32,
    },
    /// The entries of the record with `expires_at <= at` expire; the record goes with its last.
    RefusalExpired {
        /// The node.
        node: NodeId,
        /// The class.
        class: ClassName,
    },
    /// A-56.
    RefusalsCleared {
        /// The node.
        node: NodeId,
        /// The answer or the question.
        by: Eid,
    },
    /// A signal opens.
    SignalOpened {
        /// The key.
        key: SignalKey,
        /// The signal node.
        node: NodeId,
    },
    /// A signal resolves.
    SignalResolved {
        /// The key.
        key: SignalKey,
    },
    /// A pause is set or renewed; `class`: shown (CS-9).
    Paused {
        /// The scope.
        scope: NodeId,
        /// `at + pause.ttl`.
        until: Time,
        /// `pause.ttl`.
        ttl: Dur,
        /// The pausing session.
        by: SessionId,
        /// Its class.
        class: ClassName,
        /// A renewal of an active pause.
        renewed: bool,
    },
    /// A pause is lifted.
    Unpaused {
        /// The scope.
        scope: NodeId,
    },
    /// A pause expires.
    PauseExpired {
        /// The scope.
        scope: NodeId,
    },
    /// A `holder:` note is bound to the session that took the node.
    NoteBound {
        /// The note.
        note: NoteId,
        /// The taker.
        session: SessionId,
    },
    /// An entry or a file is hidden.
    Hidden {
        /// What.
        target: HideTarget,
        /// Why.
        reason: Text,
    },
    /// `expires_at = at + op.retain`. The outcome stays in the history only: evolve stores the
    /// digest, the session, and the expiry (CS-8).
    OpRecorded {
        /// The class.
        class: ClassName,
        /// The `op_id`.
        op: OpId,
        /// The request digest.
        digest: ReqDigest,
        /// The session.
        session: SessionId,
        /// When the record expires.
        expires_at: Time,
        /// The outcome.
        outcome: Result<Done, Error>,
    },

    // ---- memory (D-5)
    /// Entities that left memory, brought back because a command names them (D.4.12). The values
    /// are their state rows, read by the shell; evolve re-inserts them with their indices.
    Restored {
        /// The entities.
        items: Vec<Restored>,
    },

    // ---- 1.8
    /// An objection upheld on a closed node corrects it (§5, §11.3): `closed(o)` becomes
    /// `closed(outcome)` marked upheld by `by`. No counter changes (O2).
    NodeCorrected {
        /// The node.
        node: NodeId,
        /// Its outcome from now on: `refuted` or `failed` for one that was `done`, else unchanged.
        outcome: Outcome,
        /// The `resolve` entry.
        by: Eid,
        /// 1.10: the outcome a withdrawal restores, `closure.corrected_from`: the one the node was
        /// closed with. Appended with a default (D.7.6): a log written before 1.10 replays to the
        /// state it wrote, with none.
        #[cfg_attr(feature = "serde", serde(default))]
        was: Option<Outcome>,
    },
    /// 1.10 (§5, §11.3): a second, opposite verdict on the objection whose verdict marked a closed
    /// node upheld withdraws that correction: `closed(outcome′)` marked upheld becomes
    /// `closed(outcome)`, unmarked, with the outcome it was closed with. No counter changes (O2).
    CorrectionWithdrawn {
        /// The node.
        node: NodeId,
        /// Its outcome from now on: the one it was closed with (`corrected_from`).
        outcome: Outcome,
    },
}

/// What a `next` found (CS-31): a candidate; no candidate at all in the index (a true empty, the
/// idle share); or candidates that were all excluded for this caller (by its needs, horizon, class
/// refusals, query, or a pause), which is not idleness but work it cannot do.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum NextOutcome {
    /// A candidate.
    Item,
    /// No candidate at all.
    Nothing,
    /// Candidates, all excluded for this caller.
    NothingForYou,
}

/// How an intent closed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IntentEnd {
    /// By a checkpoint's `intent_done`.
    Done {
        /// The checkpoint entry.
        by: Eid,
    },
    /// By the closing session's close.
    ResolvedByClose,
}

/// What extended a hold.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Extension {
    /// A checkpoint.
    Checkpoint,
    /// A renew.
    Renew,
}

/// §25: live -> released | expired | ended_by_close. The cause says which restructuring ended it;
/// `Move` reuses ended_by_close for §16's "later writes become late" (A-6).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HoldEnd {
    /// Released.
    Released,
    /// Its lease ended.
    Expired,
    /// Ended by a close, a merge, or a move.
    EndedByClose(EndCause),
}

/// Which restructuring ended a hold.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum EndCause {
    /// A close.
    Close,
    /// A merge.
    Merge,
    /// A move.
    Move,
}

/// How a `said` was confirmed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum ConfirmVia {
    /// `said {said_id, confirmed}`.
    Explicit,
    /// The session's release.
    Release,
    /// The session's close.
    Close,
    /// 1.11 (§9.2): the `said` itself, recorded by a session that held nothing, so that no release
    /// or close of its will come: it is taken as sent with its author's reply (`with_reply`).
    Recorded,
}

/// What `hide` hides.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HideTarget {
    /// An entry.
    Entry(Eid),
    /// A file of a node.
    File {
        /// The node.
        node: NodeId,
        /// The file's name.
        name: Text,
    },
}

/// Everything a new node starts with. `due_fired` is computed by evolve (`due <= at`). `files`
/// starts empty (A-97).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeInit {
    /// The title.
    pub title: Text,
    /// The body.
    pub body: Text,
    /// The fields.
    pub fields: BTreeMap<Text, Text>,
    /// The status.
    pub status: Text,
    /// The digest.
    pub digest: Text,
    /// The agreements.
    pub agreements: Text,
    /// The kind.
    pub kind: Option<Label>,
    /// The tags.
    pub tags: BTreeSet<Label>,
    /// The capacity.
    pub capacity: u32,
    /// The priority.
    pub priority: i32,
    /// The estimate.
    pub est: Option<u32>,
    /// The needs.
    pub needs: BTreeSet<Label>,
    /// The due time.
    pub due: Option<Time>,
    /// The dependencies.
    pub depends_on: BTreeSet<NodeId>,
    /// The owners.
    pub owners: BTreeSet<NodeId>,
    /// The keys.
    pub keys: BTreeMap<KeyStr, NormVer>,
    /// The outgoing links.
    pub links: BTreeSet<(LinkRel, NodeId)>,
    /// The role.
    pub role: Role,
}

/// The fields an update changes, resolved. decide includes a field only if its value differs (O2;
/// for `repeat`, A-75's comparison); evolve counts an anomaly for a field equal to the current
/// value. `Change::Keep` is "absent" (D.2). `tags` and `needs` are the resulting sets, computed by
/// decide from the patch's additions and removals on the current sets (CS-34): the single writer
/// applies one command at a time, so the stored set is exact, and replay never recomputes it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodePatch {
    /// The title.
    pub title: Option<Text>,
    /// The body.
    pub body: Option<Text>,
    /// The fields.
    pub fields: Option<BTreeMap<Text, Text>>,
    /// The files.
    pub files: Option<BTreeMap<Text, Text>>,
    /// The status.
    pub status: Option<Text>,
    /// The digest.
    pub digest: Option<Text>,
    /// The agreements.
    pub agreements: Option<Text>,
    /// The kind.
    #[cfg_attr(feature = "serde", serde(default))]
    pub kind: Change<Label>,
    /// The resulting tags.
    pub tags: Option<BTreeSet<Label>>,
    /// The capacity.
    pub capacity: Option<u32>,
    /// The priority.
    pub priority: Option<i32>,
    /// The estimate.
    #[cfg_attr(feature = "serde", serde(default))]
    pub est: Change<u32>,
    /// The resulting needs.
    pub needs: Option<BTreeSet<Label>>,
    /// The due time.
    #[cfg_attr(feature = "serde", serde(default))]
    pub due: Change<Time>,
    /// The dependencies.
    pub depends_on: Option<BTreeSet<NodeId>>,
    /// The owners.
    pub owners: Option<BTreeSet<NodeId>>,
    /// The schedule.
    #[cfg_attr(feature = "serde", serde(default))]
    pub repeat: Change<Repeat>,
}

// ------------------------------------------------------------------------------------------------
// Entry bodies and texts (D.7.3)

/// The structural part of an entry.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EntryBody {
    /// A plain note (`to: None`), or an addressed message, `talk say` (§13, A-17). `recipients` and
    /// `pending_holder` are fixed by decide when the note is recorded (A-49); a `principal:` or
    /// `holders:` note has none, its readers being found when they read (CS-3, D-6).
    Note {
        /// The addressee.
        to: Option<Addressee>,
        /// Delivered until then.
        until: Option<Time>,
        /// The sessions it is delivered to.
        recipients: BTreeSet<SessionId>,
        /// For whoever takes the node next.
        pending_holder: bool,
        /// Stored only.
        expects: Option<Expects>,
        /// The entry it replies to.
        reply_to: Option<Eid>,
    },
    /// `requester` is `User(p)` with `p` the entry's principal when `from_user` is set, else
    /// `External` (the requester's name is then text, `TextField::Requester`) (CS-4). `origin` is
    /// the message, `conv` the conversation key the node was found by (`where`, or the origin
    /// without one; for a routed user entry its `user:e<n>` key), which C1's step check reads
    /// (A-61, D.9.2) and `evolve` stores as `Request::conv`, so that a `said` answers only its own
    /// conversation's requests (§9.4, 1.3).
    Request {
        /// The message.
        origin: Key,
        /// The conversation key.
        conv: KeyStr,
        /// The user entry of a `user:e<n>` origin.
        from_user: Option<UserEntryId>,
        /// Who asked.
        requester: Requester,
    },
    /// A question.
    Question(QuestionBody),
    /// `talk answer` to an agent question; delivered to the asking session.
    Answer {
        /// The question.
        question: QuestionId,
        /// The asking session.
        recipient: SessionId,
        /// Delivered until then.
        until: Time,
    },
    /// `basis`: the validated references (CS-22); empty for every decision the server or a close
    /// records. The `what` of each external reference is a `TextField::Basis` text, in order.
    Decision {
        /// The requests it settles.
        settles: BTreeSet<RequestId>,
        /// The decision it supersedes.
        supersedes: Option<DecisionId>,
        /// The questions whose assumptions it records.
        assumes: BTreeSet<QuestionId>,
        /// Stored only.
        evidence: Option<Evidence>,
        /// The references.
        basis: Vec<BasisRef>,
    },
    /// `hold checkpoint`, and the record of `hold release` (which counts as a checkpoint, A-9).
    Checkpoint {
        /// The hold.
        token: Token,
        /// The release's reason, for a release.
        release: Option<ReleaseReason>,
        /// 1.12 (§5, §20.9 rule 8): with an `intent_done`, whether it names a pull request
        /// (`words::names_pull_request`), which sets `Node::pr_named`; None without one. Appended
        /// with a default (D.7.6): a log written before 1.12 carries none, and replays to the
        /// state it wrote, whose nodes have no such flag.
        #[cfg_attr(feature = "serde", serde(default))]
        pr_named: Option<bool>,
        /// 1.12 (§10.4): for a `handoff` release, until when it keeps the node from its releaser,
        /// `lease.first` of the hold's terms after it, which sets `Session::handed_off`; None for
        /// every other checkpoint. Appended with a default (D.7.6): a log written before 1.12
        /// carries none, and replays to the state it wrote, whose sessions have no such hold.
        #[cfg_attr(feature = "serde", serde(default))]
        handed_off_until: Option<Time>,
    },
    /// A `said` entry.
    Said {
        /// The conversation key.
        conv: KeyStr,
        /// The kind.
        kind: SaidKind,
        /// The `said` it corrects.
        corrects: Option<SaidId>,
        /// 1.11 (§9.1, §9.2): its author held neither the conversation node nor work inside it,
        /// while other sessions did, none of them with the cap the conversation's scheme names, and
        /// it had that cap: it spoke as the conversation's voice, which briefs show. Appended with a
        /// default (D.7.6).
        #[cfg_attr(feature = "serde", serde(default))]
        voice: bool,
    },
    /// A `heard` entry.
    Heard {
        /// The conversation key.
        conv: KeyStr,
        /// The people question it answers.
        answers: Option<QuestionId>,
    },
    /// Whose words (CS-1).
    FromUser {
        /// The principal.
        principal: PrincipalId,
    },
    /// To whom (CS-4).
    ToUser {
        /// The kind.
        kind: ToUserKind,
        /// Whether the text is what the human sees.
        verbatim: bool,
        /// What it reports on.
        re: BTreeSet<Eid>,
        /// The principal.
        principal: PrincipalId,
    },
    /// An approval of a version.
    Approve {
        /// The version.
        version: Version,
    },
    /// An objection.
    Object,
    /// The resolution of an objection.
    Resolve {
        /// The objection.
        objection: ObjectionId,
        /// 1.8 (§11.3): resolved as upheld. Appended with a default (D.7.6).
        #[cfg_attr(feature = "serde", serde(default))]
        upheld: bool,
        /// 1.11 (§11.3): a deferral, which decides nothing: its author cannot decide the objection
        /// yet, and its text says why; the objection stays open (`Objection::deferred`). Appended
        /// with a default (D.7.6).
        #[cfg_attr(feature = "serde", serde(default))]
        deferred: bool,
    },
    /// A retraction.
    Retract {
        /// What is retracted.
        target: RetractTarget,
    },
    /// A server entry.
    System(SystemNote),
    /// A write made with a stale token (H4), with exactly what was attempted. The command lives in
    /// the event and the log only; evolve stores nothing of it in the state.
    Late {
        /// The stale token.
        token: Token,
        /// The attempted command.
        attempted: Box<Cmd>,
    },
    /// 1.8 (§10.2): `hold check {intent_done}`, what became of the node's open intent, recorded
    /// without a hold by the entry's author; its text is the `intent_done`. A `checkpoint` entry
    /// (§6) that is no checkpoint of a hold: it counts for no counter of §5.
    IntentChecked {
        /// The intent it closed.
        intent: Eid,
        /// 1.12 (§5, §20.9 rule 8): its text, the `intent_done`, names a pull request, which sets
        /// `Node::pr_named`. Appended with a default (D.7.6): false in a log written before 1.12,
        /// whose nodes have no such flag.
        #[cfg_attr(feature = "serde", serde(default))]
        pr_named: bool,
    },
}

/// The body of a question.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum QuestionBody {
    /// To a person outside the swarm.
    People {
        /// The conversation key.
        conv: KeyStr,
        /// Whether it escalates.
        blocking: bool,
        /// The `said` of kind question recorded with it.
        said: SaidId,
        /// The question it supersedes (`again`).
        supersedes: Option<QuestionId>,
        /// The first recheck (A-24).
        recheck_at: Time,
    },
    /// To agents.
    Agent {
        /// The addressee.
        to: Addressee,
        /// Delivered until then.
        until: Time,
        /// The sessions it is delivered to.
        recipients: BTreeSet<SessionId>,
        /// For whoever takes the node next.
        pending_holder: bool,
    },
    /// Stage 0 of the chain `to`, due at `due` (CS-2, D.8.12).
    User {
        /// Why it was asked.
        source: UqSource,
        /// The node it is about.
        about: Option<NodeId>,
        /// The addressee chain.
        to: Vec<PrincipalId>,
        /// Stage 0's due.
        due: Time,
        /// The default text is in the entry.
        has_default: bool,
    },
}

/// What a `retract` strikes out: the type of a basis target, so a retraction is looked up in
/// `ix.cited_by` and stored in `State::retracted` as it is (D-4).
pub type RetractTarget = BasisTarget;

/// A server entry.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SystemNote {
    /// On `USER_DIGEST`: creates a digest entry for one audience principal (CS-5).
    Digest {
        /// What happened.
        item: DigestItem,
        /// For whom.
        to: PrincipalId,
    },
    /// Text: summary, verified, learnings.
    Closed {
        /// The outcome.
        outcome: Outcome,
    },
    /// Text: the reason.
    Reopened,
    /// K7: keys another open node carries.
    KeysDropped {
        /// Each key and its open carrier.
        keys: Vec<(KeyStr, NodeId)>,
    },
    /// Text: from's digest (§16).
    MergedFrom {
        /// The merged node.
        from: NodeId,
        /// Its last checkpoint.
        last_checkpoint: Option<Eid>,
    },
    /// A move.
    Moved {
        /// The old place.
        from: Place,
    },
    /// Text: the why.
    Renewed {
        /// The hold.
        token: Token,
    },
    /// A series with no further slot (A-51).
    ScheduleEnded,
    /// Text: the reason.
    Paused {
        /// Until when.
        until: Time,
    },
    /// A pause lifted.
    Unpaused,
    /// A pause expired.
    PauseExpired,
    /// Text: the reason.
    Hidden(HideTarget),
    /// The default became the answer; rendered from the question's own entry.
    Defaulted {
        /// The question.
        question: QuestionId,
    },
}

/// The free text of an entry. The core never stores it; the history records it once, in this
/// event, and the shell renders and indexes it from there (D.11.6, D.11.12).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EntryText {
    /// The main text.
    pub main: Text,
    /// Further texts, each with its role.
    pub extra: Vec<(TextField, Text)>,
}

/// The role of a further text of an entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum TextField {
    /// A checkpoint's `done`.
    Done,
    /// A checkpoint's `next_step`.
    NextStep,
    /// An intent.
    Intent,
    /// What happened to an intent.
    IntentDone,
    /// A file of a checkpoint.
    File,
    /// An option of a user question.
    Option,
    /// The recommended option.
    Recommendation,
    /// The default.
    Default,
    /// A `from_user`'s client.
    Client,
    /// A `heard`'s speaker.
    From,
    /// A request's requester.
    Requester,
    /// A close's summary.
    Summary,
    /// How it was verified.
    Verified,
    /// Learnings.
    Learnings,
    /// A reason.
    Reason,
    /// What an approval checked.
    Checked,
    /// An external basis reference's `what`.
    Basis,
    /// A confirmation link.
    Link,
    /// An external wait's `what`.
    What,
    /// A renewal's why.
    Why,
}

/// Whether the core keeps an entry's `EntryMeta` (CS-17): every body except a checkpoint, a plain
/// note (`Note { to: None }`), a system entry other than `System(Digest)`, and a late write. Those
/// are about half of all entries at 10,000 agents (checkpoints alone are 10⁹ a year); the core
/// reads none of their metadata, so they live in the history and the shell's `entries` table only.
#[must_use]
pub fn keeps_meta(body: &EntryBody) -> bool {
    match body {
        EntryBody::Note { to, .. } => to.is_some(),
        EntryBody::System(note) => match note {
            SystemNote::Digest { .. } => true,
            SystemNote::Closed { .. }
            | SystemNote::Reopened
            | SystemNote::KeysDropped { .. }
            | SystemNote::MergedFrom { .. }
            | SystemNote::Moved { .. }
            | SystemNote::Renewed { .. }
            | SystemNote::ScheduleEnded
            | SystemNote::Paused { .. }
            | SystemNote::Unpaused
            | SystemNote::PauseExpired
            | SystemNote::Hidden(_)
            | SystemNote::Defaulted { .. } => false,
        },
        EntryBody::Checkpoint { .. } | EntryBody::Late { .. } | EntryBody::IntentChecked { .. } => {
            false
        }
        EntryBody::Request { .. }
        | EntryBody::Question(_)
        | EntryBody::Answer { .. }
        | EntryBody::Decision { .. }
        | EntryBody::Said { .. }
        | EntryBody::Heard { .. }
        | EntryBody::FromUser { .. }
        | EntryBody::ToUser { .. }
        | EntryBody::Approve { .. }
        | EntryBody::Object
        | EntryBody::Resolve { .. }
        | EntryBody::Retract { .. } => true,
    }
}

impl EntryBody {
    /// The §6 type of the entry.
    #[must_use]
    pub fn entry_type(&self) -> crate::types::EntryType {
        use crate::types::EntryType as T;
        match self {
            EntryBody::Note { .. } => T::Note,
            EntryBody::Request { .. } => T::Request,
            EntryBody::Question(_) => T::Question,
            EntryBody::Answer { .. } => T::Answer,
            EntryBody::Decision { .. } => T::Decision,
            EntryBody::Checkpoint { .. } | EntryBody::IntentChecked { .. } => T::Checkpoint,
            EntryBody::Said { .. } => T::Said,
            EntryBody::Heard { .. } => T::Heard,
            EntryBody::FromUser { .. } => T::FromUser,
            EntryBody::ToUser { .. } => T::ToUser,
            EntryBody::Approve { .. } => T::Approve,
            EntryBody::Object => T::Object,
            EntryBody::Resolve { .. } => T::Resolve,
            EntryBody::Retract { .. } => T::Retract,
            EntryBody::System(_) => T::System,
            EntryBody::Late { .. } => T::Late,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;
    use alloc::vec;

    /// `keeps_meta` for every `EntryBody` variant (CS-17, D.12.1): every body but a checkpoint, a
    /// plain note, a system entry other than `System(Digest)`, and a late write.
    #[test]
    fn keeps_meta_for_every_body() {
        let token = Token::default();
        let conv = KeyStr::of_user_entry(Eid(1));
        let note = |to| EntryBody::Note {
            to,
            until: None,
            recipients: BTreeSet::new(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        };
        let kept = vec![
            note(Some(Addressee::Session(SessionId(1)))),
            EntryBody::Request {
                origin: Key {
                    key: conv.clone(),
                    nv: NormVer(0),
                },
                conv: conv.clone(),
                from_user: Some(Eid(1)),
                requester: Requester::User(PrincipalId(0)),
            },
            EntryBody::Question(QuestionBody::People {
                conv: conv.clone(),
                blocking: false,
                said: Eid(1),
                supersedes: None,
                recheck_at: Time(1),
            }),
            EntryBody::Answer {
                question: Eid(1),
                recipient: SessionId(1),
                until: Time(1),
            },
            EntryBody::Decision {
                settles: BTreeSet::new(),
                supersedes: None,
                assumes: BTreeSet::new(),
                evidence: None,
                basis: vec![],
            },
            EntryBody::Said {
                conv: conv.clone(),
                kind: SaidKind::Ack,
                corrects: None,
                voice: false,
            },
            EntryBody::Heard {
                conv,
                answers: None,
            },
            EntryBody::FromUser {
                principal: PrincipalId(0),
            },
            EntryBody::ToUser {
                kind: ToUserKind::Result,
                verbatim: true,
                re: BTreeSet::new(),
                principal: PrincipalId(0),
            },
            EntryBody::Approve {
                version: Version(1),
            },
            EntryBody::Object,
            EntryBody::Resolve {
                objection: Eid(1),
                upheld: false,
                deferred: false,
            },
            EntryBody::Retract {
                target: BasisTarget::Entry(Eid(1)),
            },
            EntryBody::System(SystemNote::Digest {
                item: DigestItem::Lapsed { question: Eid(1) },
                to: PrincipalId(0),
            }),
        ];
        let dropped = vec![
            note(None),
            EntryBody::Checkpoint {
                token,
                release: None,
                pr_named: Some(true),
                handed_off_until: None,
            },
            EntryBody::Checkpoint {
                token,
                release: Some(ReleaseReason::Handoff),
                pr_named: None,
                handed_off_until: Some(Time(1)),
            },
            EntryBody::Late {
                token,
                attempted: Box::new(Cmd::Control(crate::cmd::ControlCmd::Health)),
            },
            EntryBody::IntentChecked {
                intent: Eid(1),
                pr_named: false,
            },
            EntryBody::System(SystemNote::Closed {
                outcome: Outcome::Done,
            }),
            EntryBody::System(SystemNote::Reopened),
            EntryBody::System(SystemNote::KeysDropped { keys: vec![] }),
            EntryBody::System(SystemNote::MergedFrom {
                from: NodeId(9),
                last_checkpoint: None,
            }),
            EntryBody::System(SystemNote::Moved {
                from: Place {
                    parent: NodeId(0),
                    name: Name::new("x").unwrap(),
                },
            }),
            EntryBody::System(SystemNote::Renewed { token }),
            EntryBody::System(SystemNote::ScheduleEnded),
            EntryBody::System(SystemNote::Paused { until: Time(1) }),
            EntryBody::System(SystemNote::Unpaused),
            EntryBody::System(SystemNote::PauseExpired),
            EntryBody::System(SystemNote::Hidden(HideTarget::Entry(Eid(1)))),
            EntryBody::System(SystemNote::Defaulted { question: Eid(1) }),
        ];
        for b in &kept {
            assert!(keeps_meta(b), "{b:?}");
        }
        for b in &dropped {
            assert!(!keeps_meta(b), "{b:?}");
        }
        let types: BTreeSet<_> = kept
            .iter()
            .chain(&dropped)
            .map(EntryBody::entry_type)
            .collect();
        assert_eq!(types.len(), 16, "every entry type has a body above");
    }
}
