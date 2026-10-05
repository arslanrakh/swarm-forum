//! What the renderer reads beside the state (design D.6.2, D.11.6): the texts the core never
//! holds, and the pages the shell lists from its own tables.
//!
//! The core keeps no entry text and no metadata of most entries (CS-17): the history records each
//! text once, in its `Appended` event, and the shell's `entries` table points at that event. A
//! [`Source`] answers the renderer's reads; [`StoreSource`] reads them on the writer's
//! connection, inside the transaction of the call (D.6.2), and [`MemorySource`] holds the events
//! of a test in memory.

use std::collections::{BTreeMap, BTreeSet};

use forum_core::{
    Author, Eid, EntryBody, EntryText, EntryType, Ev, Event, HideTarget, NodeId, Seq, SystemNote,
    Text, TextField, Time, Total, UserEntryId,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::store::history::{event_at, events_after, node_history};
use crate::store::shell_tables::entry_type_name;

/// Entries a filtered count visits before it says `≥k` (§18).
pub const SCAN_MAX: u32 = forum_core::query::SCAN_MAX;

/// Events after a `from_user` entry that [`Source::route_reason`] reads, at most, for the reason
/// of its routing to none: a routing comes soon after its entry, and a later one renders without
/// its reason rather than costing a walk through the history.
pub const ROUTE_SCAN_MAX: u32 = SCAN_MAX;

/// The events [`StoreSource::route_reason`] decodes per read.
const ROUTE_SCAN_PAGE: u32 = 256;

/// One entry as the renderer reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryView {
    /// The entry.
    pub eid: Eid,
    /// Its node.
    pub node: NodeId,
    /// Its seq there.
    pub seq: Seq,
    /// When it was recorded.
    pub at: Time,
    /// Who wrote it.
    pub by: Author,
    /// Its structural part.
    pub body: EntryBody,
    /// Its text.
    pub text: EntryText,
    /// Why it was hidden, if it was (§16): rendered as `[hidden: reason]`.
    pub hidden: Option<Text>,
}

impl EntryView {
    /// The further text with role `f`, if any.
    #[must_use]
    pub fn extra(&self, f: TextField) -> Option<&Text> {
        self.text
            .extra
            .iter()
            .find(|(g, _)| *g == f)
            .map(|(_, t)| t)
    }

    /// Every further text with role `f`, in order.
    pub fn extras(&self, f: TextField) -> impl Iterator<Item = &Text> {
        self.text
            .extra
            .iter()
            .filter(move |(g, _)| *g == f)
            .map(|(_, t)| t)
    }
}

/// A page of entries and its total.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryPage {
    /// The entries, in the page's order.
    pub entries: Vec<EntryView>,
    /// How many there are (§18).
    pub total: Total,
}

/// A page of a node's history: events, newest first, and their total.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPage {
    /// `(seq, event)`, newest first.
    pub events: Vec<(u64, Event)>,
    /// How many events the node has before the page's start (§18, A-78).
    pub total: Total,
}

/// The renderer's reads beside the state.
pub trait Source {
    /// An entry, if it exists.
    fn entry(&self, eid: Eid) -> Option<EntryView>;
    /// The reason of the pause at `scope` as shown: the text of its latest `System(Paused)` entry
    /// (D.4.6), or `[hidden: reason]` when that entry was hidden (§16).
    fn pause_reason(&self, scope: NodeId) -> Option<Text>;
    /// Why a node's file was hidden, if it was (§16).
    fn hidden_file(&self, node: NodeId, name: &str) -> Option<Text>;
    /// The entries of `node` with `after < seq ≤ upto`, oldest first, of `types` (every type when
    /// empty), at most `limit`; the total counts the matching entries after `after` (CS-17).
    fn feed(
        &self,
        node: NodeId,
        after: Seq,
        upto: Seq,
        types: &BTreeSet<EntryType>,
        limit: u32,
    ) -> EntryPage;
    /// The newest entries of `node`, newest first, at most `limit` (brief section 9).
    fn newest(&self, node: NodeId, limit: u32) -> Vec<EntryView>;
    /// The node's latest `System(Closed)` entry, however many entries followed it: its outcome
    /// and summary (§7's predecessors, the review view, and a take's `reopened: {was}`).
    fn close_entry(&self, node: NodeId) -> Option<EntryView>;
    /// The events of `node` with `seq < before`, newest first, at most `limit` (A-78).
    fn history(&self, node: NodeId, before: Option<u64>, limit: u32) -> EventPage;
    /// A page of the whole history for `control export` (D.11.15); None where the source holds
    /// no history to page, and the body then shows only the page asked for.
    fn export(&self, _after: Option<u64>, _limit: u32) -> Option<crate::export::ExportPage> {
        None
    }
    /// The reason of the routing of user entry `entry` to none (§8.2, D.6.2: "the shell renders
    /// `NoAction`'s reason from the routing event's text"), read from its `EntryRouted` event
    /// within `ROUTE_SCAN_MAX` events of the entry; None when there is none there.
    fn route_reason(&self, entry: UserEntryId) -> Option<Text>;
}

