//! The lists of `view` and `talk who` (§18, §20.4, design D.11.7): `list` with each node's rollup
//! and the exact counts of the subtree it lists from (CS-32), the pages of `feed`, `user_log`, and
//! `history` from the shell's tables (CS-17, A-78), each history event in the shell's formats
//! (`events`), the request board with each state's detail (§8.3, CS-25), `find` by key, a page of
//! carriers, or by text, and `who` (CS-21). A list cut short carries `shown/total` or `≥k`, and one
//! shown whole carries no counts (1.7, §18, §20.1).

use std::collections::BTreeSet;

use forum_core::query::{BoardRow, NoMeter, PAGE_DEFAULT, PAGE_MAX, board, find_key, holders, who};
use forum_core::{
    BoardScope, BoardState, Eid, EntryType, FindBy, NodeId, ROOT, Requester, Seq, State, TextField,
    Time, Total, USER, ViewCmd,
};

use super::brief::{below, holders_json, rollup};
use super::entries::{entry, node_line, shown_text, text_of};
use super::events::{history_event, wait};
use super::format::{board_state, dur, node_ref, path, principal, shown_if_cut, time};
use super::hello::{SearchHits, hit, hits};
use super::json::Json;
use super::source::Source;

/// A title in a list is cut to this many bytes.
pub const LIST_TITLE_MAX: usize = 200;

/// The closed carriers `view find {key}` lists, newest first (§18: every list is bounded; the
/// same 20 as a search by text, §20.4).
pub const FIND_MAX: usize = 20;

/// An object without its fields that are not set (1.7, §20.1), such as the root's empty title.
fn unset_left_out(j: Json) -> Json {
    match j {
        Json::O(fields) => Json::O(fields.into_iter().filter(|(_, v)| v.is_set()).collect()),
        other => other,
    }
}

/// `view list`: each node with its subtree counts, its needs, and its holders, when it has them
/// (1.7: in the lab's second round, a list of 9.5 KB was a third rollups of zeros, and agents read
/// each node again for its needs and holders), and the counts of the subtree the list is drawn
/// from that are not 0 (CS-32).
#[must_use]
pub fn list(
    s: &State,
    nodes: &[NodeId],
    total: Total,
    under: Option<NodeId>,
    now: Time,
    viewer: Option<forum_core::SessionId>,
    src: &dyn Source,
) -> Json {
    let items = nodes
        .iter()
        .map(|n| {
            let r = s.ix.node.get(n).copied().unwrap_or_default();
            let roll = rollup(&forum_core::query::Rollup {
                open_children: r.open_children,
                held_children: r.held_children,
                flagged_children: r.flagged_children,
                sub: r.sub,
            });
            let needs = s
                .nodes
                .get(n)
                .map(|x| Json::strs(x.needs.iter().map(ToString::to_string)))
                .unwrap_or(Json::Null);
            unset_left_out(node_line(s, *n, LIST_TITLE_MAX, viewer, src))
                .with_opt("rollup", roll.map(Json::S))
                .with_set("needs", needs)
                .with_set("holders", holders_json(&holders(s, *n, now)))
        })
        .collect();
    let u = under.unwrap_or(ROOT);
    let sub = s.ix.node.get(&u).map(|i| i.sub).unwrap_or_default();
    Json::obj()
        .with("nodes", Json::A(items))
        .with_opt("shown", shown_if_cut(nodes.len(), total))
        .with(
            "under",
            Json::obj()
                .with("path", Json::S(path(s, u)))
                .with_opt("below", below(&sub).map(Json::S)),
        )
}

/// `view feed`: the page the core named (CS-17), oldest first, and the `since` of the next page.
#[must_use]
pub fn feed(
    s: &State,
    node: NodeId,
    after: Seq,
    upto: Seq,
    types: &BTreeSet<EntryType>,
    limit: u32,
    src: &dyn Source,
) -> Json {
    let page = src.feed(node, after, upto, types, limit);
    let next = page.entries.last().map_or(after, |v| v.seq);
    Json::obj()
        .with("at", Json::S(path(s, node)))
        .with(
            "entries",
            Json::A(page.entries.iter().map(|v| entry(s, v, None)).collect()),
        )
        .with_opt("shown", shown_if_cut(page.entries.len(), page.total))
        .with("next_since", Json::U(next.0))
}

