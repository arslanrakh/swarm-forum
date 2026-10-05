//! Errors (design D.6.3): every code of §21, with a structured hint that the shell renders to text.
//!
//! Only `stale_token` applies something: one `late` entry (H4). Two pieces of bookkeeping may
//! accompany any error but `limit(TextTooLong)`, which appends nothing (D.8.1 step 1): the
//! `SessionOpened` of a first call (A-38), with the `PrincipalSeen` of its principal when that is
//! new (CS-1), and the `OpRecorded` of O1 when the error appended events.

use alloc::vec::Vec;

use crate::types::{Eid, NodeId, Time, Token, Version};

/// A refusal: a code and a structured hint.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Error {
    /// The code of §21.
    pub code: ErrorCode,
    /// What the shell renders as the hint.
    pub hint: Hint,
}

/// The error codes of §21.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum ErrorCode {
    /// `full` (H1).
    Full,
    /// `holds_limit` (H2).
    HoldsLimit,
    /// `paused` (P2).
    Paused,
    /// `stale_token` (H4).
    StaleToken,
    /// `stale_version` (O2).
    StaleVersion,
    /// `cycle` (T5, K4, K5).
    Cycle,
    /// `closed` (H6, K5).
    Closed,
    /// `op_id_conflict` (O1).
    OpIdConflict,
    /// `limit` (L1, H5).
    Limit,
    /// `invalid`.
    Invalid,
    /// `not_found`.
    NotFound,
}

/// The structured hint of an error.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Hint {
    /// H1.
    Full {
        /// The node.
        node: NodeId,
        /// Its capacity.
        capacity: u32,
        /// Its live holds.
        live: u32,
    },
    /// H2.
    HoldsLimit {
        /// The session's holds.
        holds: Vec<Token>,
    },
    /// P2.
    Paused {
        /// The paused scope.
        scope: NodeId,
        /// Until when.
        until: Time,
    },
    /// H4: the late entry saved.
    StaleToken {
        /// The stale token.
        token: Token,
        /// The `late` entry that holds the attempted write.
        late: Eid,
    },
    /// O2.
    StaleVersion {
        /// The node's version.
        current: Version,
        /// The node's digest version.
        digest_current: Version,
    },
    /// T5, K4, K5.
    Cycle {
        /// The cycle.
        path: Vec<NodeId>,
    },
    /// H6, K5.
    Closed {
        /// The node.
        node: NodeId,
        /// The node it was merged into.
        merged_into: Option<NodeId>,
    },
    /// O1.
    OpIdConflict,
    /// L1, H5.
    Limit(LimitKind),
    /// Why the call is invalid.
    Invalid(Why),
    /// What was not found.
    NotFound(What),
}

/// `WritesPerHour` and `Busy` are counted by the shell (D.11.15); `WritesPerHour` is answered in
/// §21's precedence, after the session and the target (D.8.1). `Busy`: the class's oldest queued
/// job has waited longer than `queue.max_wait`, so nothing was applied; retry with the same `op_id`
/// (CS-29). `TextTooLong`: a free-text argument longer than `text.max` bytes after masking (§19,
/// L1, A-114; the shell renders the limit from `params`); the shell answers it before the job is
/// queued, and `apply` repeats the check before anything else (D.8.1); nothing is applied, and no
/// session opens.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum LimitKind {
    /// `renew.max` renews without a checkpoint.
    Renew,
    /// `tree.max_depth`.
    Depth,
    /// `tree.max_children`.
    Children,
    /// `write.per_hour` (the shell's).
    WritesPerHour,
    /// The writer queue (the shell's).
    Busy,
    /// `text.max`.
    TextTooLong,
}

/// What a `not_found` did not find. `Session` is the call's own session (§21: its remedy is a new
/// session); `Addressee` (1.4, §21, R12) a session named in `to` of `say` or `ask`, such as a
/// retired one: it has no remedy, and the hint names `to` and says that the caller's own session
/// is unaffected.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum What {
    /// A node.
    Node,
    /// The call's own session (also a retired one, CS-8).
    Session,
    /// An entry.
    Entry,
    /// A request.
    Request,
    /// A question.
    Question,
    /// A `said` entry.
    Said,
    /// A decision.
    Decision,
    /// An objection.
    Objection,
    /// A pause.
    Pause,
    /// A key.
    Key,
    /// A user entry.
    UserEntry,
    /// A session named in `to` (1.4, R12).
    Addressee,
}

/// The empty text an `invalid(EmptyText)` names.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Field {
    /// A title.
    Title,
    /// A summary.
    Summary,
    /// A next step.
    NextStep,
    /// A text.
    Text,
    /// A reason.
    Reason,
}

