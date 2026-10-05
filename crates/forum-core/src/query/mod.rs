//! Read-only queries for the shell (design D.8.25): the signatures and result types are frozen at
//! WP-D1. WP-D3's bodies live in the submodules: `reads` (`op_lookup`, `precheck`, `resolve`,
//! `resolve_merged`, `path_of`, `is_under`, `archived`, `find_key`, `list`, `backlinks`,
//! `board_scope`, `principal_key`, `who`, `holders`, `inbox_ready`), `status` (`status_line`, and
//! 1.4's `checkpoint_by`), `hello`, `next` (`ranked`), `brief`, `leave_better`, and `board`. The
//! bodies of WP-D5b (`inbox`), WP-D6 (`health`, `m1b_gaps`, in `crate::health`), and WP-D7
//! (`guide`) arrive with their packages; until then each returns an empty value.
//!
//! Queries never mutate the state; the commands that must record what they showed (`Shown`,
//! `InboxRead`, `NextCalled`) do so through their own events.

mod board;
mod brief;
pub mod guide;
mod hello;
pub(crate) mod leave_better;
pub(crate) mod next;
pub(crate) mod reads;
mod status;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::cmd::{BoardState, NextQuery};
use crate::health::{Health, MoverGap};
use crate::reply::{AgreementItem, EmptyWhy, Total, WhyThis};
use crate::state::{Cand, ExpiredHold, Intent, State, Subtree, UqState};
use crate::types::{
    ClassName, CommitmentId, DecisionId, DigestId, Eid, Flag, FlagSet, Label, LinkRel, NodeId,
    NoteId, ObjectionId, PrincipalId, QuestionId, RequestId, Requester, SaidId, SessionId, Text,
    Time, Token, UserEntryId, Version,
};

pub use crate::helpers::agents::{
    Voice, conversation_holder, deliverable, inbox, voice, voice_cap,
};
pub use board::board;
pub use brief::brief;
pub use guide::guide;
pub use hello::hello;
pub use leave_better::leave_better;
pub use reads::{
    agreements, archived, backlinks, board_scope, find_key, gone_intent, holders, inbox_ready,
    is_under, list, no_basis, op_lookup, path_of, precheck, principal_key, resolve, resolve_merged,
    who,
};
pub use status::{checkpoint_by, status_line};

/// Entries the `moved` warning lists, newest first (CS-17).
pub const MOVED_MAX: usize = 20;
/// Notes a `hello` or a `talk wait` delivers at most, oldest first (1.5, §13, §22, D.8.16): only
/// those are read; the rest wait for the next call.
pub const INBOX_DELIVER_MAX: usize = 10;

/// The notes the response to a write delivers, at most (1.7, §13, §22): oldest first, marked read
/// in the write's own step; the status line's `inbox` counts the rest.
pub const WRITE_NOTES_MAX: usize = 2;
/// Sessions `who` and `hello.present` list (CS-21).
pub const WHO_MAX: usize = 10;
/// Entries a filtered scan visits before it says `≥k`.
pub const SCAN_MAX: u32 = 2_000;
/// Items per list and page of what rests on a node (CS-22).
pub const BACKLINKS_MAX: usize = 20;
/// `view feed`, `user_log`, and `history` pages (A-78).
pub const PAGE_DEFAULT: u32 = 50;
/// The largest page.
pub const PAGE_MAX: u32 = 200;

// ------------------------------------------------------------------------------------------------
// Cost meters (§18)

/// Cost meter for queries (§18). Production passes `NoMeter`; the cost test counts visits.
pub trait Meter {
    /// Counts `n` visits.
    fn tick(&mut self, n: u64);
}

/// Counts nothing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct NoMeter;

impl Meter for NoMeter {
    fn tick(&mut self, _n: u64) {}
}

/// Counts every visit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct CountMeter(pub u64);

impl Meter for CountMeter {
    fn tick(&mut self, n: u64) {
        self.0 = self.0.saturating_add(n);
    }
}

// ------------------------------------------------------------------------------------------------
// The queries (D.8.25)

/// The first `k` candidates for `sess` in the order of D.8.5, read from `ix.next` (CS-16);
/// `spec::ranked` sorts every candidate instead and is the oracle (D.12.3). A session that does not
/// exist has none.
#[must_use]
pub fn ranked<M: Meter>(
    s: &State,
    sess: SessionId,
    now: Time,
    q: &NextQuery,
    k: usize,
    m: &mut M,
) -> (Vec<Ranked>, EmptyWhy) {
    match next::Caller::of(s, sess) {
        Some(c) => next::rank(s, &c, now, q, k, m),
        None => (Vec::new(), EmptyWhy::default()),
    }
}