/// `view user_log`: the `from_user` and `to_user` entries of `@user` after `since`, oldest first.
#[must_use]
pub fn user_log(s: &State, since: Option<Eid>, limit: u32, src: &dyn Source) -> Json {
    let after = since
        .and_then(|e| src.entry(e))
        .filter(|v| v.node == USER)
        .map_or(Seq(0), |v| v.seq);
    let types: BTreeSet<EntryType> = [EntryType::FromUser, EntryType::ToUser].into();
    let upto = s.nodes.get(&USER).map_or(Seq(0), |x| x.seq);
    let page = src.feed(USER, after, upto, &types, limit.min(PAGE_MAX));
    Json::obj()
        .with(
            "entries",
            Json::A(page.entries.iter().map(|v| entry(s, v, None)).collect()),
        )
        .with_opt("shown", shown_if_cut(page.entries.len(), page.total))
}

/// `view history`: the node's events with `seq < before`, newest first, and the `before` of the
/// next page (A-78).
#[must_use]
pub fn history(s: &State, node: NodeId, before: Option<u64>, limit: u32, src: &dyn Source) -> Json {
    let page = src.history(node, before, limit.min(PAGE_MAX));
    let next = page.events.last().map(|(q, _)| *q);
    Json::obj()
        .with("at", Json::S(path(s, node)))
        .with(
            "events",
            Json::A(
                page.events
                    .iter()
                    .map(|(q, e)| history_event(s, *q, e, src))
                    .collect(),
            ),
        )
        .with_opt("shown", shown_if_cut(page.events.len(), page.total))
        .with_opt("next_before", next.map(Json::U))
}

/// The detail of one row of the board, by its state (§8.3's table, D.8.8): how long an unrouted
/// entry has waited; when and through which agent a request was answered; the flag of a request
/// closed without an answer; the decision that lists a decided one and its text, the outcome; what
/// a waiting one waits for; and the node's holders and last checkpoint for one in progress.
fn row_detail(s: &State, r: &BoardRow, now: Time, src: &dyn Source, j: &mut Json) {
    match r.state {
        BoardState::Unrouted => {
            if let Some(v) = r.entry.and_then(|e| src.entry(e)) {
                j.put("waited", Json::S(dur(now.since(v.at))));
            }
        }
        BoardState::Answered => {
            let answer = r
                .request
                .and_then(|q| s.requests.get(&q))
                .and_then(|x| x.answered_by);
            if let Some(a) = answer {
                let mut x = Json::obj().with("entry", Json::S(a.render()));
                if let Some(v) = src.entry(a) {
                    x.put("at", Json::S(time(v.at)));
                    x.put("by", Json::S(super::entries::author(&v.by)));
                }
                j.put("answered", x);
            }
        }
        BoardState::ClosedUnanswered => {
            j.put("flag", Json::s("unanswered_request"));
        }
        BoardState::Decided => {
            let decision = r
                .request
                .and_then(|q| s.ix.settling.get(&q))
                .and_then(|ds| ds.iter().next_back().copied());
            if let Some(d) = decision {
                j.put("decision", Json::S(d.render()));
                if let Some(v) = src.entry(d) {
                    j.put("outcome", Json::S(text_of(&v, Some(LIST_TITLE_MAX))));
                }
            }
        }
        BoardState::Waiting => {
            let w = r
                .node
                .and_then(|n| s.nodes.get(&n))
                .and_then(|x| x.wait.as_ref().map(|w| (x, w)));
            if let Some((x, w)) = w {
                let waits = match w.on {
                    // The `what` of an external wait is a text of the release that set it, the
                    // node's last checkpoint: `{external, recheck_after}`, as `waits_for` reads.
                    forum_core::WaitOn::External(t) => {
                        let what = x.last_checkpoint.and_then(|c| src.entry(c)).and_then(|v| {
                            v.extra(TextField::What)
                                .map(|t| shown_text(&v, t, Some(LIST_TITLE_MAX)))
                        });
                        Json::obj()
                            .with_opt("external", what.map(Json::S))
                            .with("recheck_after", Json::S(time(t)))
                    }
                    _ => wait(s, &w.on),
                };
                j.put("waits_for", waits);
            }
        }
        BoardState::InProgress => {
            if let Some(n) = r.node {
                j.put(
                    "holders",
                    Json::A(
                        holders(s, n, now)
                            .iter()
                            .map(|(x, until)| {
                                Json::obj()
                                    .with("session", Json::S(x.render()))
                                    .with("until", Json::S(time(*until)))
                            })
                            .collect(),
                    ),
                );
                let last = s.nodes.get(&n).and_then(|x| x.last_checkpoint);
                if let Some(c) = last {
                    let mut x = Json::obj().with("entry", Json::S(c.render()));
                    if let Some(v) = src.entry(c) {
                        x.put("at", Json::S(time(v.at)));
                    }
                    j.put("last_checkpoint", x);
                }
            }
        }
    }
}

