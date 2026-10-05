//! The state (design D.4): the base facts mirrored by the Quint model, the history that depends on
//! the order of events, and the derived indices with their full-recount oracle.
//!
//! The base and history parts are what the shell keeps in its SQLite state tables, the source of
//! truth (D.11.6, FQ-2); the indices are recounted from them at startup (D.11.8). Every map whose
//! key is neither a string nor an integer is encoded as a list of pairs in key order
//! (`crate::serde_pairs`, D.7.6).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::params::Params;
use crate::types::{
    ClassName, CommitmentId, DecisionId, DigestId, Dur, Eid, EntryType, Fence, FlagSet, Key,
    KeyStr, Label, LinkRel, Name, NodeId, NormVer, NoteId, ObjectionId, OpId, Outcome, PEOPLE,
    PrincipalId, PrincipalKey, QuestionId, ReqDigest, RequestId, Requester, SaidId, SaidKind, Seq,
    SessionId, SignalKind, TEAMS, Text, Time, Token, UserEntryId, Version,
};

/// How long a finished entity stays in memory (D-5, D.4.12). Not a parameter of §22: no agent sees
/// it, and it is real time, never scaled, so no model run reaches it (D.10.5).
pub const FINISHED_RETAIN: Dur = Dur(604_800_000); // 7 days

// ------------------------------------------------------------------------------------------------
// The state (D.4.2)

/// The whole state of the core.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct State {
    // ---------- base: facts (mirrored by the Quint model, D.10.3; stored in the state tables) ----
    /// The parameters in force (D.3).
    pub params: Params,
    /// T1: the largest `at` of any applied event.
    pub last_now: Time,
    /// The id counters and the event count.
    pub counters: Counters,
    /// Events that `evolve` could not apply as given (D.7.5). `check_invariants` requires 0.
    pub anomalies: u64,

    /// Every node.
    pub nodes: BTreeMap<NodeId, Node>,
    /// Live names: (parent, name) -> node. Disjoint from `aliases` (K3).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub names: BTreeMap<(NodeId, Name), NodeId>,
    /// Former names: (parent, name) -> node.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub aliases: BTreeMap<(NodeId, Name), NodeId>,

    /// Metadata of the feed entries the core reads (`keeps_meta`, D.4.5, CS-17). Immutable once
    /// inserted (O3); it leaves memory with its entity, or `FINISHED_RETAIN` after its own time when
    /// it bears none (D.4.12). Every other entry is counted in its node's `seq` and kept in the
    /// history.
    pub entries: BTreeMap<Eid, EntryMeta>,

    /// Holds that have not ended. An ended hold leaves the map; its history is in the log.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub holds: BTreeMap<Token, Hold>,

    // The entity maps hold the resident entities: a finished one leaves memory FINISHED_RETAIN after
    // it finished, and its state row stays in the state tables (D.4.12).
    /// Requests (§9.4).
    pub requests: BTreeMap<RequestId, Request>,
    /// Questions to people (§9.3).
    pub pquestions: BTreeMap<QuestionId, PQuestion>,
    /// Questions between agents (§13); one leaves with its note, at `until` (D.4.12).
    pub aquestions: BTreeMap<QuestionId, AQuestion>,
    /// Questions for the user (§8.4).
    pub uquestions: BTreeMap<QuestionId, UQuestion>,
    /// Commitments (§9.2).
    pub commitments: BTreeMap<CommitmentId, Commitment>,
    /// `said` entries (§9.2).
    pub saids: BTreeMap<SaidId, Said>,
    /// Decisions (§11.1).
    pub decisions: BTreeMap<DecisionId, Decision>,
    /// Objections (§11.3).
    pub objections: BTreeMap<ObjectionId, Objection>,
    /// `from_user` entries (§8.1).
    pub user_entries: BTreeMap<UserEntryId, UserEntry>,
    /// `@user/digest` (§8.5).
    pub digest: BTreeMap<DigestId, DigestEntry>,
    /// Addressed messages (§13) until their `until`: evolve drops a note at the first event at or
    /// after it (CS-17), as nothing delivers it later.
    pub notes: BTreeMap<NoteId, Note>,
    /// `approve` entries per node version (§11.2 warning). The warning counts the versions from the
    /// node's `content_version` on (CS-34).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub approvals: BTreeMap<(NodeId, Version), u32>,
    /// What `talk retract` struck out: an entry or a node, never removed (O3). A basis reference to
    /// a target here is stale (`basis_changed`, D-4, D.8.17). Retractions are rare, so this grows
    /// with history only by them.
    pub retracted: BTreeSet<BasisTarget>,

    /// Live refusals, one record per (node, class) with an entry per refusing session (CS-7). An
    /// entry leaves when it expires; the record leaves with its last entry, or when cleared.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub refusals: BTreeMap<(NodeId, ClassName), Refusal>,
    /// The classes that asked for work with an open `next` within `refusal.window`, with the caps
    /// they asked with (E1, CS-7). A class leaves the map when its last caps profile expires.
    pub classes: BTreeMap<ClassName, ClassActivity>,
    /// Open signals: key -> signal node (S1). A resolved signal leaves the map.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub signals: BTreeMap<SignalKey, NodeId>,
    /// Active pauses by scope node. An expired or lifted pause leaves the map.
    pub pauses: BTreeMap<NodeId, Pause>,

    /// Live and silent sessions. A session leaves at `SessionRetired`, `session.retain` after its
    /// last `Touched` (its presence's last refresh, §3), holding nothing (CS-8, D.8.23).
    pub sessions: BTreeMap<SessionId, Session>,
    /// Interned principals (§3, CS-1): key -> id, never removed. Genesis interns `user` as
    /// `USER_PRINCIPAL`; every other key enters by `PrincipalSeen` the first time a command names
    /// it, a `person:<key>` key included (D.2.2), so every alias of a person has an id.
    pub principal_ids: BTreeMap<PrincipalKey, PrincipalId>,
    /// Retry store (O1): (class, op_id) -> digest, session, and expiry. Evolve drops a record at the
    /// first event at or after its `expires_at` (CS-8).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub ops: BTreeMap<(ClassName, OpId), OpRecord>,

    // ---------- history: values that depend on event order (D.4.9; stored in the state tables) ----
    /// Statistics for `control health` and the coverage warning.
    pub stats: Stats,
    /// The time each candidate of `next` became one (§10.4 `since`): the event that made it a
    /// candidate, an occurrence's slot, a routing item's entry time. It depends on history, so it
    /// is stored, not recounted; its key set equals `ix.next.of`'s (R1, CS-16).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub since: BTreeMap<Cand, Time>,
    /// Each principal's `from_user` times, ascending. At each message of `p`, evolve drops the times
    /// of `p` before `at − user_gap.window`, with the window in force then (D.8.12, CS-2).
    pub user_msgs: BTreeMap<PrincipalId, Vec<Time>>,
    /// The time each resident entity leaves memory (D.4.12): `FINISHED_RETAIN` after it finished,
    /// or after the last reference that held it back went, or after a restore; for a pending digest
    /// entry, `DIGEST_RETAIN` after its creation or its restore.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub leaving: BTreeMap<EntityRef, Time>,
    /// Digest entries that left the pending list unshown after `DIGEST_RETAIN`, per principal
    /// (CS-5, D.4.12): `hello` adds them to its count as older entries, in `@user/digest`.
    pub digest_left: BTreeMap<PrincipalId, u64>,

    // ---------- derived: indices with a full-recount oracle (R1, F, T4) ----------
    /// The derived indices (D.4.10).
    pub ix: Ix,
}

/// The id counters and the event count.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Counters {
    /// Starts at `FIRST_FREE_NODE`.
    pub next_node: NodeId,
    /// Starts at `Eid(1)`.
    pub next_eid: Eid,
    /// Starts at `SessionId(1)`.
    pub next_session: SessionId,
    /// Starts at `PrincipalId(1)`; 0 is `user` (CS-1).
    pub next_principal: PrincipalId,
    /// Events applied since genesis (O3: the log length).
    pub events: u64,
}

