//! The query API of design D.8.25 as frozen at WP-D1, seen from outside the crate as the shell sees
//! it: one function pointer per query at its public path `forum_core::query::*`, so a changed
//! signature, or a query that is missing or not public there, fails to compile.
//!
//! One departure from D.8.25 as written: `principal_key` returns `Option<&PrincipalKey>`, so that it
//! is total over ids the state never interned without a made-up key (the crate denies panics).

use forum_core::health::{Health, MoverGap};
use forum_core::query::guide::Guidance;
use forum_core::query::{
    Backlinks, BoardPage, BriefData, FindResult, HelloData, ListFilter, ListPage, NoMeter, Ranked,
    StatusLine, WhoPage, archived, backlinks, board, board_scope, brief, find_key, guide, health,
    hello, holders, inbox, inbox_ready, is_under, leave_better, list, m1b_gaps, op_lookup, path_of,
    precheck, principal_key, ranked, resolve, resolve_merged, status_line, who,
};
use forum_core::*;

#[test]
#[allow(clippy::type_complexity, reason = "the signatures themselves")]
fn the_signatures_of_d_8_25() {
    let _: for<'a> fn(&'a State, &ClassName, &OpId, Time) -> Option<&'a OpRecord> = op_lookup;
    let _: fn(&State, &Ctx, &Cmd) -> Option<Error> = precheck;
    let _: fn(&State, &NodeRef) -> Option<NodeId> = resolve;
    let _: fn(&State, NodeId) -> NodeId = resolve_merged;
    let _: fn(&State, NodeId) -> Path = path_of;
    let _: fn(&State, NodeId, NodeId) -> bool = is_under;
    let _: fn(&State, NodeId, Time) -> bool = archived;
    let _: fn(&State, SessionId, Time) -> StatusLine = status_line;
    let _: fn(&State, SessionId, Time) -> HelloData = hello;
    let _: fn(&State, SessionId, Time, &NextQuery, usize, &mut NoMeter) -> (Vec<Ranked>, EmptyWhy) =
        ranked::<NoMeter>;
    let _: fn(&State, NodeId, Option<SessionId>, Time, &mut NoMeter) -> BriefData =
        brief::<NoMeter>;
    let _: fn(&State, Option<NodeId>, Option<SessionId>) -> Vec<NodeId> = leave_better;
    let _: fn(&State, NodeId, u32, &mut NoMeter) -> Backlinks = backlinks::<NoMeter>;
    let _: fn(&State, &KeyStr) -> FindResult = find_key;
    let _: fn(&State, &ListFilter, Time, &mut NoMeter) -> ListPage = list::<NoMeter>;
    let _: fn(&State, Option<BoardState>, &BoardScope, Time, u32, &mut NoMeter) -> BoardPage =
        board::<NoMeter>;
    let _: fn(&State, SessionId, Option<&RequesterFilter>) -> BoardScope = board_scope;
    let _: for<'a> fn(&'a State, PrincipalId) -> Option<&'a PrincipalKey> = principal_key;
    let _: fn(&State, Option<NodeId>, Time) -> WhoPage = who;
    let _: fn(&State, NodeId, Time) -> Vec<(SessionId, Time)> = holders;
    let _: fn(&State, SessionId, Time) -> Vec<NoteId> = inbox;
    let _: fn(&State, SessionId, Option<NodeId>, Option<Seq>, Time) -> bool = inbox_ready;
    let _: fn(&State, Time) -> Health = health;
    let _: fn(&State, Time) -> Vec<MoverGap> = m1b_gaps;
    let _: fn(
        &State,
        Option<SessionId>,
        Time,
        &Cmd,
        &Result<Done, Error>,
        &mut NoMeter,
    ) -> Guidance = guide::<NoMeter>;
}

/// §22's fixed values of the next calls, as the server sees them through this crate: the same
/// whatever features the build unifies (`cargo test --workspace` turns `conformance` on).
#[test]
fn the_next_calls_use_the_values_of_section_22() {
    use forum_core::query::guide::{CHECKPOINT_EVERY, LEASE_MARGIN};
    assert_eq!(CHECKPOINT_EVERY, Dur(5 * 60_000));
    assert_eq!(LEASE_MARGIN, Dur(60_000));
}
