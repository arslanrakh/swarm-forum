//! Executable invariants (design D.9): the invariant ids, violations, the registry, and the
//! checks. WP-D1 freezes the types, the registry, and the signature of `check_guide`; the checks
//! themselves are WP-D2's (state), WP-D3's and WP-D4's (step), and WP-D7's (G1).
//!
//! The checks compute from the base state by full recount and share no code with `evolve`'s
//! incremental maintenance, nor predicate code with `apply`: they use `forum_core::spec`, naive
//! definitions over the base maps (D.9.1).

mod guide;
pub mod registry;
mod state;
mod step;

pub use state::{check_invariants, check_keys, check_startup, r1_differences};
pub use step::{check_fire, check_step};

use alloc::vec::Vec;

use crate::cmd::Cmd;
use crate::error::Error;
use crate::query::guide::Guidance;
use crate::reply::Done;
use crate::state::{DeadlineKey, State};
use crate::types::{Eid, KeyStr, NodeId, SessionId, Time, Token};

/// One violated invariant.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Violation {
    /// Which invariant.
    pub inv: Inv,
    /// Where.
    pub detail: ViolationDetail,
}

/// The §26 items, plus `Total` (evolve applied every event as given: `anomalies == 0`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(kani, derive(kani::Arbitrary))]
#[allow(
    clippy::upper_case_acronyms,
    reason = "the ids are §26's own: AR is archived"
)]
pub enum Inv {
    /// Slots.
    H1,
    /// Holds per session.
    H2,
    /// Fences.
    H3,
    /// Stale tokens (step).
    H4,
    /// Leases.
    H5,
    /// No hold on a closed node.
    H6,
    /// Retries.
    O1,
    /// Counters.
    O2,
    /// History (step).
    O3,
    /// Indices equal a recount.
    R1,
    /// One open carrier per key.
    K1,
    /// Normal keys.
    K2,
    /// Names.
    K3,
    /// Parents reach the root.
    K4,
    /// `merged_into`.
    K5,
    /// One open occurrence per open series.
    K6,
    /// Reopened keys (step).
    K7,
    /// Reserved nodes.
    K8,
    /// Conversation nodes (step).
    C1,
    /// One request per origin.
    Q1,
    /// One open signal per pair.
    S1,
    /// User entries are routed.
    N1,
    /// Settled requests.
    Settled,
    /// Flags.
    F,
    /// Merges (step).
    N5,
    /// Refusals and questions reach a mover.
    E1,
    /// Waiting nodes.
    W1,
    /// Missed occurrences.
    R2,
    /// The digest.
    D1,
    /// Movers.
    M1,
    /// Questions for the user.
    U1,
    /// Time stamps.
    T1,
    /// Path independence.
    T2,
    /// Settled states.
    T3,
    /// Deadlines.
    T4,
    /// No wait or dependency cycle.
    T5,
    /// Limits.
    L1,
    /// Pauses.
    P1,
    /// No hold under a pause (step).
    P2,
    /// Archiving.
    AR,
    /// Next calls (1.3).
    G1,
    /// `anomalies == 0`.
    Total,
}

impl Inv {
    /// Every invariant, in declaration order.
    pub const ALL: [Inv; 42] = [
        Inv::H1,
        Inv::H2,
        Inv::H3,
        Inv::H4,
        Inv::H5,
        Inv::H6,
        Inv::O1,
        Inv::O2,
        Inv::O3,
        Inv::R1,
        Inv::K1,
        Inv::K2,
        Inv::K3,
        Inv::K4,
        Inv::K5,
        Inv::K6,
        Inv::K7,
        Inv::K8,
        Inv::C1,
        Inv::Q1,
        Inv::S1,
        Inv::N1,
        Inv::Settled,
        Inv::F,
        Inv::N5,
        Inv::E1,
        Inv::W1,
        Inv::R2,
        Inv::D1,
        Inv::M1,
        Inv::U1,
        Inv::T1,
        Inv::T2,
        Inv::T3,
        Inv::T4,
        Inv::T5,
        Inv::L1,
        Inv::P1,
        Inv::P2,
        Inv::AR,
        Inv::G1,
        Inv::Total,
    ];
}