impl Default for Counters {
    /// The starting values of D.4.2.
    fn default() -> Counters {
        Counters {
            next_node: crate::types::FIRST_FREE_NODE,
            next_eid: Eid(1),
            next_session: SessionId(1),
            next_principal: PrincipalId(1),
            events: 0,
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Nodes (D.4.3)

/// A node (§5).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Node {
    /// None only for ROOT.
    pub place: Option<Place>,
    /// §5 `created_by`.
    pub origin: Origin,
    /// When the node was created.
    pub created_at: Time,

    // content (O2): a change of title, body, fields, files, or status bumps `version`; a change of
    // digest bumps `digest_version`; agreements bump neither (A-3).
    /// The title.
    pub title: Text,
    /// The body.
    pub body: Text,
    /// Value: canonical JSON text (sorted keys, no spaces).
    pub fields: BTreeMap<Text, Text>,
    /// File name -> a reference: a URL or a path, with an optional content hash (`sha256:<hex>`) the
    /// server stores and never checks; the artefact lives outside the forum (FQ-5); empty at
    /// creation, set by `update` only (A-97).
    pub files: BTreeMap<Text, Text>,
    /// The status.
    pub status: Text,
    /// At most `params.digest_max` bytes.
    pub digest: Text,
    /// Empty means none (§11.2).
    pub agreements: Text,
    /// 1.4 (§20.4, D.14.5): when the agreements were last set, by `create` or `update`; None while
    /// the node never had any. `view agreements` shows it as the agreements' last change.
    pub agreements_at: Option<Time>,

    // labels and settings
    /// The kind.
    pub kind: Option<Label>,
    /// Contains `offer` while the node is offered.
    pub tags: BTreeSet<Label>,
    /// Slots, default 1; reserved nodes have 0.
    pub capacity: u32,
    /// Default 0, higher first.
    pub priority: i32,
    /// Minutes.
    pub est: Option<u32>,
    /// The caps a taker needs.
    pub needs: BTreeSet<Label>,
    /// The due time.
    pub due: Option<Time>,
    /// One-shot marker for the Due deadline: true once `now >= due` has been processed (D.4.8).
    /// Never read by a flag predicate.
    pub due_fired: bool,
    /// The node's owners (D-3, FQ-5): person or team nodes (D.2.2); empty means "inherit from the
    /// nearest owned ancestor". On a team node this field itself names the team's members, never an
    /// inherited value: a team with none has no members (§4, A-116). A setting like `needs`: it
    /// bumps no counter (A-3), and an `update` replaces it whole, last writer wins.
    pub owners: BTreeSet<NodeId>,

    /// The keys, each with its normaliser version (K2).
    pub keys: BTreeMap<KeyStr, NormVer>,
    /// Outgoing links.
    pub links: BTreeSet<(LinkRel, NodeId)>,
    /// The nodes this one depends on.
    pub depends_on: BTreeSet<NodeId>,
    /// The node's own wait (§10.3); at most one.
    pub wait: Option<Wait>,
    /// Plain, a series, an occurrence, or a signal.
    pub role: Role,
    /// Open or closed.
    pub life: Life,

    // the four counters of §5 (O2)
    /// Content version.
    pub version: Version,
    /// Digest version.
    pub digest_version: Version,
    /// Hold counter.
    pub fence: Fence,
    /// The feed's last sequence number.
    pub seq: Seq,
    /// The `version` at the last change of title, body, fields, or files (1 at creation). Not a
    /// counter of its own: approvals from this version on are approvals of the current content
    /// (§11.2, CS-34), so a status, key, link, or open/closed change does not reset them.
    pub content_version: Version,

    // maintained by the server (§5)
    /// Holds ended without a checkpoint since the last one.
    pub attempts: u32,
    /// Checkpoint counter.
    pub checkpoints: u32,
    /// The last checkpoint entry.
    pub last_checkpoint: Option<Eid>,
    /// 1.4 (§10.4, D.14.5, re-run R30): the last hold that expired after the node's last
    /// checkpoint without a checkpoint of its own, which the brief's section 2 shows (an outside
    /// action may have happened); cleared by the next checkpoint or release on the node.
    pub expired_hold: Option<ExpiredHold>,
    /// At most one open intent (§10.2).
    pub open_intent: Option<Intent>,
    /// Questions whose recheck fired for this node: input of `question_recheck` (A-81).
    pub recheck_marks: BTreeSet<QuestionId>,
    /// Expired questions this node must proceed on: input of `proceed_on_assumption`.
    pub assume_marks: BTreeSet<QuestionId>,
    /// Total time held, credited when a hold ends (median of completed work, D.8.22).
    pub held_ms: u64,
    /// The highest `seq` of a people entry of this node whose metadata left memory (D.4.12): the
    /// `moved` warning's total is exact only for a `seen` at or above it (D.8.15).
    pub people_left: Seq,
    /// How many entities whose home is this node have left memory (D.4.12): the brief's sections 3
    /// and 4 then say that older items are in the feed (D.8.6).
    pub left: u32,
    /// 1.8 (§5, §14): when the node was last touched: created or reopened, a field, key, or
    /// outgoing link changed, an entry other than a `late` one appended to its feed, or its time or
    /// external wait resolved at its time. None only for a node loaded from a state written before
    /// 1.8, which no deadline makes stale until it is touched.
    #[cfg_attr(feature = "serde", serde(default))]
    pub touched_at: Option<Time>,
    /// 1.8: the one-shot marker of the Stale deadline at `touched_at + offer.stale` (D.4.8); a
    /// touch clears it. Never read by a flag predicate.
    #[cfg_attr(feature = "serde", serde(default))]
    pub stale_fired: bool,
    /// 1.11 (§11.3, §24): the newest decision of the node that was retracted, kept in the node's
    /// state, not read from the decisions in memory, so that `no_basis` and the brief's "closed
    /// done, decision eX retracted" still name it once the retracted decision has left memory, 7
    /// days after its retraction. Set by a `DecisionRetracted` that names the node; None while none
    /// was, and for a retraction recorded before 1.11, whose log replays to the state it wrote. In
    /// the lab's sixth round (l5), 1.10's `no_basis` never fired: the retractions were 9 to 31 days
    /// old.
    #[cfg_attr(feature = "serde", serde(default))]
    pub retracted_decision: Option<DecisionId>,
    /// 1.12 (§10.2, §20.9 rule 8): the node's last `intent_done`, of a checkpoint, a release, or a
    /// `hold check`, names a pull request (`words::names_pull_request`, 1.10's detector): its work
    /// waits for a merge (`merge_wait`), so rule 8's `why` names the blocked release, and a
    /// decision recorded on it makes no close a `now` item. Set by the `Appended` of the entry
    /// that carries the `intent_done`, from the flag the entry carries (`EntryBody::Checkpoint`'s
    /// and `IntentChecked`'s `pr_named`), and cleared by a reopen, so that it describes the
    /// node's current round of work; false for a node none of whose intents closed so since 1.12,
    /// and in a log written before 1.12, whose entries carry no flag, so that it replays to the
    /// state it wrote (D.7.6). Read by the next calls only. In the lab's seventh round, 1.11's
    /// "decide now" made three of six workers file a pull request's status as a decision, and the
    /// close of the unmerged work then came as a `now` item.
    #[cfg_attr(feature = "serde", serde(default))]
    pub pr_named: bool,
}

impl Node {
    /// 1.12 (§5, §20.9 rule 8): the node's work waits for its merge, as far as its texts say: its
    /// last `intent_done` names a pull request (`pr_named`), or its open intent does
    /// (`Intent::pr_named`), the action announced before the release that closes it.
    #[must_use]
    pub fn merge_wait(&self) -> bool {
        self.pr_named || self.open_intent.is_some_and(|i| i.pr_named)
    }

    /// Whether the node is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        matches!(self.life, Life::Open)
    }

    /// The node's parent, None for ROOT.
    #[must_use]
    pub fn parent(&self) -> Option<NodeId> {
        self.place.as_ref().map(|p| p.parent)
    }

    /// `merged(n)` of D.8.3: closed with `merged_into ≠ None`, never a candidate (A-94).
    #[must_use]
    pub fn merged_into(&self) -> Option<NodeId> {
        match &self.life {
            Life::Open => None,
            Life::Closed(c) => c.merged_into,
        }
    }

    /// `is_directory(n)`: a person or a team of the directory, with or without `person:` keys (§4,
    /// A-113): a child of `@people` or `@teams`.
    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.parent().is_some_and(|p| p == PEOPLE || p == TEAMS)
    }

    /// `is_container(n)`: reserved, a series, or a person or team (A-29, A-113). Containers never
    /// receive the server's `offer` tag when a hold ends, and an `offer` tag on them does not make
    /// them candidates for `next`; only their flags do.
    #[must_use]
    pub fn is_container(&self) -> bool {
        matches!(self.origin, Origin::Reserved)
            || matches!(self.role, Role::Series(_))
            || self.is_directory()
    }
}

/// Where a node is: its parent and its name there.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Place {
    /// The parent node.
    pub parent: NodeId,
    /// The node's live name under it.
    pub name: Name,
}

/// Who made a node (§5 `created_by`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Origin {
    /// The eight reserved nodes made by Genesis.
    Reserved,
    /// `node create`.
    Agent(SessionId),
    /// A missing ancestor made by `node create` or `move` (§4).
    Ancestor(SessionId),
    /// Made by the server for a series (§12).
    Occurrence,
    /// Made by the server under `sys/signals` (§14).
    Signal,
}

/// Open or closed.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Life {
    /// Open.
    Open,
    /// Closed, with how.
    Closed(Closure),
}

/// How a node was closed.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Closure {
    /// AR: archived iff `now >= at + archive.after`.
    pub at: Time,
    /// The outcome.
    pub outcome: Outcome,
    /// K5: only with `Outcome::Superseded`.
    pub merged_into: Option<NodeId>,
    /// 1.8 (§5, §11.3): the `resolve` entry of an objection upheld on the node while it was closed,
    /// which corrected its outcome. A node marked so is never a candidate, and a take of it is
    /// `closed`; a reopen drops the closure, and the mark with it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub upheld: Option<Eid>,
    /// 1.10 (§5, §11.3): the outcome the node was closed with, before the first correction that
    /// still marks it upheld: a second, opposite verdict on the objection whose verdict marked it
    /// withdraws the correction and restores this outcome. None while no correction marks it, and
    /// for a correction recorded before 1.10, whose withdrawal keeps the outcome.
    #[cfg_attr(feature = "serde", serde(default))]
    pub corrected_from: Option<Outcome>,
    /// 1.11 (§13): the session that closed the node; None for the server, and for a close recorded
    /// before 1.11. A `say` without `to` at the closed node reaches that session while it is live
    /// (not silent, not retired), and nobody otherwise: a closed node is never taken, so the next
    /// holder 1.10 named would never come.
    #[cfg_attr(feature = "serde", serde(default))]
    pub by: Option<SessionId>,
}