/// Computed by the writer at most once per `HEALTH_REFRESH` and cached whole by the shell
/// (D.8.22). Body: WP-D6 (`health::compute`).
#[must_use]
pub fn health(s: &State, now: Time) -> Health {
    crate::health::compute(s, now)
}

/// The objects with no mover that `health` reports as `NoMover` (D.9.3). Body: WP-D6
/// (`health::gaps`).
#[must_use]
pub fn m1b_gaps(s: &State, now: Time) -> Vec<MoverGap> {
    crate::health::gaps(s, now)
}

// ------------------------------------------------------------------------------------------------
// Result types

/// The status line's core part. Its health segment comes from the shell's cache of
/// `health(s, now)` (D.8.22, D.11.2), not from this query, so a call never recomputes health.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StatusLine {
    /// ROOT is paused (A-44), with its source (CS-9).
    pub root_pause: Option<PauseView>,
    /// Other active pauses covering the session's holds.
    pub hold_pauses: Vec<PauseView>,
    /// 1.4 (§20.1, R8): the time of the response, the `now` the call ran at (T1), so an agent with
    /// no clock of its own can tell what is due and when.
    pub now: Time,
    /// The session's class.
    pub class: ClassName,
    /// The session.
    pub session: SessionId,
    /// The live holds, in token order.
    pub holds: Vec<HoldStatus>,
    /// Unread notes.
    pub inbox: u32,
    /// Pending questions addressed now to the session's principals (CS-3); None for a session that
    /// serves no principal, whose status line then omits the segment.
    pub user_decisions: Option<u32>,
}

/// One hold on the status line: its node, its lease's end, and, 1.4 (§20.1, R8), when its
/// checkpoint is due, `checkpoint_by(h)`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HoldStatus {
    /// The node.
    pub node: NodeId,
    /// When the lease ends.
    pub lease_until: Time,
    /// When its checkpoint is due.
    pub checkpoint_by: Time,
}

/// A pause as the status line, an empty `next`, and `health` show it (CS-9): the shell adds the
/// reason from the pause's `System(Paused)` entry (D.4.6).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PauseView {
    /// The scope.
    pub scope: NodeId,
    /// The session that set or renewed it.
    pub by: SessionId,
    /// That session's class.
    pub class: ClassName,
    /// Until when.
    pub until: Time,
}

/// What `session hello` shows.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HelloData {
    /// The session's holds.
    pub holds: Vec<(Token, NodeId, Time)>,
    /// 1.4 (§20.3, R25): the session's caps, the token's default caps and those `hello` passed
    /// (§3, 1.6); never dropped by the budget.
    pub caps: BTreeSet<Label>,
    /// Its notes: the `INBOX_DELIVER_MAX` oldest of its inbox (1.5, §13), those a `hello` delivers.
    pub inbox: Vec<NoteId>,
    /// `ranked` with `k = 3`, nothing taken.
    pub offers: Vec<Ranked>,
    /// 1.4 (§20.3, R25): when `offers` is empty, the `EmptyWhy` of that read, as an empty `next`
    /// gives it (D.8.5): what excluded the candidates, and the nodes that wait.
    pub offers_why: Option<EmptyWhy>,
    /// 1.4 (§11.2, §20.3, D.14.5): the forum map and the house rules.
    pub orientation: Orientation,
    /// `who(None)`: at most `WHO_MAX`, with counts (CS-21).
    pub present: WhoPage,
    /// Whom the session serves (CS-1), shown by key.
    pub principals: Vec<PrincipalId>,
    /// Only what is addressed to the session's principals or their aliases, with a total (CS-3):
    /// questions whose current addressee is one of them, in `UqRank` order (CS-33). Empty for a
    /// session that serves none.
    pub user_decisions: UserItems<QuestionId>,
    /// Digest entries pending for them, oldest first, with the number of older ones that left the
    /// pending list unshown (`digest_left`, D.8.13).
    pub user_digest: UserItems<DigestId>,
}

/// `hello.orientation = {map, house_rules}` (1.4, §11.2, §20.3, D.14.5): the digest of `/`, which
/// agents keep as the forum map (what lives where), and the agreements of `/`, the swarm's house
/// rules. Dropped by the budget after `present` and before `offers`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Orientation {
    /// The digest of `/`.
    pub map: Text,
    /// The agreements of `/`.
    pub house_rules: Text,
}