/// Why a call is `invalid`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Why {
    /// A live name at that (parent, name) (§4).
    NameTaken,
    /// Move, merge, close, a capacity/repeat change, a key, or a routing target on a reserved node
    /// (K8); a reserved conversation node or request's node (C1) (§4, §21, A-29).
    ReservedNode,
    /// An agent-supplied `repeat:` key (A-90).
    ReservedKey,
    /// Close of a closed node (A-66).
    AlreadyClosed,
    /// Reopen of an open node.
    NotClosed,
    /// Reopen or take of an occurrence while its series is open (§12, A-13).
    OccurrenceReopen,
    /// Take, merge from, or a close other than `dropped` of an occurrence still before its slot
    /// (§12, §25, A-91).
    OccurrenceNotYetAvailable,
    /// A link to its own node; a decision's basis reference to its own node is dropped with
    /// `self_basis_dropped` instead (1.4, R14).
    SelfLink,
    /// A merge of a node into itself.
    SelfMerge,
    /// A-33.
    MergeFromClosed,
    /// A merge of a series into its own open occurrence (1.5, N5): closing the series would drop
    /// that occurrence, and with it what the merge moved there.
    MergeIntoOwnOccurrence,
    /// A new intent while one is open (A-67).
    IntentOpen,
    /// `intent_done`, of a checkpoint or a release (1.4), with no open intent (A-67).
    NoIntent,
    /// No `where`, and no node from `at` up carries a conversation key.
    NoConversationKey,
    /// The nearest such node carries several (A-30).
    AmbiguousConversationKey,
    /// `create {request}` with no origin and no keys.
    NoRequestOrigin,
    /// No slot within `forum_cron::MAX_SEARCH_DAYS`.
    ScheduleNeverFires,
    /// The `TzTable` fails `forum_cron::valid_table`.
    BadTimeZone,
    /// A `repeat` with `run_within = 0`.
    RunWithinZero,
    /// A required text is empty.
    EmptyText(Field),
    /// A digest longer than `digest.max` bytes.
    DigestTooLong,
    /// `update` sets `digest` without `digest_version` (CS-11).
    MissingDigestVersion,
    /// `talk said` with kind question (questions go through `talk ask`).
    SaidKindQuestion,
    /// `heard {answers}` on an agent or user question.
    NotPeopleQuestion,
    /// `talk answer` on a people or user question (A-36).
    NotAgentQuestion,
    /// `from_user {answers}` on a people or agent question.
    NotUserQuestion,
    /// `talk route` on an entry that is not `from_user`.
    NotUserEntry,
    /// `to_user {re}` naming an entry that is not a request, a `from_user` entry, or a digest entry
    /// (§20.7).
    NotReportable,
    /// `supersedes` names a superseded or retracted decision.
    DecisionNotActive,
    /// `repeat` on an occurrence or a signal node.
    RoleConflict,
    /// An agent closing with outcome `missed` (A-66).
    ServerOutcome,
    /// A session of another class (A-69).
    ClassMismatch,
    /// `approve {on: id@v}`, or a basis reference `#id@v` (CS-22), with `v` newer than the node
    /// (A-71).
    VersionAhead,
    /// An `external` basis reference whose `checked_at` lies after `now` (1.4, §15, re-run R27b,
    /// D.14.5). No longer returned: since 1.6 such a time is read as `now` (§15). Kept so that a
    /// stored outcome that names it still reads.
    CheckedAtAhead,
    /// `from_user` or `to_user` with no principal argument, in a session that serves principals of
    /// several persons (CS-1, D.8.11).
    AmbiguousPrincipal,
    /// 1.9 (§8.1, §21): `from_user` with no principal, the call's or `hello`'s, in a session that
    /// serves none. A `from_user` records a person who wrote to the agent directly, whom the
    /// agent's client names, or `user` when it serves a person it does not name; in the lab, every
    /// human message logged at `hello` came with a principal, and every scheduler's prompt or
    /// tool's output logged as one came without.
    NoPrincipal,
    /// `AdminCmd` params fail `Params::valid` or current usage; `Admin` outside
    /// `SessionCtx::System` or `System` with another command; the placeholder of a command stub
    /// before its package lands (D.13.2).
    BadParams,
}

impl Error {
    /// `invalid(why)`.
    #[must_use]
    pub const fn invalid(why: Why) -> Error {
        Error {
            code: ErrorCode::Invalid,
            hint: Hint::Invalid(why),
        }
    }

    /// `not_found(what)`.
    #[must_use]
    pub const fn not_found(what: What) -> Error {
        Error {
            code: ErrorCode::NotFound,
            hint: Hint::NotFound(what),
        }
    }

    /// `limit(kind)`.
    #[must_use]
    pub const fn limit(kind: LimitKind) -> Error {
        Error {
            code: ErrorCode::Limit,
            hint: Hint::Limit(kind),
        }
    }

    /// `op_id_conflict`.
    #[must_use]
    pub const fn op_id_conflict() -> Error {
        Error {
            code: ErrorCode::OpIdConflict,
            hint: Hint::OpIdConflict,
        }
    }

    /// The code that goes with a hint: every hint belongs to exactly one code.
    #[must_use]
    pub const fn code_of(hint: &Hint) -> ErrorCode {
        match hint {
            Hint::Full { .. } => ErrorCode::Full,
            Hint::HoldsLimit { .. } => ErrorCode::HoldsLimit,
            Hint::Paused { .. } => ErrorCode::Paused,
            Hint::StaleToken { .. } => ErrorCode::StaleToken,
            Hint::StaleVersion { .. } => ErrorCode::StaleVersion,
            Hint::Cycle { .. } => ErrorCode::Cycle,
            Hint::Closed { .. } => ErrorCode::Closed,
            Hint::OpIdConflict => ErrorCode::OpIdConflict,
            Hint::Limit(_) => ErrorCode::Limit,
            Hint::Invalid(_) => ErrorCode::Invalid,
            Hint::NotFound(_) => ErrorCode::NotFound,
        }
    }

    /// The error a hint describes, with its code.
    #[must_use]
    pub const fn of(hint: Hint) -> Error {
        Error {
            code: Error::code_of(&hint),
            hint,
        }
    }
}