/// A node's role.
///
/// Which occurrence of a series is open is derived (`Ix::series_open`), so opening or closing an
/// occurrence changes one node only. K6: that set has exactly one element while the series is
/// open, and none while it is closed.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Role {
    /// An ordinary node.
    Plain,
    /// A recurring node (§12).
    Series(Series),
    /// One occurrence of a series.
    Occurrence(Occurrence),
    /// A signal node (§14).
    Signal(SignalKey),
}

/// A recurring node.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Series {
    /// The schedule.
    pub repeat: Repeat,
    /// Input of `missed` (§14).
    pub last_missed: bool,
}

/// A schedule: cron, frozen time zone, and window.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Repeat {
    /// As the agent wrote it, for rendering.
    pub cron_text: Text,
    /// The parsed cron.
    pub cron: forum_cron::CronSpec,
    /// Frozen by the shell (D.11.10).
    pub tz: forum_cron::TzTable,
    /// Greater than zero.
    pub run_within: Dur,
}

/// An occurrence of a series.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Occurrence {
    /// The series.
    pub series: NodeId,
    /// UTC minute.
    pub slot: Time,
    /// `slot + run_within`; also the node's `due` (A-57).
    pub window_end: Time,
    /// Waiting or available.
    pub phase: OccPhase,
    /// One-shot marker for the OccurrenceWindow deadline.
    pub window_fired: bool,
}

/// §25 occurrence states: `held` is derived from holds; closed states are the node's closure.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum OccPhase {
    /// Before its slot.
    Waiting,
    /// From its slot on.
    Available,
}

/// A node's wait (§10.3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Wait {
    /// What it waits on.
    pub on: WaitOn,
    /// When it was set.
    pub set_at: Time,
    /// One-shot marker for the Wait deadline of `Time` and `External` waits. Resolution itself is
    /// the predicate `resolved(w, now)` of D.8.3 (T3); this field only keeps the queue exact.
    pub fired: bool,
}

/// What a wait waits on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WaitOn {
    /// A question to a person: resolves at its next recheck or when it leaves Open.
    Question {
        /// The question.
        q: QuestionId,
        /// Its rechecks when the wait was set.
        rechecks_at_set: u32,
    },
    /// A question for the user: resolves when it is answered, defaulted, or lapsed (A-31, CS-2).
    UserQuestion(QuestionId),
    /// Resolves while the target is closed (a merged node is closed).
    Node(NodeId),
    /// `{time}`: resolves at that time.
    Time(Time),
    /// `{external: what, recheck_after}`: resolves at that time. The `what` text is in the event.
    External(Time),
}

/// A hold that expired without a checkpoint of its own (1.4, §10.4): its token, its taker, and when
/// it expired.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExpiredHold {
    /// The hold.
    pub token: Token,
    /// Its taker.
    pub session: SessionId,
    /// When it expired.
    pub at: Time,
}

/// An open intent (§10.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Intent {
    /// The checkpoint entry that opened it.
    pub eid: Eid,
    /// The session that opened it.
    pub session: SessionId,
    /// 1.12 (§5, §10.2, §20.9 rule 8): its text names a pull request (`words::names_pull_request`,
    /// 1.10's detector): while it is open, its node's work waits for a merge (`Node::merge_wait`),
    /// so the close's `why` already names the blocked release before the release that closes the
    /// intent. Carried by `IntentOpened`, from the checkpoint's text; false in a log written
    /// before 1.12, which replays to the state it wrote (D.7.6). Without it, 1.12's review found,
    /// a worker that announced "push the fix and open a PR" and would pass `intent_done` in its
    /// release, as rule 3 says, was told "else release handoff" for work waiting on its merge.
    #[cfg_attr(feature = "serde", serde(default))]
    pub pr_named: bool,
}

// ------------------------------------------------------------------------------------------------
// Holds (D.4.4)

/// A hold that has not ended.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Hold {
    /// The taker; the hold counts against it (H2).
    pub session: SessionId,
    /// When it was issued.
    pub taken_at: Time,
    /// When the lease ends.
    pub lease_until: Time,
    /// `taken_at`, then the time of each checkpoint or renew.
    pub last_extended: Time,
    /// The lease terms in force at issue (snapshot), so a later `ParamsChanged` cannot break H5. H5
    /// is evaluated with these terms, not with the current params (A-102).
    pub terms: LeaseTerms,
    /// L of §10.1: the requested lease for later extensions (A-8).
    pub l: Dur,
    /// Renews since the last checkpoint (H5).
    pub renews: u32,
    /// A checkpoint happened during this hold.
    pub checkpointed: bool,
    /// 1.4 (§10.1, R4): the outcome the node had when this hold's take reopened it, None when the
    /// node was open. Fixed at issue (`HoldIssued::reopened`); rule 8 of the next calls reads it to
    /// suggest closing the node again with that outcome once its flags are cleared (D.8.26).
    pub reopened: Option<Outcome>,
}

/// The lease parameters in force when a hold was issued.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct LeaseTerms {
    /// `lease.first`.
    pub first: Dur,
    /// `lease.default`.
    pub default: Dur,
    /// `lease.max`.
    pub max: Dur,
    /// `lease.ceiling`.
    pub ceiling: Dur,
    /// `renew.max`.
    pub renew_max: u32,
}

/// `live(h, now)` of §26. An ended hold is not in the map, so liveness is the lease predicate.
#[must_use]
pub fn live(h: &Hold, now: Time) -> bool {
    now < h.lease_until
}

// ------------------------------------------------------------------------------------------------
// Feed entries and the entities born from them (D.4.5)

/// The metadata of a feed entry the core reads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EntryMeta {
    /// The entry's node.
    pub node: NodeId,
    /// Its sequence number there.
    pub seq: Seq,
    /// When it was appended.
    pub at: Time,
    /// Who wrote it.
    pub by: Author,
    /// Its type.
    pub ty: EntryType,
    /// The principal of a `FromUser` entry (whose words) or a `ToUser` entry (to whom), None for
    /// every other type (CS-1, CS-4): the `DigestShown` precondition and the Settled and D1 checks
    /// read it from the state (D.7.4, D.9.2).
    pub principal: Option<PrincipalId>,
}

/// Who wrote an entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Author {
    /// A session.
    Session(SessionId),
    /// The server.
    Server,
}

/// A request (§9.4).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Request {
    /// The conversation node (C1); for a user request, the node it was routed to. Changes only by
    /// merge (N5).
    pub node: NodeId,
    /// The message the request came in (A-61; D.2.4), with the normaliser version it was
    /// normalised with (K2): Q1, unique by `origin.key`, so a repeat of one message is the same
    /// request, and a new message in the same conversation is a new one.
    pub origin: Key,
    /// The conversation key the node was found by: its `where`, or its origin without one; for a
    /// user request its `user:e<n>` key (as `Appended(Request)` carries it, D.7.3). Only a `said`
    /// whose own key is this one answers the request (§9.4, 1.3, D.8.8).
    pub conv: KeyStr,
    /// Set when the origin is `user:e<entry>`.
    pub from_user: Option<UserEntryId>,
    /// `User(p)` exactly when `from_user` is set, with `p` that entry's principal; only a
    /// `to_user` of kind `result` to `p` answers it (CS-4, A-23).
    pub requester: Requester,
    /// The first qualifying answer after the request.
    pub answered_by: Option<Eid>,
    /// Latched once, never cleared (Settled).
    pub settled: Option<Settlement>,
    /// The node it was on before a merge.
    pub moved_from: Option<NodeId>,
}

/// How a request settled.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Settlement {
    /// The decision.
    pub decision: DecisionId,
    /// The answer.
    pub answer: Eid,
}

/// A question to a person (§9.3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PQuestion {
    /// The conversation node (C1); changes only by merge.
    pub node: NodeId,
    /// The conversation key.
    pub conv: KeyStr,
    /// A blocking question escalates to the user (§9.3).
    pub blocking: bool,
    /// The `said` of kind question recorded with it.
    pub said: SaidId,
    /// The state.
    pub state: PqState,
    /// `k` of §25 `open(k)`.
    pub rechecks: u32,
    /// Meaningful while Open.
    pub recheck_at: Time,
    /// Set on expiry or escalation, kept after an answer.
    pub lapse: Option<Lapse>,
    /// The decision that recorded the assumption.
    pub assumed_by: Option<DecisionId>,
    /// The node it was on before a merge.
    pub moved_from: Option<NodeId>,
}