/// A page of `view agreements` (1.4, §20.4, D.14.5): the nodes of the subtree that have
/// agreements, by id, each with the approvals of its current content and its agreements' last
/// change, and how many there are (`≥k` when the scan stopped, §18).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AgreementsPage {
    /// The page.
    pub items: Vec<AgreementItem>,
    /// How many nodes of the subtree have agreements.
    pub total: Total,
}

/// `count` is the number of pending items over the session's principals and their aliases,
/// exactly (U1); `items` the first `HELLO_USER_ITEMS` of them. `older` is None for questions; for
/// the digest, the entries that left the pending list unshown after `DIGEST_RETAIN` (§8.5, §20.3),
/// never counted in `count`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserItems<T> {
    /// How many are pending.
    pub count: u32,
    /// The first few.
    pub items: Vec<T>,
    /// For the digest: how many left the pending list unshown.
    pub older: Option<u64>,
}

/// One ranked candidate.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ranked {
    /// The candidate.
    pub cand: Cand,
    /// Why it ranks there.
    pub why: WhyThis,
}

/// The carriers of a key.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FindResult {
    /// The open carrier.
    pub open: Option<NodeId>,
    /// The closed carriers: by id, newest first.
    pub closed: Vec<NodeId>,
}

/// The filter of `view list`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListFilter {
    /// The subtree.
    pub under: Option<NodeId>,
    /// The kind.
    pub kind: Option<Label>,
    /// Tags every listed node carries.
    pub tags: BTreeSet<Label>,
    /// The status.
    pub status: Option<Text>,
    /// Field values.
    pub fields: BTreeMap<Text, Text>,
    /// Flags every listed node carries.
    pub flags: FlagSet,
    /// None: open + not archived (AR).
    pub closed: Option<bool>,
    /// Page size.
    pub limit: u32,
}

/// `under`: the subtree counts of the filter's `under` node, or ROOT's, exact whatever the scan
/// visited (CS-32); the shell shows each listed node's own `sub` from `ix.node` beside it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListPage {
    /// The nodes.
    pub nodes: Vec<NodeId>,
    /// How many match.
    pub total: Total,
    /// The subtree counts.
    pub under: Subtree,
}

/// Incoming links by relation, the nodes that depend on `n`, and the active decisions that cite
/// `n` or one of its decisions (CS-22), each with its total (§18).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Backlinks {
    /// Incoming links.
    pub links: Vec<(LinkRel, NodeId)>,
    /// How many.
    pub links_total: Total,
    /// Dependent nodes.
    pub dependents: Vec<NodeId>,
    /// How many.
    pub dependents_total: Total,
    /// Citing decisions.
    pub cited_by: Vec<DecisionId>,
    /// How many.
    pub cited_by_total: Total,
}

/// One row of the request board.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BoardRow {
    /// The request.
    pub request: Option<RequestId>,
    /// The user entry.
    pub entry: Option<UserEntryId>,
    /// The node.
    pub node: Option<NodeId>,
    /// An unrouted entry: `User(its principal)` (CS-25).
    pub requester: Requester,
    /// The row's state.
    pub state: BoardState,
}

/// A page of the request board (D.8.8).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BoardPage {
    /// The rows.
    pub rows: Vec<BoardRow>,
    /// How many.
    pub total: Total,
}

/// A present session.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Present {
    /// The session.
    pub session: SessionId,
    /// Its class.
    pub class: ClassName,
    /// Its holds.
    pub holds: Vec<NodeId>,
    /// When it was last seen.
    pub last_seen: Time,
}

/// `who` and `hello.present` (§18: shown and total). `by_class` only without `at`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WhoPage {
    /// The sessions.
    pub items: Vec<Present>,
    /// Live sessions per class.
    pub by_class: Vec<(ClassName, u32)>,
    /// How many.
    pub total: Total,
}