/// The reason of a routing to none, if `e` is that routing of `entry`.
fn routing_reason(e: &Event, entry: UserEntryId) -> Option<Text> {
    match &e.ev {
        Ev::EntryRouted {
            entry: x,
            routing: forum_core::Routing::NoAction,
            reason,
        } if *x == entry => Some(reason.clone().unwrap_or_default()),
        _ => None,
    }
}

/// The main text of `v` as shown: `[hidden: reason]` for a hidden entry (§16).
fn shown_main(v: &EntryView) -> Text {
    Text(super::entries::text_of(v, None))
}

/// The view of an `Appended` event, if it is one.
fn view_of(e: &Event, hidden: Option<Text>) -> Option<EntryView> {
    match &e.ev {
        Ev::Appended {
            eid,
            node,
            seq,
            by,
            body,
            text,
        } => Some(EntryView {
            eid: *eid,
            node: *node,
            seq: *seq,
            at: e.at,
            by: *by,
            body: body.clone(),
            text: text.clone(),
            hidden,
        }),
        _ => None,
    }
}

/// The user entry whose words a request carries (§8.2, 1.6): a request routed from a `from_user`
/// entry has no text of its own, since the core holds no feed text and routing records the request
/// by reference; its words are the entry's. None for any other entry, and for a request with text.
fn words_from(v: &EntryView) -> Option<UserEntryId> {
    match &v.body {
        EntryBody::Request {
            from_user: Some(e), ..
        } if v.text.main.is_empty() => Some(*e),
        _ => None,
    }
}

/// `v` with the words of its user entry `u` (§8.2, 1.6): the entry's text, hidden when the entry
/// is (§16). In the lab's first round, every request routed from a user's message showed `text:
/// ""` in briefs, feeds, the request board, and `next`, so a stranger saw who asked but not what.
fn with_words(mut v: EntryView, u: Option<EntryView>) -> EntryView {
    if let Some(u) = u {
        v.text.main = u.text.main;
        if v.hidden.is_none() {
            v.hidden = u.hidden;
        }
    }
    v
}

// ------------------------------------------------------------------------------------------------
// The store

/// The renderer's reads on a connection to the store (D.11.6): the `entries`, `pauses`, and
/// `hidden_files` tables and the events they point at. A read that fails reads as nothing: the
/// rendering of a committed call never fails (crash-only storage errors surface in the writer).
pub struct StoreSource<'c> {
    conn: &'c Connection,
}