/// §25 states of a question to a person.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PqState {
    /// Waiting for an answer.
    Open,
    /// Expired after its last recheck (non-blocking).
    Expired,
    /// Escalated to a question for the user (blocking).
    Escalated {
        /// The question for the user.
        uq: QuestionId,
    },
    /// Answered.
    Answered {
        /// The answering entry.
        by: Eid,
    },
    /// Superseded by a new question (`again`).
    Superseded {
        /// The new question.
        by: QuestionId,
    },
    /// 1.12 (§9.3): withdrawn by a `talk retract` of it while it was open, for good.
    Withdrawn {
        /// The `retract` entry.
        by: Eid,
    },
}

/// How a question to a person lapsed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Lapse {
    /// It expired.
    Expired,
    /// It escalated.
    Escalated,
}

/// A question between agents (§13). Leaves memory with its note, at `until` (D.4.12): nothing reads
/// it after that; a later `talk answer` is validated against the shell's `entries` table, which adds
/// the node and the asker.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AQuestion {
    /// The node it was asked on.
    pub node: NodeId,
    /// The asking session.
    pub asker: SessionId,
    /// The state.
    pub state: AqState,
}

/// States of a question between agents.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AqState {
    /// Waiting for an answer.
    Open,
    /// Answered.
    Answered {
        /// The answer entry.
        by: Eid,
    },
}

/// A question for the user (§8.4).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UQuestion {
    /// Why it was asked.
    pub source: UqSource,
    /// The node it is about.
    pub about: Option<NodeId>,
    /// The addressee chain, fixed at creation (CS-2, D.8.12): non-empty, no duplicates, at most
    /// `UQ_CHAIN_MAX` stages, the last one `USER_PRINCIPAL` (U1).
    pub to: Vec<PrincipalId>,
    /// The current stage: the question is addressed to `to[stage]` (U1: `stage < to.len()`).
    pub stage: u8,
    /// The current stage's due.
    pub due: Time,
    /// The default text is in the entry.
    pub has_default: bool,
    /// The state.
    pub state: UqState,
    /// The decision that recorded the assumption after a lapse (CS-2), as `PQuestion`'s.
    pub assumed_by: Option<DecisionId>,
}

/// Why a question for the user was asked.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UqSource {
    /// `talk ask {to: user | principal:<p>}` (A-16); `digest`: SHA-256 of the masked text and
    /// options, computed by the shell like the request digest (D.11.5), so that an identical ask for
    /// the same first addressee returns this question (D.8.12).
    Agent {
        /// The asking session.
        session: SessionId,
        /// The question's digest.
        digest: ReqDigest,
    },
    /// A blocking people question (§9.3).
    Escalation {
        /// The people question.
        question: QuestionId,
    },
    /// Every active class refused (§10.3).
    Refusal {
        /// The refused node.
        node: NodeId,
    },
}

/// §25 (CS-2): pending(k) -> pending(k+1) (re-addressed) | answered | defaulted | lapsed. The stage
/// is `UQuestion::stage`, so `Pending` covers every pending(k).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UqState {
    /// Waiting for an answer from `to[stage]`.
    Pending,
    /// Answered.
    Answered {
        /// The answering entry.
        by: Eid,
    },
    /// The default became the answer.
    Defaulted,
    /// Nobody answered by the last stage's due.
    Lapsed,
}

/// A commitment (§9.2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Commitment {
    /// The conversation node (C1); changes only by merge.
    pub node: NodeId,
    /// The conversation key: refreshed and fulfilled only in it (§9.2).
    pub conv: KeyStr,
    /// The last `ack`.
    pub last_ack: Time,
    /// `last_ack + commitment.followup` (snapshot).
    pub followup_at: Time,
    /// One-shot marker for the Commitment deadline; reset by a refreshing ack.
    pub followup_fired: bool,
    /// The state.
    pub state: CommitmentState,
    /// The node it was on before a merge.
    pub moved_from: Option<NodeId>,
}

/// States of a commitment.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommitmentState {
    /// Open.
    Open,
    /// Fulfilled.
    Fulfilled {
        /// The fulfilling `said`.
        by: SaidId,
    },
}

/// A `said` entry (§9.2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Said {
    /// The conversation node (C1); changes only by merge (A-94).
    pub node: NodeId,
    /// The conversation key.
    pub conv: KeyStr,
    /// The kind.
    pub kind: SaidKind,
    /// The recording session.
    pub session: SessionId,
    /// The state.
    pub state: SaidState,
    /// The node it was on before a merge.
    pub moved_from: Option<NodeId>,
    /// Confirmed by its session's release or close, not by a link: `with_reply`, unverified (1.7,
    /// §9.2).
    #[cfg_attr(feature = "serde", serde(default))]
    pub with_reply: bool,
}

/// States of a `said` entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum SaidState {
    /// Recorded, not yet confirmed sent.
    Pending,
    /// Confirmed sent.
    Confirmed,
    /// Its session fell silent before confirming.
    Unconfirmed,
    /// 1.9 (§9.2): not sent, and nobody is to send it: recorded by a session that held neither its
    /// conversation node nor work inside it while another session did (`conversation_held`), or
    /// listed in `unsent` after a later `said` in its conversation replaced it. It raises no flag,
    /// its author's release or close does not confirm it, and silence does not unconfirm it; its
    /// author may still confirm it, if it was sent after all.
    Withheld,
}

/// A decision (§11.1).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Decision {
    /// The node it is on.
    pub node: NodeId,
    /// The requests it settles.
    pub settles: BTreeSet<RequestId>,
    /// The state.
    pub state: DecisionState,
    /// What the decision rests on (§15, CS-22), in the order given, as validated references.
    pub basis: Vec<BasisRef>,
}

/// States of a decision.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DecisionState {
    /// Active.
    Active,
    /// Superseded by a later decision.
    Superseded {
        /// The later decision.
        by: DecisionId,
    },
    /// Retracted.
    Retracted {
        /// The `retract` entry.
        by: Eid,
    },
}

/// One reference of a decision's basis (§15, CS-22). The shell parses `#12@4`, a path with `@4`,
/// `#12`, `e57` or `entry:e57`, and `{external: what, checked_at, recheck?}`; the core validates and
/// stores the result, so a later check can read a year of decisions (events are immutable).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BasisRef {
    /// A node (after `merged_into`, as links resolve, A-88): the version cited, kept for rendering
    /// only, and the node's outcome when the decision was recorded, None while it was open. That
    /// snapshot is what `stale` compares with (D-4, D.8.17).
    Node {
        /// The node.
        id: NodeId,
        /// The version cited.
        version: Option<Version>,
        /// The node's outcome when the decision was recorded.
        life_at: Option<Outcome>,
    },
    /// An entry, usually a decision.
    Entry(Eid),
    /// Something outside the forum; its `what` is a `TextField::Basis` text of the decision's
    /// entry, in the order of the external references. Stored only: its staleness is later (§29,
    /// D-4).
    External {
        /// When it was checked.
        checked_at: Time,
        /// When it should be checked again.
        recheck: Option<Time>,
    },
}

/// What a node or entry reference points at: the key of `ix.cited_by` and of `State::retracted`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BasisTarget {
    /// A node.
    Node(NodeId),
    /// An entry.
    Entry(Eid),
}

/// An objection (§11.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Objection {
    /// The node it is on.
    pub node: NodeId,
    /// The state.
    pub state: ObjectionState,
    /// The node it was on before a merge.
    pub moved_from: Option<NodeId>,
    /// 1.11 (§11.3): the newest deferral of the objection while it was open, a `resolve` entry
    /// that decided nothing (`talk resolve {defer}`): its author could not decide it yet, and said
    /// why. The objection stays open; briefs show it as deferred by that author.
    #[cfg_attr(feature = "serde", serde(default))]
    pub deferred: Option<Eid>,
}

/// States of an objection.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ObjectionState {
    /// Open.
    Open,
    /// Resolved.
    Resolved {
        /// The `resolve` entry.
        by: Eid,
    },
    /// 1.8 (§11.3): resolved as upheld; the objection stood.
    Upheld {
        /// The `resolve` entry.
        by: Eid,
    },
}

/// A `from_user` entry (§8.1). `principal`: whose words these are (CS-1, D.8.11). `at`: the
/// entry's time, for the per-principal message times of D.8.12 (kept here so the index recounts
/// from this map alone).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserEntry {
    /// The recording session.
    pub session: SessionId,
    /// Whose words.
    pub principal: PrincipalId,
    /// When.
    pub at: Time,
    /// How it was routed.
    pub routing: Routing,
}

/// §25: unrouted -> routed (Request, Clarifies) | answered (Answers) | no_action.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Routing {
    /// Not routed yet.
    Unrouted,
    /// Routed to a node as a request.
    Request {
        /// The node.
        node: NodeId,
        /// The request.
        request: RequestId,
    },
    /// Attached to a request as a clarification.
    Clarifies {
        /// The request.
        request: RequestId,
    },
    /// Answers a question for the user.
    Answers {
        /// The question.
        question: QuestionId,
    },
    /// Needs no action.
    NoAction,
}

/// One entry per audience principal (CS-5, D.8.13): `to` is the human it is for, and only a
/// `to_user` to `to` shows it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DigestEntry {
    /// What happened.
    pub item: DigestItem,
    /// For whom.
    pub to: PrincipalId,
    /// Pending or shown.
    pub state: DigestState,
}