impl Default for WhoPage {
    fn default() -> WhoPage {
        WhoPage {
            items: Vec::new(),
            by_class: Vec::new(),
            total: Total::Exact(0),
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Brief data (D.8.6)

/// The data of a brief; the shell renders it within `brief.budget` (D.11.7).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BriefData {
    /// The node.
    pub node: NodeId,
    /// Its version.
    pub version: Version,
    /// Its digest version.
    pub digest_version: Version,
    /// Holder and `lease_until`; no tokens.
    pub holders: Vec<(SessionId, Time)>,
    /// The header's rollup (§5): the children open, held, and flagged, and the subtree's counts by
    /// state (CS-32), read from `ix.node[n]` at O(1).
    pub rollup: Rollup,
    /// §7: predecessors first, along the chain.
    pub continues: Vec<Predecessor>,
    /// How many predecessors.
    pub continues_total: Total,
    /// Exactly nine, in §10.4 order.
    pub sections: Vec<Section>,
    /// 1.8 (§10.4): the nodes this one rests on, through `depends_on` or its active decisions'
    /// basis, followed through `merged_into`, that are closed `failed` or `refuted`, by id, at most
    /// 100. Appended with a default (D.7.6).
    #[cfg_attr(feature = "serde", serde(default))]
    pub rests_on: Vec<RestsOn>,
}

/// 1.8 (§10.4): a node a brief's node rests on whose result does not stand.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RestsOn {
    /// The node, after `merged_into`.
    pub node: NodeId,
    /// Its outcome: `failed`, or `refuted`.
    pub outcome: crate::types::Outcome,
    /// How the brief's node rests on it.
    pub via: RestsVia,
}

/// How a node rests on another (1.8, §10.4).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RestsVia {
    /// It depends on it (`depends_on`).
    DependsOn,
    /// An active decision of it cites it.
    Basis(DecisionId),
}

/// The rollup of a node's header (§5, CS-32).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub struct Rollup {
    /// Open children.
    pub open_children: u32,
    /// Children with a live hold.
    pub held_children: u32,
    /// Children with flags.
    pub flagged_children: u32,
    /// The descendants by state.
    pub sub: Subtree,
}

/// A predecessor along the chain of `continues` links; the summary text is the shell's.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Predecessor {
    /// The node.
    pub node: NodeId,
    /// Its unsettled requests.
    pub unsettled: Vec<RequestId>,
}

/// One section of a brief.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Section {
    /// Which section.
    pub name: SectionName,
    /// Its items, newest first, at most 100.
    pub items: Vec<BriefItem>,
    /// How many.
    pub total: u32,
}

/// The nine sections of a brief, in §10.4 order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum SectionName {
    /// 1.
    Flags,
    /// 2: unfinished intent and unconfirmed `said`.
    Unfinished,
    /// 3.
    SaidAndCommitments,
    /// 4.
    DecisionsAndAgreements,
    /// 5.
    UnsettledRequests,
    /// 6.
    Objections,
    /// 7.
    Digest,
    /// 8.
    LastCheckpoint,
    /// 9.
    Everything,
}

/// One item of a brief section.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BriefItem {
    /// A flag.
    Flag(Flag),
    /// The open intent.
    Intent(Intent),
    /// The shell marks `Unconfirmed` ones.
    Said(SaidId),
    /// An open commitment.
    Commitment(CommitmentId),
    /// A decision.
    Decision(DecisionId),
    /// Inherited agreements carry their source.
    Agreement {
        /// The node whose agreements these are.
        source: NodeId,
    },
    /// An answer that arrived after its question expired.
    LateAnswer {
        /// The question.
        question: QuestionId,
        /// The answer.
        answer: Eid,
        /// The decision that recorded the assumption.
        assumption: Option<DecisionId>,
    },
    /// A question for the user about this node (A-92), in its state: pending (with its current
    /// addressee), answered, defaulted, or lapsed (CS-2).
    UserQuestion {
        /// The question.
        question: QuestionId,
        /// Its state.
        state: UqState,
    },
    /// An unsettled request.
    Request(RequestId),
    /// A clarification of a request.
    Clarification(UserEntryId),
    /// An open objection.
    Objection(ObjectionId),
    /// The node's digest.
    Digest,
    /// The last checkpoint.
    Checkpoint(Eid),
    /// 1.4 (§10.4, re-run R30): the last hold that expired after the node's last checkpoint
    /// without a checkpoint of its own, in section 2: an outside action may have happened.
    ExpiredHold(ExpiredHold),
    /// Any other entry.
    Entry(Eid),
}

/// 1.9 (§10.4, §11.3): the objection a review of `n` is about: `n`'s first open objection, while
/// `n` is closed. `next` offers such a node as `{review: n, objection}`, with no hold, and a take
/// of it is refused with `closed`.
#[must_use]
pub fn review_objection(s: &State, n: NodeId) -> Option<crate::types::ObjectionId> {
    if s.nodes.get(&n).is_none_or(crate::state::Node::is_open) {
        return None;
    }
    s.ix.items
        .get(&n)
        .and_then(|it| it.open_objections.first().copied())
}

#[cfg(test)]
mod tests;