/// Where a violation is. Serialize only under the `serde` feature (D.2): `&'static str` cannot be
/// deserialised.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ViolationDetail {
    /// A node.
    Node(NodeId),
    /// A hold.
    Hold(Token),
    /// A session.
    Session(SessionId),
    /// An entry.
    Entry(Eid),
    /// A key.
    Key(KeyStr),
    /// A deadline.
    Deadline(DeadlineKey),
    /// An index, by name.
    Index(&'static str),
    /// Anything else, described.
    Text(&'static str),
}

/// G1 (§26 "Responses", 1.3): the next calls `g` of the response to `sess` for `cmd` with outcome
/// `out`, computed on `s`, the state after the call and its post-settle at `now`. Run by
/// `World::call` after `check_step` in every test, and by the server in debug builds after every
/// command. Its clauses are D.9.2's G1 row. Body: WP-D7.
///
/// # Errors
///
/// The violations of G1 it finds.
pub fn check_guide(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    g: &Guidance,
) -> Result<(), Vec<Violation>> {
    guide::check_guide(s, sess, now, cmd, out, None, g)
}

/// `check_guide` with the first row of the call's `related`, which the shell's search gave the
/// next calls (1.11, §20.9 "Inputs"): G1 (a) recomputes them with it.
///
/// # Errors
///
/// The violations of G1 it finds.
pub fn check_guide_with(
    s: &State,
    sess: Option<SessionId>,
    now: Time,
    cmd: &Cmd,
    out: &Result<Done, Error>,
    related: Option<NodeId>,
    g: &Guidance,
) -> Result<(), Vec<Violation>> {
    guide::check_guide(s, sess, now, cmd, out, related, g)
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;

    /// Each variant's place in declaration order. The match names every variant and the crate
    /// denies `wildcard_enum_match_arm`, so a new `Inv` does not compile until it is numbered here,
    /// and the test below then fails until `Inv::ALL` lists it (D.9.4: one registry row per `Inv`,
    /// which the registry's tests check over `Inv::ALL`).
    fn ordinal(inv: Inv) -> usize {
        match inv {
            Inv::H1 => 0,
            Inv::H2 => 1,
            Inv::H3 => 2,
            Inv::H4 => 3,
            Inv::H5 => 4,
            Inv::H6 => 5,
            Inv::O1 => 6,
            Inv::O2 => 7,
            Inv::O3 => 8,
            Inv::R1 => 9,
            Inv::K1 => 10,
            Inv::K2 => 11,
            Inv::K3 => 12,
            Inv::K4 => 13,
            Inv::K5 => 14,
            Inv::K6 => 15,
            Inv::K7 => 16,
            Inv::K8 => 17,
            Inv::C1 => 18,
            Inv::Q1 => 19,
            Inv::S1 => 20,
            Inv::N1 => 21,
            Inv::Settled => 22,
            Inv::F => 23,
            Inv::N5 => 24,
            Inv::E1 => 25,
            Inv::W1 => 26,
            Inv::R2 => 27,
            Inv::D1 => 28,
            Inv::M1 => 29,
            Inv::U1 => 30,
            Inv::T1 => 31,
            Inv::T2 => 32,
            Inv::T3 => 33,
            Inv::T4 => 34,
            Inv::T5 => 35,
            Inv::L1 => 36,
            Inv::P1 => 37,
            Inv::P2 => 38,
            Inv::AR => 39,
            Inv::G1 => 40,
            Inv::Total => 41,
        }
    }

    /// The last variant in declaration order; a variant added after it must move this too.
    const LAST: Inv = Inv::Total;

    #[test]
    fn all_lists_every_inv_once_in_declaration_order() {
        for (i, inv) in Inv::ALL.into_iter().enumerate() {
            assert_eq!(ordinal(inv), i, "Inv::ALL[{i}] is {inv:?}");
        }
        assert_eq!(Inv::ALL.len(), ordinal(LAST) + 1);
        assert_eq!(Inv::ALL[Inv::ALL.len() - 1], LAST);
        // The derived order is declaration order.
        assert!(Inv::ALL.windows(2).all(|w| w[0] < w[1]));
    }
}