/// What a digest entry reports (§8.5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DigestItem {
    /// A people question expired (§9.3).
    Assumption {
        /// The question.
        question: QuestionId,
    },
    /// A user question defaulted (§8.4).
    DefaultApplied {
        /// The question.
        question: QuestionId,
    },
    /// A refused node was dropped (§10.3).
    RefusalDrop {
        /// The node.
        node: NodeId,
        /// The refusal question.
        question: QuestionId,
    },
    /// A user question lapsed (§8.4, CS-2).
    Lapsed {
        /// The question.
        question: QuestionId,
    },
}

/// States of a digest entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DigestState {
    /// Not yet shown to its principal.
    Pending,
    /// Shown by a `to_user`.
    Shown {
        /// The `to_user` entry.
        by: Eid,
    },
}

/// An addressed message: `talk say {to}`, an agent question, or an agent answer (§13). It leaves
/// the state at its `until` (CS-17): the inbox already shows only notes with `until > now`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Note {
    /// The node it was recorded on.
    pub node: NodeId,
    /// The addressee.
    pub to: crate::types::Addressee,
    /// Delivered until then.
    pub until: Time,
    /// Sessions the note is delivered to, fixed when it is recorded (A-49). `Principal(p)` notes
    /// are delivered instead to the sessions that serve `p` or an alias of it when they read (CS-3,
    /// A-18), and `Holders(a)` notes to the sessions that hold in `a`'s branch when they read (D-6);
    /// both are recorded with no recipients.
    pub recipients: BTreeSet<SessionId>,
    /// `holder:` with nobody holding the node: delivered to whoever takes it next.
    pub pending_holder: bool,
    /// Sessions that have received it in `hello` or `talk wait`. A `Holders(a)` note is read once
    /// per session through the session's cursor for the scope `a` instead
    /// (`Session::branch_seen[a]`, D.8.16).
    pub read_by: BTreeSet<SessionId>,
}

// ------------------------------------------------------------------------------------------------
// Refusals, classes, signals, pauses (D.4.6)

/// The refusals of one class on one node (§10.3, CS-7). Every time is a snapshot taken at the
/// `cannot`, so a `ParamsChanged` never changes what a stored refusal excludes (A-103).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Refusal {
    /// `refusal.quorum` when the record was created: the class has refused the node while at least
    /// this many entries are live (`class_refused`, D.8.14).
    pub quorum: u32,
    /// One entry per refusing session; a repeat by the same session renews its entry.
    pub by: BTreeMap<SessionId, RefusalEntry>,
}

/// One session's refusal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct RefusalEntry {
    /// `at + refusal.backoff`: until then the whole class skips the node.
    pub backoff_until: Time,
    /// `at + refusal.ttl`: the entry counts towards the quorum until then.
    pub expires_at: Time,
}

/// A class that asked for work (E1, CS-7).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClassActivity {
    /// The caps a session of the class held at each open `next` (CS-7), mapped to that call's
    /// `at + refusal.window` (the latest call's, a snapshot, A-103). The class is active for node
    /// `n` while some profile here has caps ⊇ `n.needs` (D.8.14). A profile leaves the map when its
    /// window ends (the class's RefusalExpiry deadline, A-27); the map is never empty. Each profile
    /// is its own state row (`Fact::ClassProfile`).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub caps_seen: BTreeMap<BTreeSet<Label>, Time>,
}

/// One signal per (kind, unordered pair): `a < b` always.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct SignalKey {
    /// The kind.
    pub kind: SignalKind,
    /// The smaller node.
    pub a: NodeId,
    /// The larger node.
    pub b: NodeId,
}

/// An active pause (§19). The reason of a pause is the text of its `System(Paused)` entry, which
/// the shell projects (D.11.6) and shows.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pause {
    /// P1: `now < until <= last_renewed + ttl`.
    pub until: Time,
    /// When it was last set or renewed.
    pub last_renewed: Time,
    /// `pause.ttl` at the last renewal (snapshot).
    pub ttl: Dur,
    /// The last session to pause or renew it.
    pub by: SessionId,
    /// That session's class, shown with the pause (CS-9).
    pub class: ClassName,
}

// ------------------------------------------------------------------------------------------------
// Sessions and the retry store (D.4.7)

/// A live or silent session (§3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Session {
    /// The class of its bearer token.
    pub class: ClassName,
    /// When it opened.
    pub started_at: Time,
    /// The time of the last `Touched`: within `presence.resolution` of the last successful call
    /// that appended events (CS-14, D.8.23).
    pub last_seen: Time,
    /// `last_seen + presence.silence` (snapshot).
    pub silence_at: Time,
    /// When the Retire deadline fires (CS-8): the largest `now + session.retain` of the session's
    /// touches (a snapshot that never decreases), so it lies after every hold the session took.
    pub retire_at: Time,
    /// §25 live ⇄ silent. Set by the Silence deadline, cleared by `Touched`. On a settled state
    /// `silent == (now >= silence_at and no hold in holds has session = this one)`, the predicate
    /// T3 reads: a session is never silent while a hold it took is live (§3, 1.4, R27).
    pub silent: bool,
    /// The caps it works with.
    pub caps: BTreeSet<Label>,
    /// Minutes.
    pub horizon: Option<u32>,
    /// The principals the session serves (CS-1), at the agent's word: bound by `hello {principal}`
    /// (at `SessionOpened` or later) and by every `from_user` it records. Empty for a session that
    /// serves no human (a coding agent, a scheduled job). Every alias of a principal it serves
    /// counts as served (`same_person`, D.8.11).
    pub principals: BTreeSet<PrincipalId>,
    /// The highest seq shown to this session per node (§9.2 `seen`).
    pub shown: BTreeMap<NodeId, Seq>,
    /// Per scope node `a`, the newest `holders:<a>` note delivered to this session (D-6, D.8.16).
    pub branch_seen: BTreeMap<NodeId, Eid>,
    /// 1.4 (§20.9 rule 12, R13): the principals this session has recorded a `to_user` to, each as
    /// named (`Appended(ToUser)` inserts its principal; aliases are read through `same_person` when
    /// rule 12 runs). Base state, because a `to_user` entry's metadata leaves memory (D.4.12) while
    /// the session lives; at most one entry per principal it reported to, gone with the session at
    /// retirement.
    pub told: BTreeSet<PrincipalId>,
    /// 1.12 (§10.4): the hold this session last released as `handoff`, and until when `next`,
    /// `hello`'s offers, and `leave_better` keep its node from this session: `lease.first` after
    /// the release, by the hold's terms, or until another hold of the node is issued, which moves
    /// the node's `fence` past the token's (`handed_back`). Set by the release's `Appended`, from
    /// the time its entry carries (`EntryBody::Checkpoint`'s `handed_off_until`); None for a
    /// session that has released nothing as `handoff`, and in a log written before 1.12, whose
    /// releases carry no such time, so that it replays to the state it wrote (D.7.6). In
    /// the lab's seventh round, a worker that handed a lemma off got it back from its next `next`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub handed_off: Option<HandedOff>,
}

/// 1.12 (§10.4): a hold its session released as `handoff`, and until when its node is kept from
/// that session.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HandedOff {
    /// The released hold.
    pub token: Token,
    /// Its release plus `lease.first` of the hold's terms.
    pub until: Time,
}

/// 1.12 (§10.4): node `n` is the one session `x` last released as `handoff`, no hold of it was
/// issued since (its `fence` is still the released token's), and that release was less than
/// `lease.first` ago: `next`, `hello`'s offers, `leave_better`, and rule 13 do not give it back to
/// `x`. A handoff says that someone else should continue now.
#[must_use]
pub fn handed_back(s: &State, x: SessionId, n: NodeId, now: Time) -> bool {
    s.sessions
        .get(&x)
        .and_then(|y| y.handed_off)
        .is_some_and(|h| {
            h.token.node == n
                && now < h.until
                && s.nodes.get(&n).is_some_and(|m| m.fence == h.token.fence)
        })
}

/// 1.12 (§10.4, §11.3): the review item of closed node `n`, its first open objection, was raised by
/// session `x`: `next`, `hello`'s offers, `leave_better`, and rule 13 give that review to another
/// session, whose look decides it. In the lab's seventh round, a session was handed the review of
/// its own minute-old objection and upheld it on its own word.
#[must_use]
pub fn own_review(s: &State, x: SessionId, n: NodeId) -> bool {
    crate::query::review_objection(s, n)
        .and_then(|o| s.entries.get(&o))
        .is_some_and(|m| m.by == Author::Session(x))
}

/// One live `op_id` record (O1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OpRecord {
    /// The request digest of the first execution.
    pub digest: ReqDigest,
    /// The session the first execution ran in (CS-8): a repeat that names another session is
    /// `op_id_conflict`, so a colliding `op_id` never hands one agent another's token (D.8.1).
    pub session: SessionId,
    /// `at + op.retain` of the first execution (a snapshot): the record answers repeats while
    /// `expires_at > now`, and evolve drops it at the first event at or after that time (CS-8).
    pub expires_at: Time,
}

// ------------------------------------------------------------------------------------------------
// Deadlines (D.4.8)