/// `view requests`: the request board (§8.3), `PAGE_DEFAULT` rows, each with its state's detail.
/// The requester of a request from outside the swarm is a text of the request, hidden with it.
#[must_use]
pub fn requests(
    s: &State,
    status: Option<BoardState>,
    scope: &BoardScope,
    now: Time,
    src: &dyn Source,
) -> Json {
    let page = board(s, status, scope, now, PAGE_DEFAULT, &mut NoMeter);
    let rows = page
        .rows
        .iter()
        .map(|r| {
            let request = r.request.and_then(|q| src.entry(q));
            let requester = match r.requester {
                Requester::User(p) => principal(s, p),
                Requester::External => request
                    .as_ref()
                    .and_then(|v| {
                        v.extra(TextField::Requester)
                            .map(|t| shown_text(v, t, Some(LIST_TITLE_MAX)))
                    })
                    .unwrap_or_else(|| "external".into()),
            };
            let text = request
                .or_else(|| r.entry.and_then(|e| src.entry(e)))
                .map(|v| text_of(&v, Some(LIST_TITLE_MAX)));
            let mut j = Json::obj()
                .with_opt("request", r.request.map(|q| Json::S(q.render())))
                .with_opt("entry", r.entry.map(|e| Json::S(e.render())))
                .with_opt("node", r.node.map(|n| Json::S(node_ref(s, n))))
                .with("requester", Json::S(requester))
                .with("state", Json::s(board_state(r.state)));
            row_detail(s, r, now, src, &mut j);
            j.with_opt("text", text.map(Json::S))
        })
        .collect();
    Json::obj()
        .with("rows", Json::A(rows))
        .with_opt("shown", shown_if_cut(page.rows.len(), page.total))
}

/// `view find`: by key, the open carrier first, then a page of the closed carriers, newest first
/// (§7), at most `FIND_MAX`, with `shown/total` and, when there are more, how to reach the older
/// ones; by text, the search's matches (D.11.12).
#[must_use]
pub fn find(
    s: &State,
    by: &FindBy,
    found: &SearchHits,
    viewer: Option<forum_core::SessionId>,
    src: &dyn Source,
) -> Json {
    match by {
        FindBy::Key(k) => {
            let f = find_key(s, k);
            let all = f.closed.len();
            let page: Vec<NodeId> = f.closed.iter().take(FIND_MAX).copied().collect();
            let total = Total::Exact(u32::try_from(all).unwrap_or(u32::MAX));
            let more = (all > page.len()).then(|| {
                Json::s(
                    "the older carriers stay reachable along the continues chain: view node of \
                     the last one shown lists its predecessors",
                )
            });
            Json::obj()
                .with("key", Json::S(k.to_string()))
                .with_opt(
                    "open",
                    f.open.map(|n| hit(s, n, LIST_TITLE_MAX, viewer, src)),
                )
                .with(
                    "closed",
                    Json::A(
                        page.iter()
                            .map(|n| hit(s, *n, LIST_TITLE_MAX, viewer, src))
                            .collect(),
                    ),
                )
                .with_opt("shown", shown_if_cut(page.len(), total))
                .with_opt("more", more)
        }
        FindBy::Text(t) => Json::obj().with("text", Json::S(t.0.clone())).with(
            "matches",
            hits(
                s,
                &found.related,
                &found.matched,
                LIST_TITLE_MAX,
                viewer,
                src,
            ),
        ),
    }
}

/// `talk who {at?}` (CS-21): at most `WHO_MAX` sessions, with counts by class without `at`.
#[must_use]
pub fn who_list(s: &State, at: Option<NodeId>, now: Time) -> Json {
    let w = who(s, at, now);
    let items = w
        .items
        .iter()
        .map(|p| {
            Json::obj()
                .with("session", Json::S(p.session.render()))
                .with("class", Json::S(p.class.to_string()))
                .with_set("holds", Json::strs(p.holds.iter().map(|n| node_ref(s, *n))))
                .with("last_seen", Json::S(time(p.last_seen)))
        })
        .collect();
    let mut j = Json::obj()
        .with("sessions", Json::A(items))
        .with_opt("shown", shown_if_cut(w.items.len(), w.total));
    if at.is_none() {
        j.put(
            "by_class",
            Json::O(
                w.by_class
                    .iter()
                    .map(|(c, k)| (c.to_string(), Json::from(*k)))
                    .collect(),
            ),
        );
    }
    j
}

/// The `under` of a `view list` command.
#[must_use]
pub fn list_under(s: &State, cmd: &forum_core::Cmd) -> Option<NodeId> {
    match cmd {
        forum_core::Cmd::View(ViewCmd::List { under, .. }) => under
            .as_ref()
            .and_then(|r| forum_core::query::resolve(s, r)),
        _ => None,
    }
}
