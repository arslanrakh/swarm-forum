//! The request board (design D.8.8, §8.3, CS-25): every request and every unrouted user entry,
//! each with its requester and the first state that holds (A-35). Unrouted entries and unsettled
//! requests come first, oldest first; settled requests follow, newest first. A page costs the
//! page size plus the unsettled backlog, not the size of the forum (§18).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::{BoardPage, BoardRow, Meter, SCAN_MAX};
use crate::cmd::BoardState;
use crate::reply::{BoardScope, Total};
use crate::state::{Request, State};
use crate::types::{Eid, PrincipalId, RequestId, Requester, Time};

/// One row before its state: an unrouted entry or a request.
#[derive(Clone, Copy)]
enum Item {
    Entry(Eid),
    Request(RequestId),
}

/// The state of a request's row: the first that holds (A-35).
fn request_state(s: &State, r: &Request, id: RequestId, now: Time) -> BoardState {
    if r.answered_by.is_some() {
        BoardState::Answered
    } else if s.nodes.get(&r.node).is_some_and(|x| !x.is_open()) {
        BoardState::ClosedUnanswered
    } else if s.ix.settling.get(&id).is_some_and(|d| !d.is_empty()) {
        BoardState::Decided
    } else if s
        .nodes
        .get(&r.node)
        .is_some_and(|x| crate::flags::waiting(s, x, now))
    {
        BoardState::Waiting
    } else {
        BoardState::InProgress
    }
}

fn row(s: &State, item: Item, now: Time) -> Option<BoardRow> {
    match item {
        Item::Entry(e) => {
            let u = s.user_entries.get(&e)?;
            Some(BoardRow {
                request: None,
                entry: Some(e),
                node: None,
                requester: Requester::User(u.principal),
                state: BoardState::Unrouted,
            })
        }
        Item::Request(id) => {
            let r = s.requests.get(&id)?;
            Some(BoardRow {
                request: Some(id),
                entry: r.from_user,
                node: Some(r.node),
                requester: r.requester,
                state: request_state(s, r, id, now),
            })
        }
    }
}

/// The request board (D.8.8, CS-25), the first `limit` rows of the scope with a `status` filter
/// or without, and the total: exact without a filter; with one, the rows are tested in order and
/// the scan stops after `SCAN_MAX` visited rows, the total then `≥k`. Settled requests that left
/// memory are the shell's to page on (D.4.12); the core lists the resident ones.
#[must_use]
pub fn board<M: Meter>(
    s: &State,
    status: Option<BoardState>,
    scope: &BoardScope,
    now: Time,
    limit: u32,
    m: &mut M,
) -> BoardPage {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    // Part 1: unrouted entries and unsettled requests, oldest first.
    let mut first: Vec<(Eid, Item)> = Vec::new();
    let in_scope = |p: PrincipalId| match scope {
        BoardScope::All => true,
        BoardScope::Principals(ps) => ps.contains(&p),
    };
    for &e in &s.ix.unrouted {
        if s.user_entries
            .get(&e)
            .is_some_and(|u| in_scope(u.principal))
        {
            first.push((e, Item::Entry(e)));
        }
    }
    match scope {
        BoardScope::All => {
            first.extend(
                s.ix.unsettled_requests
                    .iter()
                    .map(|&r| (r, Item::Request(r))),
            );
        }
        BoardScope::Principals(ps) => {
            for p in ps {
                first.extend(
                    s.ix.unsettled_by
                        .get(p)
                        .into_iter()
                        .flatten()
                        .map(|&r| (r, Item::Request(r))),
                );
            }
        }
    }
    first.sort_by_key(|(e, _)| *e);
    m.tick(u64::try_from(first.len()).unwrap_or(u64::MAX));
    // Part 2: settled requests, newest first.
    let settled: BTreeSet<RequestId> = match scope {
        BoardScope::All => BTreeSet::new(),
        BoardScope::Principals(ps) => ps
            .iter()
            .flat_map(|p| s.ix.requests_by.get(p).into_iter().flatten())
            .copied()
            .filter(|r| s.requests.get(r).is_some_and(|x| x.settled.is_some()))
            .collect(),
    };
    let second: alloc::boxed::Box<dyn Iterator<Item = (Eid, Item)>> = match scope {
        BoardScope::All => alloc::boxed::Box::new(
            s.requests
                .iter()
                .rev()
                .filter(|(_, r)| r.settled.is_some())
                .map(|(&id, _)| (id, Item::Request(id))),
        ),
        BoardScope::Principals(_) => {
            alloc::boxed::Box::new(settled.iter().rev().map(|&id| (id, Item::Request(id))))
        }
    };
    let all_count = match scope {
        BoardScope::All => first.len().saturating_add(
            s.requests
                .len()
                .saturating_sub(s.ix.unsettled_requests.len()),
        ),
        BoardScope::Principals(_) => first.len().saturating_add(settled.len()),
    };
    let mut rows: Vec<BoardRow> = Vec::new();
    let mut matched = 0_usize;
    let mut visited = 0_u32;
    let mut capped = false;
    for (_, item) in first.into_iter().chain(second) {
        if status.is_some() {
            if visited >= SCAN_MAX {
                capped = true;
                break;
            }
            visited = visited.saturating_add(1);
        } else if rows.len() >= limit {
            break;
        }
        m.tick(1);
        let Some(r) = row(s, item, now) else { continue };
        if status.is_some_and(|st| st != r.state) {
            continue;
        }
        matched = matched.saturating_add(1);
        if rows.len() < limit {
            rows.push(r);
        }
    }
    let total = match status {
        None => Total::Exact(u32::try_from(all_count).unwrap_or(u32::MAX)),
        Some(_) => {
            let n = u32::try_from(matched).unwrap_or(u32::MAX);
            if capped {
                Total::AtLeast(n)
            } else {
                Total::Exact(n)
            }
        }
    };
    BoardPage { rows, total }
}