/// Kind order of §17. Derived `Ord` follows the declaration order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum DeadlineKind {
    /// A lease ends.
    Lease,
    /// A time or external wait resolves.
    Wait,
    /// A people question's recheck.
    Recheck,
    /// A question for the user is due.
    UserQuestion,
    /// A commitment's follow-up.
    Commitment,
    /// An occurrence's slot.
    OccurrenceSlot,
    /// An occurrence's window ends.
    OccurrenceWindow,
    /// Refusal entries or a class's caps profiles expire.
    RefusalExpiry,
    /// A node's due.
    Due,
    /// A pause expires.
    Pause,
    /// A session falls silent.
    Silence,
    /// A session retires.
    Retire,
    /// 1.8 (§14): a node has gone untouched for `offer.stale`.
    Stale,
}

/// The owner of a deadline. Derived `Ord` follows the declaration order, then the payload (A-43).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Owner {
    /// Lease.
    Hold(Token),
    /// Wait, OccurrenceSlot, OccurrenceWindow, Due, Pause (scope).
    Node(NodeId),
    /// Recheck (people question), UserQuestion, Commitment.
    Entry(Eid),
    /// RefusalExpiry of the entries of one refusal record (CS-7).
    Refusal(NodeId, ClassName),
    /// RefusalExpiry: a caps profile's window ends (A-27, CS-7).
    Class(ClassName),
    /// Silence, Retire (CS-8).
    Session(SessionId),
}

/// A queued deadline. Fires in derived order `(at, kind, owner)`, as §17 requires.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeadlineKey {
    /// When.
    pub at: Time,
    /// Which kind.
    pub kind: DeadlineKind,
    /// Whose.
    pub owner: Owner,
}

// ------------------------------------------------------------------------------------------------
// Statistics (D.4.9)

/// Statistics: they feed `control health` and the coverage warning only; no decision reads them.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Stats {
    /// Day index -> counters. Pruned by evolve to the days within `capacity.window` of the event's
    /// `at` (deterministic, monotone in `at`, so it commutes with T2).
    pub days: BTreeMap<u64, DayStats>,
    /// Held minutes of the last `DONE_RING` nodes closed Done, oldest first (§17 "median of
    /// completed work", A-41).
    pub done_minutes: Vec<u64>,
    /// Open nodes whose estimate is already in `demand_min`: a node counts once per opening,
    /// however often it is handed off (A-41).
    pub demand_counted: BTreeSet<NodeId>,
    /// The (caps, horizon) profiles of sessions (CS-9), keyed by the caps and the bit length of the
    /// horizon (0..=32; 33 for no horizon), each with the largest horizon seen under that key and
    /// the time it was last seen. `Unreachable` reads it (D.8.22).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub profiles: BTreeMap<(BTreeSet<Label>, u8), ProfileSeen>,
    /// Session starts per UTC minute (`at / 60_000`), pruned by evolve at each `SessionOpened` to
    /// the minutes within `coverage.window` of its `at` (CS-17).
    pub starts: BTreeMap<u64, u32>,
}

/// The largest horizon seen under a profile key (None: no horizon), and when the key was last
/// seen.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProfileSeen {
    /// The largest horizon, in minutes.
    pub max_horizon: Option<u32>,
    /// When the key was last seen.
    pub last_seen: Time,
}

/// One day's counters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DayStats {
    /// Estimated minutes of work that became hold candidates, each node once per opening
    /// (`Stats::demand_counted`).
    pub demand_min: u64,
    /// Time held in holds that made progress: a checkpoint during the hold, a release, or a close
    /// or merge of the node (CS-31).
    pub supply_ms: u64,
    /// Time held in holds that expired, or were moved off, without a checkpoint: the holds that
    /// count as attempts (CS-31).
    pub lost_ms: u64,
    /// `next` calls.
    pub next_calls: u32,
    /// `next` found no candidate at all (CS-31).
    pub next_empty: u32,
    /// Candidates existed, and every one was excluded for the caller.
    pub next_empty_for_you: u32,
    /// The first `UNDERUSED_SESSIONS` distinct sessions that called `next` this day, in the order
    /// of their first call (1.7, §17): `Underused` needs at least that many in the window, and the
    /// union of the days' lists counts them exactly up to that number.
    #[cfg_attr(feature = "serde", serde(default))]
    pub next_callers: [Option<SessionId>; 3],
    /// `|ix.flagged_active|` after the day's last flag change, or, with none, as the day began
    /// (CS-31; carried forward when the day's entry is created, `index::day_stats`).
    pub flagged_end: u32,
}

// ------------------------------------------------------------------------------------------------
// Derived indices (D.4.10)

/// The derived indices. Every index is maintained incrementally by `evolve` (D.7.4) and compared
/// with `recount` by `check_invariants`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ix {
    /// Per-node rollups and flags.
    pub node: BTreeMap<NodeId, NodeIx>,
    /// From `Node::place`.
    pub children: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// Parent -> children with flags.
    pub flagged_children: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// Parent -> the children with `subtree_live > 0` (attended): what `holders:` delivery and
    /// `who {at}` descend through.
    pub attended_children: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// Parent -> the children that `leave_better` may suggest (`unheld_flagged`, D.8.7).
    pub unheld_flagged_children: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// The same nodes by their candidate partition.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub unheld_flagged_by_part: BTreeMap<PartKey, BTreeSet<NodeId>>,
    /// K1: a map, so at most one open carrier per key.
    pub keys_open: BTreeMap<KeyStr, NodeId>,
    /// Open and closed carriers.
    pub keys_all: BTreeMap<KeyStr, BTreeSet<NodeId>>,
    /// Q1, one request per message (A-61).
    pub origins: BTreeMap<KeyStr, RequestId>,
    /// The board's first pages (§18).
    pub unsettled_requests: BTreeSet<RequestId>,
    /// User requests by requester (CS-25 board filter): all of them.
    pub requests_by: BTreeMap<PrincipalId, BTreeSet<RequestId>>,
    /// User requests by requester: the unsettled ones.
    pub unsettled_by: BTreeMap<PrincipalId, BTreeSet<RequestId>>,
    /// The people entries of each node (request, said, heard, question, answer), by seq: what
    /// `moved` lists (D.8.15, CS-17).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub people: BTreeMap<(NodeId, Seq), Eid>,
    /// Entities whose home is the node.
    pub items: BTreeMap<NodeId, NodeItems>,
    /// Decisions that list the request and are not retracted (§9.4: a superseded decision still
    /// counts). A decision leaves the set only at its `DecisionRetracted`.
    pub settling: BTreeMap<RequestId, BTreeSet<DecisionId>>,
    /// Clarifications of each request.
    pub clarifications: BTreeMap<RequestId, BTreeSet<UserEntryId>>,
    /// Nodes with flags.
    pub flagged: BTreeSet<NodeId>,
    /// Flagged nodes that are `next` candidates (in `ix.next.of`, held back or not) or carry a
    /// live hold of their own: the work `FlagsPilingUp` watches (CS-31).
    pub flagged_active: BTreeSet<NodeId>,
    /// The candidate index (CS-16, D.8.5).
    pub next: NextIx,
    /// Series -> open occurrences (K6).
    pub series_open: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// Series -> its flagged occurrences, open or closed, wherever they are in the tree: the
    /// nodes whose classification depends on the series being open (`occ_locked`), which a
    /// close, reopen, or change of `repeat` of the series refreshes (D.7.4). An occurrence moved
    /// out of its series is no longer its child, so `flagged_children` does not find it (WP-D6).
    pub flagged_occurrences: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// 1.4 (§20.4, D.14.5): the nodes whose `agreements` are not empty, open or closed, by id:
    /// what `view agreements` reads, so that its answer does not depend on where in the tree the
    /// agreements sit.
    pub agreements: BTreeSet<NodeId>,
    /// N1.
    pub unrouted: BTreeSet<UserEntryId>,
    /// Pending user questions by current addressee `to[stage]`, in `hello` order (CS-3, CS-33).
    pub pending_uq_by: BTreeMap<PrincipalId, BTreeSet<UqRank>>,
    /// The pending `Refusal { n }` question of each node.
    pub refusal_question: BTreeMap<NodeId, QuestionId>,
    /// Pending digest entries by audience principal, oldest first (CS-5).
    pub pending_digest_by: BTreeMap<PrincipalId, BTreeSet<DigestId>>,
    /// Reverse of `Node::wait`.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub waiters: BTreeMap<WaitKey, BTreeSet<NodeId>>,
    /// Reverse of `depends_on`.
    pub dependents: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// Reverse of `Node::links`: target -> (relation, source), what `view node {view: full}` lists
    /// as incoming links (CS-22).
    pub backlinks: BTreeMap<NodeId, BTreeSet<(LinkRel, NodeId)>>,
    /// The `Active` decisions whose basis names the target (CS-22). A node reference is filed under
    /// `Node(resolve_merged(m))` (D.8.17).
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub cited_by: BTreeMap<BasisTarget, BTreeSet<DecisionId>>,
    /// Reverse of the marks.
    pub marked_by_question: BTreeMap<QuestionId, BTreeSet<NodeId>>,
    /// Each session's holds.
    pub holds_by_session: BTreeMap<SessionId, BTreeSet<Token>>,
    /// The `said` entries each session recorded that are `Pending` or `Unconfirmed` (1.2's
    /// `pending_saids`, widened by 1.3, when rule 2 of the next calls read both states; since 1.5
    /// it reads only the `Pending` ones, §20.9). The Silence arm, a release, and a close read the
    /// `Pending` ones too (D.8.2, D.8.10).
    pub open_saids_by: BTreeMap<SessionId, BTreeSet<SaidId>>,
    /// The unrouted `from_user` entries each session recorded (`UserEntry::session`): rule 5 of the
    /// next calls (§20.9).
    pub unrouted_by: BTreeMap<SessionId, BTreeSet<UserEntryId>>,
    /// Reverse of refusals.
    pub refused: BTreeMap<NodeId, BTreeSet<ClassName>>,
    /// Sessions that are not silent.
    pub live_sessions: BTreeSet<SessionId>,
    /// `|live sessions|` per class (CS-21).
    pub live_by_class: BTreeMap<ClassName, u32>,
    /// `(expires_at, class, op_id)` of every op record: evolve drops their front at each event
    /// (CS-8).
    pub ops_expiry: BTreeSet<(Time, ClassName, OpId)>,
    /// `(until, note)` of every note: evolve drops their front at each event (CS-17).
    pub notes_until: BTreeSet<(Time, NoteId)>,
    /// `(time, entity)` of `State::leaving`: evolve drops the front (D.4.12).
    pub leaving_by_time: BTreeSet<(Time, EntityRef)>,
    /// Bound, unread notes of the sessions in `sessions` (CS-8).
    pub inbox: BTreeMap<SessionId, BTreeSet<NoteId>>,
    /// `holder:` notes waiting for a taker.
    pub pending_holder: BTreeMap<NodeId, BTreeSet<NoteId>>,
    /// Live `holders:<a>` notes by their scope `a`, read when a session's inbox is read (D-6).
    pub branch_notes: BTreeMap<NodeId, BTreeSet<NoteId>>,
    /// `(until, note)` of `principal:` notes, by principal (CS-3).
    pub user_notes: BTreeMap<PrincipalId, BTreeSet<(Time, NoteId)>>,
    /// Reverse of `principal_ids`.
    pub principal_key: BTreeMap<PrincipalId, PrincipalKey>,
    /// The directory (D.2.2, D-3): each alias principal -> its person node (an open child of PEOPLE
    /// carrying `person:<key>`).
    pub person_of: BTreeMap<PrincipalId, NodeId>,
    /// Each person node -> its alias principals.
    pub aliases_of: BTreeMap<NodeId, BTreeSet<PrincipalId>>,
    /// `(last seen, key)` of `stats.profiles`, for its prune (D.4.9).
    pub profiles_by_time: BTreeSet<(Time, (BTreeSet<Label>, u8))>,
    /// Each class's caps profiles by window end, for its RefusalExpiry deadline (D.4.6).
    pub class_ends: BTreeMap<ClassName, BTreeSet<(Time, BTreeSet<Label>)>>,
    /// T4: equals `recount_deadlines`.
    pub deadlines: BTreeSet<DeadlineKey>,
    /// For O(log n) move and cancel.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub deadline_at: BTreeMap<(DeadlineKind, Owner), Time>,
}