impl<'c> StoreSource<'c> {
    /// Reads on `conn`.
    #[must_use]
    pub fn new(conn: &'c Connection) -> StoreSource<'c> {
        StoreSource { conn }
    }

    fn i(v: u64) -> i64 {
        i64::try_from(v).unwrap_or(i64::MAX)
    }

    fn u(v: i64) -> u64 {
        u64::try_from(v).unwrap_or(0)
    }

    /// The reason of the `Hidden` event at `ev`.
    fn hidden_reason(&self, ev: Option<i64>) -> Option<Text> {
        let ev = ev?;
        match event_at(self.conn, Self::u(ev)).ok()??.ev {
            Ev::Hidden { reason, .. } => Some(reason),
            _ => Some(Text::default()),
        }
    }

    /// The entries of rows `(ev, hidden_ev)`, a routed request with its user entry's words.
    fn views(&self, rows: Vec<(i64, Option<i64>)>) -> Vec<EntryView> {
        rows.into_iter()
            .filter_map(|(ev, hidden)| {
                let v = self.view(ev, hidden)?;
                Some(match words_from(&v) {
                    Some(e) => {
                        let u = self.row(e).and_then(|(ev, hidden)| self.view(ev, hidden));
                        with_words(v, u)
                    }
                    None => v,
                })
            })
            .collect()
    }

    /// The entry of row `(ev, hidden_ev)`, as stored.
    fn view(&self, ev: i64, hidden: Option<i64>) -> Option<EntryView> {
        let e = event_at(self.conn, Self::u(ev)).ok()??;
        view_of(&e, self.hidden_reason(hidden))
    }

    /// The row `(ev, hidden_ev)` of entry `eid`.
    fn row(&self, eid: Eid) -> Option<(i64, Option<i64>)> {
        self.conn
            .query_row(
                "SELECT ev, hidden_ev FROM entries WHERE eid = ?1",
                [Self::i(eid.0)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()?
    }

    fn rows(&self, sql: &str, p: impl rusqlite::Params) -> Vec<(i64, Option<i64>)> {
        let Ok(mut st) = self.conn.prepare_cached(sql) else {
            return Vec::new();
        };
        st.query_map(p, |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    fn count(&self, sql: &str, p: impl rusqlite::Params) -> u64 {
        self.conn
            .query_row(sql, p, |r| r.get::<_, i64>(0))
            .map(Self::u)
            .unwrap_or(0)
    }
}

impl Source for StoreSource<'_> {
    fn entry(&self, eid: Eid) -> Option<EntryView> {
        let row = self.row(eid);
        self.views(row.into_iter().collect()).pop()
    }

    fn pause_reason(&self, scope: NodeId) -> Option<Text> {
        let ev: i64 = self
            .conn
            .query_row(
                "SELECT ev FROM pauses WHERE scope = ?1",
                [Self::i(scope.0)],
                |r| r.get(0),
            )
            .optional()
            .ok()??;
        let e = event_at(self.conn, Self::u(ev)).ok()??;
        let v = view_of(&e, None)?;
        // The pause's entry may have been hidden since (§16): its reason is then not shown.
        let hidden: Option<i64> = self
            .conn
            .query_row(
                "SELECT hidden_ev FROM entries WHERE eid = ?1",
                [Self::i(v.eid.0)],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .flatten();
        let v = EntryView {
            hidden: self.hidden_reason(hidden),
            ..v
        };
        Some(shown_main(&v))
    }

    fn hidden_file(&self, node: NodeId, name: &str) -> Option<Text> {
        let ev: i64 = self
            .conn
            .query_row(
                "SELECT ev FROM hidden_files WHERE node = ?1 AND name = ?2",
                params![Self::i(node.0), name],
                |r| r.get(0),
            )
            .optional()
            .ok()??;
        self.hidden_reason(Some(ev))
    }

    fn feed(
        &self,
        node: NodeId,
        after: Seq,
        upto: Seq,
        types: &BTreeSet<EntryType>,
        limit: u32,
    ) -> EntryPage {
        let n = Self::i(node.0);
        if types.is_empty() {
            let rows = self.rows(
                "SELECT ev, hidden_ev FROM entries WHERE node = ?1 AND seq > ?2 AND seq <= ?3
                 ORDER BY seq LIMIT ?4",
                params![n, Self::i(after.0), Self::i(upto.0), limit],
            );
            let last: u64 = self.count(
                "SELECT coalesce(max(seq), 0) FROM entries WHERE node = ?1",
                [n],
            );
            let total = u32::try_from(last.saturating_sub(after.0)).unwrap_or(u32::MAX);
            return EntryPage {
                entries: self.views(rows),
                total: Total::Exact(total),
            };
        }
        let names: Vec<&str> = types.iter().map(|t| entry_type_name(*t)).collect();
        let list = names
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(",");
        let rows = self.rows(
            &format!(
                "SELECT ev, hidden_ev FROM entries WHERE node = ?1 AND seq > ?2 AND type IN ({list})
                 ORDER BY seq LIMIT ?3"
            ),
            params![n, Self::i(after.0), limit],
        );
        let counted = self.count(
            &format!(
                "SELECT count(*) FROM (SELECT 1 FROM entries WHERE node = ?1 AND seq > ?2
                 AND type IN ({list}) LIMIT ?3)"
            ),
            params![n, Self::i(after.0), SCAN_MAX + 1],
        );
        let total = if counted > u64::from(SCAN_MAX) {
            Total::AtLeast(SCAN_MAX)
        } else {
            Total::Exact(u32::try_from(counted).unwrap_or(u32::MAX))
        };
        EntryPage {
            entries: self.views(rows),
            total,
        }
    }

    fn newest(&self, node: NodeId, limit: u32) -> Vec<EntryView> {
        let rows = self.rows(
            "SELECT ev, hidden_ev FROM entries WHERE node = ?1 ORDER BY seq DESC LIMIT ?2",
            params![Self::i(node.0), limit],
        );
        self.views(rows)
    }

    fn close_entry(&self, node: NodeId) -> Option<EntryView> {
        // The node's system entries, newest first, read one at a time until the first close: the
        // cost is the system entries after it (hides, pauses, moves), with no window that a long
        // run of them could push the close out of.
        let mut st = self
            .conn
            .prepare_cached(
                "SELECT ev, hidden_ev FROM entries WHERE node = ?1 AND type = 'system'
                 ORDER BY seq DESC",
            )
            .ok()?;
        let rows = st
            .query_map(params![Self::i(node.0)], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
            })
            .ok()?;
        for (ev, hidden) in rows.flatten() {
            let Some(e) = event_at(self.conn, Self::u(ev)).ok().flatten() else {
                continue;
            };
            if let Ev::Appended {
                body: EntryBody::System(SystemNote::Closed { .. }),
                ..
            } = &e.ev
            {
                return view_of(&e, self.hidden_reason(hidden));
            }
        }
        None
    }

    fn history(&self, node: NodeId, before: Option<u64>, limit: u32) -> EventPage {
        let seqs = node_history(self.conn, node, before, limit).unwrap_or_default();
        let events = seqs
            .into_iter()
            .filter_map(|q| event_at(self.conn, q).ok().flatten().map(|e| (q, e)))
            .collect();
        let before = before.map_or(i64::MAX, Self::i);
        let counted = self.count(
            "SELECT count(*) FROM (SELECT 1 FROM event_nodes WHERE node = ?1 AND seq < ?2 LIMIT ?3)",
            params![Self::i(node.0), before, SCAN_MAX + 1],
        );
        let total = if counted > u64::from(SCAN_MAX) {
            Total::AtLeast(SCAN_MAX)
        } else {
            Total::Exact(u32::try_from(counted).unwrap_or(u32::MAX))
        };
        EventPage { events, total }
    }

    fn export(&self, after: Option<u64>, limit: u32) -> Option<crate::export::ExportPage> {
        crate::export::page(self.conn, after, limit).ok()
    }

    fn route_reason(&self, entry: UserEntryId) -> Option<Text> {
        let ev: i64 = self
            .conn
            .query_row(
                "SELECT ev FROM entries WHERE eid = ?1",
                [Self::i(entry.0)],
                |r| r.get(0),
            )
            .optional()
            .ok()??;
        let mut after = Self::u(ev);
        let mut read = 0_u32;
        while read < ROUTE_SCAN_MAX {
            let page = events_after(self.conn, after, ROUTE_SCAN_PAGE).ok()?;
            let last = page.last()?;
            after = last.seq;
            read = read.saturating_add(u32::try_from(page.len()).unwrap_or(u32::MAX));
            if let Some(r) = page.iter().find_map(|x| routing_reason(&x.event, entry)) {
                return Some(r);
            }
        }
        None
    }
}

// ------------------------------------------------------------------------------------------------
// In memory

/// The renderer's reads over a list of events held in memory: what a test, or a caller that holds
/// the events of a history, renders from.
#[derive(Clone, Debug, Default)]
pub struct MemorySource {
    /// `(seq, event)` in seq order.
    events: Vec<(u64, Event)>,
    entries: BTreeMap<Eid, usize>,
    hidden: BTreeMap<Eid, Text>,
    hidden_files: BTreeMap<(NodeId, String), Text>,
    pauses: BTreeMap<NodeId, usize>,
}

impl MemorySource {
    /// An empty source.
    #[must_use]
    pub fn new() -> MemorySource {
        MemorySource::default()
    }

    /// A source over `events`, in order.
    pub fn of<'a>(events: impl IntoIterator<Item = &'a Event>) -> MemorySource {
        let mut m = MemorySource::new();
        for e in events {
            m.push(e.clone());
        }
        m
    }

    /// Appends one event.
    pub fn push(&mut self, e: Event) {
        let i = self.events.len();
        let seq = u64::try_from(i).unwrap_or(u64::MAX).saturating_add(1);
        match &e.ev {
            Ev::Appended {
                eid, node, body, ..
            } => {
                self.entries.insert(*eid, i);
                if matches!(body, EntryBody::System(SystemNote::Paused { .. })) {
                    self.pauses.insert(*node, i);
                }
            }
            Ev::Hidden { target, reason } => match target {
                HideTarget::Entry(eid) => {
                    self.hidden.insert(*eid, reason.clone());
                }
                HideTarget::File { node, name } => {
                    self.hidden_files
                        .insert((*node, name.0.clone()), reason.clone());
                }
            },
            Ev::Unpaused { scope } | Ev::PauseExpired { scope } => {
                self.pauses.remove(scope);
            }
            _ => {}
        }
        self.events.push((seq, e));
    }

    /// The entry of event `i`, a routed request with its user entry's words (§8.2, 1.6).
    fn at(&self, i: usize) -> Option<EntryView> {
        let v = self.stored(i)?;
        Some(match words_from(&v) {
            Some(e) => {
                let u = self.entries.get(&e).and_then(|&j| self.stored(j));
                with_words(v, u)
            }
            None => v,
        })
    }

    /// The entry of event `i`, as stored.
    fn stored(&self, i: usize) -> Option<EntryView> {
        let (_, e) = self.events.get(i)?;
        let v = view_of(e, None)?;
        let hidden = self.hidden.get(&v.eid).cloned();
        Some(EntryView { hidden, ..v })
    }

    fn of_node(&self, node: NodeId) -> Vec<EntryView> {
        self.entries
            .values()
            .filter_map(|&i| self.at(i))
            .filter(|v| v.node == node)
            .collect()
    }
}

impl Source for MemorySource {
    fn entry(&self, eid: Eid) -> Option<EntryView> {
        self.at(*self.entries.get(&eid)?)
    }

    fn pause_reason(&self, scope: NodeId) -> Option<Text> {
        self.at(*self.pauses.get(&scope)?).map(|v| shown_main(&v))
    }

    fn hidden_file(&self, node: NodeId, name: &str) -> Option<Text> {
        self.hidden_files.get(&(node, name.to_string())).cloned()
    }

    fn feed(
        &self,
        node: NodeId,
        after: Seq,
        upto: Seq,
        types: &BTreeSet<EntryType>,
        limit: u32,
    ) -> EntryPage {
        let mut all: Vec<EntryView> = self
            .of_node(node)
            .into_iter()
            .filter(|v| v.seq > after)
            .filter(|v| types.is_empty() || types.contains(&v.body.entry_type()))
            .collect();
        all.sort_by_key(|v| v.seq);
        let count = all.len();
        let total = if types.is_empty() {
            let last = self
                .of_node(node)
                .iter()
                .map(|v| v.seq.0)
                .max()
                .unwrap_or(0);
            Total::Exact(u32::try_from(last.saturating_sub(after.0)).unwrap_or(u32::MAX))
        } else if count > usize::try_from(SCAN_MAX).unwrap_or(usize::MAX) {
            Total::AtLeast(SCAN_MAX)
        } else {
            Total::Exact(u32::try_from(count).unwrap_or(u32::MAX))
        };
        let entries = all
            .into_iter()
            .filter(|v| !types.is_empty() || v.seq <= upto)
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect();
        EntryPage { entries, total }
    }

    fn newest(&self, node: NodeId, limit: u32) -> Vec<EntryView> {
        let mut all = self.of_node(node);
        all.sort_by(|a, b| b.seq.cmp(&a.seq));
        all.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        all
    }

    fn close_entry(&self, node: NodeId) -> Option<EntryView> {
        self.newest(node, u32::MAX)
            .into_iter()
            .find(|v| matches!(v.body, EntryBody::System(SystemNote::Closed { .. })))
    }

    fn history(&self, node: NodeId, before: Option<u64>, limit: u32) -> EventPage {
        let mut named: Vec<(u64, Event)> = self
            .events
            .iter()
            .filter(|(q, e)| {
                before.is_none_or(|b| *q < b)
                    && crate::store::history::nodes_named(e).contains(&node)
            })
            .cloned()
            .collect();
        named.reverse();
        let total = Total::Exact(u32::try_from(named.len()).unwrap_or(u32::MAX));
        named.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        EventPage {
            events: named,
            total,
        }
    }

    fn route_reason(&self, entry: UserEntryId) -> Option<Text> {
        let from = *self.entries.get(&entry)?;
        self.events
            .iter()
            .skip(from.saturating_add(1))
            .take(usize::try_from(ROUTE_SCAN_MAX).unwrap_or(usize::MAX))
            .find_map(|(_, e)| routing_reason(e, entry))
    }
}