/// What a wait names: people and user questions, or a node.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WaitKey {
    /// A `{node}` wait.
    Node(NodeId),
    /// A `{question}` wait.
    Question(QuestionId),
}

/// The `hello` order of an addressee's pending questions (CS-33): the `about` node's priority,
/// highest first (0 without `about`), then the due, then the id. Derived `Ord`, field order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UqRank {
    /// The negated priority of the `about` node.
    pub neg_priority: i64,
    /// The current stage's due.
    pub due: Time,
    /// The question.
    pub q: QuestionId,
}

/// A candidate for `next` (§10.4). Derived `Ord`: Node < Status < Route (the kind rank).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Cand {
    /// A node to take.
    Node(NodeId),
    /// A status item: `{status: node}`, no hold.
    Status(NodeId),
    /// A routing item: an unrouted user entry.
    Route(UserEntryId),
}

/// The candidate index of `next` (CS-16, D.8.5). Every candidate of the classification is in `of`.
/// One that no session-independent exclusion holds back is also in its partition, in key order, so
/// that a session's best candidate is a range query; one held back (an open `depends_on` target,
/// or a pause over it) is in `held` instead, ready to move back.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NextIx {
    /// Every candidate and where it sits.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub of: BTreeMap<Cand, CandAt>,
    /// The candidates nothing holds back, by partition.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub parts: BTreeMap<PartKey, BTreeSet<NextKey>>,
    /// The candidates held back for every caller.
    #[cfg_attr(feature = "serde", serde(with = "crate::serde_pairs"))]
    pub held: BTreeMap<Cand, Held>,
    /// Node and Status entries of `of`: the backlog (CS-21).
    pub nodes: u32,
    /// 1.4 (§10.4, R27): the open nodes that the classification would make a `Cand::Node` but for
    /// an unresolved wait (the row "n open, waiting(n)" of D.8.5), so that an empty `next` counts
    /// them (`EmptyWhy::waiting`). Kept at the same flag and candidate refresh as `of`.
    pub waiting: BTreeSet<NodeId>,
    /// The non-empty partitions' needs sets as a trie over their sorted labels, each end holding
    /// the partitions' est buckets (D.8.5).
    pub needs_trie: NeedsTrie,
}

/// A trie over sorted needs labels.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NeedsTrie {
    /// The est buckets of the partitions whose needs end here.
    pub buckets: BTreeSet<u8>,
    /// The next labels.
    pub next: BTreeMap<Label, NeedsTrie>,
}

/// Where a candidate sits. `since` repeats `State::since[c]`, its source.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CandAt {
    /// When it became a candidate.
    pub since: Time,
    /// Its partition.
    pub part: PartKey,
    /// Its key in the partition.
    pub key: NextKey,
}

/// A partition: the candidate's `needs`, and its `est` bucket: 0 without `est`, else the bit
/// length of `est` (1..=32), so a bucket below the bit length of a session's horizon fits it
/// entirely. Routing items are in the partition with no needs and bucket 0.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PartKey {
    /// The needs.
    pub needs: BTreeSet<Label>,
    /// The est bucket.
    pub est_bucket: u8,
}

/// Derived `Ord`, field order: the tier; the hour group `⌊since / 1 h⌋ − priority` (smaller is
/// older, so it orders like −aged, CS-12); the ring position `fnv_id(id)` (D.2.6); the id; the kind
/// rank.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct NextKey {
    /// 0, 1, or 2.
    pub tier: u8,
    /// `⌊since / 1 h⌋ − priority`.
    pub group: i64,
    /// `fnv_id(id)`.
    pub fnv: u64,
    /// The node id or entry id.
    pub id: u64,
    /// Node 0, Status 1, Route 2.
    pub kind: u8,
}

/// Why a candidate is held back for every caller: an open `depends_on` target, and the nearest
/// paused ancestor-or-self, if any.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Held {
    /// An open `depends_on` target.
    pub depends: bool,
    /// The nearest paused ancestor-or-self.
    pub paused_by: Option<NodeId>,
    /// 1.11 (§10.4, §12): a series whose open occurrence waits for its slot.
    #[cfg_attr(feature = "serde", serde(default))]
    pub before_slot: bool,
}

/// Per-node derived values.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeIx {
    /// The node's flags.
    pub flags: FlagSet,
    /// ROOT = 0.
    pub depth: u32,
    /// Children.
    pub children: u32,
    /// Open children with origin Agent or Ancestor (L1, CS-10); L1 does not compare it at PEOPLE
    /// and TEAMS (A-113).
    pub open_agent_children: u32,
    /// Open children.
    pub open_children: u32,
    /// Children with a live hold on themselves.
    pub held_children: u32,
    /// Children with flags.
    pub flagged_children: u32,
    /// Live holds on the node itself.
    pub live: u32,
    /// Live holds on the node or any descendant ("attended").
    pub subtree_live: u32,
    /// The descendants by state (CS-32).
    pub sub: Subtree,
}

/// The node's descendants, not the node itself, by state (CS-32): progress below a node (how many
/// sub-lemmas are proved, refuted, or open), exact however large the subtree, where `view list`
/// stops at `SCAN_MAX` visits. `u64`: ROOT counts every node of the forum.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Subtree {
    /// Open.
    pub open: u64,
    /// Closed with outcome `done`.
    pub done: u64,
    /// Closed with outcome `failed`.
    pub failed: u64,
    /// Closed with any other outcome (dropped, superseded, missed, …).
    pub other: u64,
    /// With flags, open or closed.
    pub flagged: u64,
}

/// Every entity whose home is the node, and the open subsets that flags, answers, and brief
/// sections read (CS-20).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeItems {
    // every entity whose home is the node: history views, merges, and recounts
    /// Requests.
    pub requests: BTreeSet<RequestId>,
    /// People questions.
    pub pquestions: BTreeSet<QuestionId>,
    /// Agent questions.
    pub aquestions: BTreeSet<QuestionId>,
    /// User questions with `about = Some(n)` (A-92).
    pub uquestions: BTreeSet<QuestionId>,
    /// Commitments.
    pub commitments: BTreeSet<CommitmentId>,
    /// `said` entries.
    pub saids: BTreeSet<SaidId>,
    /// Decisions.
    pub decisions: BTreeSet<DecisionId>,
    /// Objections.
    pub objections: BTreeSet<ObjectionId>,
    // the open subsets that flags, answers, and brief sections read (CS-20)
    /// `settled = None`.
    pub unsettled: BTreeSet<RequestId>,
    /// The requesters of the unsettled user requests on the node, each with its oldest such
    /// request, ordered by it: `(r, p)`. `chain` and `audience` read the first few (D.8.12).
    pub requesters_by_age: BTreeSet<(RequestId, PrincipalId)>,
    /// `settled = None` and `answered_by = None`, by `Request::conv`: a `said` answers only its own
    /// conversation's (§9.4, 1.3), so it reads one entry, not the node's other conversations.
    pub unanswered: BTreeMap<KeyStr, BTreeSet<RequestId>>,
    /// Unsettled and unanswered, and listed by a decision that is not retracted (`ix.settling`
    /// non-empty): owed a reply (rule 6 of §20.9).
    pub owed_reply: BTreeSet<RequestId>,
    /// Unsettled with an answer, so no such decision lists it: owed a decision (rules 7 and 8).
    pub owed_decision: BTreeSet<RequestId>,
    /// People questions `Open`, by conversation.
    pub open_pq: BTreeMap<KeyStr, BTreeSet<QuestionId>>,
    /// `Open`.
    pub open_commitments: BTreeSet<CommitmentId>,
    /// `Pending` or `Unconfirmed`.
    pub open_saids: BTreeSet<SaidId>,
    /// `Unconfirmed`.
    pub unconfirmed: BTreeSet<SaidId>,
    /// `Open`.
    pub open_objections: BTreeSet<ObjectionId>,
    /// `Active`.
    pub active_decisions: BTreeSet<DecisionId>,
    /// `Active`, with a stale reference (D-4).
    pub stale_basis: BTreeSet<DecisionId>,
}

// ------------------------------------------------------------------------------------------------
// Finished entities leave memory (D.4.12)

/// An entity the core may drop from memory and restore (D-5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EntityRef {
    /// A request.
    Request(RequestId),
    /// A people question.
    PQuestion(QuestionId),
    /// A user question.
    UQuestion(QuestionId),
    /// A commitment.
    Commitment(CommitmentId),
    /// A `said` entry.
    Said(SaidId),
    /// A decision.
    Decision(DecisionId),
    /// An objection.
    Objection(ObjectionId),
    /// A user entry.
    UserEntry(UserEntryId),
    /// A digest entry.
    Digest(DigestId),
    /// Entry metadata.
    Meta(Eid),
}

/// One entity brought back by `Ev::Restored`, with the metadata of the entry it was born from.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Restored {
    /// A request.
    Request(RequestId, Request, EntryMeta),
    /// A people question.
    PQuestion(QuestionId, PQuestion, EntryMeta),
    /// A user question.
    UQuestion(QuestionId, UQuestion, EntryMeta),
    /// A commitment.
    Commitment(CommitmentId, Commitment, EntryMeta),
    /// A `said` entry.
    Said(SaidId, Said, EntryMeta),
    /// A decision.
    Decision(DecisionId, Decision, EntryMeta),
    /// An objection.
    Objection(ObjectionId, Objection, EntryMeta),
    /// A user entry.
    UserEntry(UserEntryId, UserEntry, EntryMeta),
    /// A digest entry.
    Digest(DigestId, DigestEntry, EntryMeta),
    /// Entry metadata alone.
    Meta(Eid, EntryMeta),
}

impl State {
    /// `occ_locked(o)` of D.4.3: a closed occurrence of a series that is still open and still a
    /// series cannot be reopened or taken (§12, A-13), and is never a candidate. (For an open
    /// occurrence the predicate holds too; callers ask it of closed nodes.)
    #[must_use]
    pub fn occ_locked(&self, o: NodeId) -> bool {
        let Some(node) = self.nodes.get(&o) else {
            return false;
        };
        let Role::Occurrence(occ) = &node.role else {
            return false;
        };
        self.nodes
            .get(&occ.series)
            .is_some_and(|x| x.is_open() && matches!(x.role, Role::Series(_)))
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;
    use crate::types::{DAY, ROOT, USER};

    #[test]
    fn the_retention_is_seven_days() {
        assert_eq!(FINISHED_RETAIN, Dur(7 * DAY.0));
    }

    #[test]
    fn counters_start_at_the_values_of_d_4_2() {
        let c = Counters::default();
        assert_eq!(c.next_node, NodeId(8));
        assert_eq!(c.next_eid, Eid(1));
        assert_eq!(c.next_session, SessionId(1));
        assert_eq!(c.next_principal, PrincipalId(1));
        assert_eq!(c.events, 0);
    }

    #[test]
    fn deadline_kinds_fire_in_the_order_of_section_17() {
        use DeadlineKind::*;
        let order = [
            Lease,
            Wait,
            Recheck,
            UserQuestion,
            Commitment,
            OccurrenceSlot,
            OccurrenceWindow,
            RefusalExpiry,
            Due,
            Pause,
            Silence,
            Retire,
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]));
        let key = |at, kind, owner| DeadlineKey {
            at: Time(at),
            kind,
            owner,
        };
        assert!(key(1, Retire, Owner::Session(SessionId(1))) < key(2, Lease, Owner::Node(ROOT)));
        assert!(key(1, Lease, Owner::Session(SessionId(9))) < key(1, Wait, Owner::Node(ROOT)));
        assert!(key(1, Due, Owner::Node(NodeId(9))) < key(1, Due, Owner::Entry(Eid(1))));
    }

    #[test]
    fn a_hold_is_live_until_its_lease_ends() {
        // §26's `live(h, now)`, behind H1, H2, and H4: live strictly before `lease_until`.
        let lease_until = Time(10 * 60_000);
        let hold = Hold {
            session: SessionId(1),
            taken_at: Time(0),
            lease_until,
            last_extended: Time(0),
            terms: LeaseTerms {
                first: Dur(15 * 60_000),
                default: Dur(20 * 60_000),
                max: Dur(2 * 3_600_000),
                ceiling: Dur(8 * 3_600_000),
                renew_max: 3,
            },
            l: Dur(20 * 60_000),
            renews: 0,
            checkpointed: false,
            reopened: None,
        };
        assert!(live(&hold, Time(0)));
        assert!(live(&hold, Time(lease_until.0 - 1)));
        assert!(!live(&hold, lease_until));
        assert!(!live(&hold, Time(lease_until.0 + 1)));
        assert!(!live(&hold, Time(u64::MAX)));
    }

    #[test]
    fn candidates_order_by_kind() {
        assert!(Cand::Node(NodeId(9)) < Cand::Status(NodeId(1)));
        assert!(Cand::Status(NodeId(9)) < Cand::Route(Eid(1)));
    }

    fn node(parent: NodeId, role: Role, origin: Origin) -> Node {
        Node {
            place: Some(Place {
                parent,
                name: Name::new("x").unwrap(),
            }),
            origin,
            created_at: Time(0),
            title: Text::default(),
            body: Text::default(),
            fields: BTreeMap::new(),
            files: BTreeMap::new(),
            status: Text::default(),
            digest: Text::default(),
            agreements: Text::default(),
            agreements_at: None,
            kind: None,
            tags: BTreeSet::new(),
            capacity: 1,
            priority: 0,
            est: None,
            needs: BTreeSet::new(),
            due: None,
            due_fired: false,
            owners: BTreeSet::new(),
            keys: BTreeMap::new(),
            links: BTreeSet::new(),
            depends_on: BTreeSet::new(),
            wait: None,
            role,
            life: Life::Open,
            version: Version(1),
            digest_version: Version(1),
            fence: Fence(0),
            seq: Seq(0),
            content_version: Version(1),
            attempts: 0,
            checkpoints: 0,
            last_checkpoint: None,
            expired_hold: None,
            open_intent: None,
            recheck_marks: BTreeSet::new(),
            assume_marks: BTreeSet::new(),
            held_ms: 0,
            people_left: Seq(0),
            left: 0,
            touched_at: Some(Time(0)),
            stale_fired: false,
            retracted_decision: None,
            pr_named: false,
        }
    }

    #[test]
    fn containers_and_the_directory() {
        let agent = Origin::Agent(SessionId(1));
        assert!(node(PEOPLE, Role::Plain, agent).is_directory());
        assert!(node(TEAMS, Role::Plain, agent).is_container());
        assert!(!node(ROOT, Role::Plain, agent).is_container());
        assert!(node(USER, Role::Plain, Origin::Reserved).is_container());
        let series = Role::Series(Series {
            repeat: Repeat {
                cron_text: Text::default(),
                cron: forum_cron::parse_cron("0 7 * * *").unwrap(),
                tz: forum_cron::TzTable {
                    name: alloc::string::String::from("UTC"),
                    tzdb_version: alloc::string::String::new(),
                    initial: 0,
                    transitions: Vec::new(),
                },
                run_within: Dur(1),
            },
            last_missed: false,
        });
        assert!(node(ROOT, series, agent).is_container());
    }
}
