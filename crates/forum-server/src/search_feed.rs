//! The search feeder (design D.11.2, D.11.12, CS-19): a thread with a read-only connection to the
//! forum's database that reads the committed events in `seq` order and feeds the search file. It
//! may lag the writer by seconds and never delays it; the writer only tells it, after each commit,
//! how far the history goes.
//!
//! What it indexes:
//! - one document per node, in four columns (1.9): its title; its path, read from `st_names`; its
//!   keys; and its text: its body and digest, read from its current content row in `st_nodes`,
//!   and the summary and learnings of its last close (the text of its latest `System(Closed)`
//!   entry, unless hidden). It is indexed again when the node is created, its title, body, or
//!   digest change, it closes, or an entry of it is hidden (the document then counts as of that
//!   time), and when a key is added or removed or it or an ancestor moves (it keeps its time);
//! - one document per people-facing entry (request, `said`, `heard`, question, answer, decision,
//!   `from_user`, `to_user`), its text read from its event; a hidden one is removed (contentless
//!   delete);
//! - per document: whether its node lies inside `kb/` (re-derived for a moved subtree), whether
//!   it is disputed (an open objection, or an open `conflict` signal, which the feeder tracks from
//!   the events in its own `disputes` table), and whether it was retracted (`retract {path}`);
//! - the words of every document in the vocabulary, which corrections read (1.9).
//!
//! It records the last `seq` it fed (`fed`), in the same transaction as the documents, so a
//! restart resumes there; a new or rebuilt search file starts at 0 and is refilled from the whole
//! history in the background (D.11.8 step 5), while search answers with what it has. The search
//! file names the forum it indexes (the forum's `forum_id`), so the file of another forum, or one
//! fed past the end of this forum's history, is rebuilt rather than reused.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use forum_core::{
    BasisTarget, EntryBody, Ev, HideTarget, Node, NodeId, ROOT, SignalKind, SystemNote, TextField,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::render::{Source, StoreSource};
use crate::search::{
    Columns, FORUM_ID, Indexer, fed, index_forum, open_index, reset_index, set_index_forum,
};
use crate::store::history::{StoredEvent, events_after, last_seq};
use crate::store::schema::get_meta;
use crate::store::shell_tables::entry_node;
use crate::store::{Result, StoreError};

/// Events fed per transaction of the search file.
const BATCH: u32 = 256;
/// How long the feeder sleeps when it has nothing to do before it looks at the history again.
const IDLE: Duration = Duration::from_secs(1);
/// The deepest tree the `kb/` walk follows.
const DEPTH_MAX: usize = 256;

/// `disputes.kind` of an open objection.
const OBJECTION: i64 = 0;
/// `disputes.kind` of an open conflict signal.
const CONFLICT: i64 = 1;

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// Whether an entry body is people-facing, and so indexed (D.11.12).
#[must_use]
pub fn indexed(body: &EntryBody) -> bool {
    matches!(
        body,
        EntryBody::Request { .. }
            | EntryBody::Said { .. }
            | EntryBody::Heard { .. }
            | EntryBody::Question(_)
            | EntryBody::Answer { .. }
            | EntryBody::Decision { .. }
            | EntryBody::FromUser { .. }
            | EntryBody::ToUser { .. }
    )
}

/// One batch's work on the search file.
struct Batch<'a> {
    idx: &'a Connection,
    main: &'a Connection,
    /// Writes the documents and their words.
    indexer: &'a mut Indexer,
    /// Nodes whose own document is indexed again: with the time of the last event that changed
    /// its content, or `None` when only its keys or its path changed, and it keeps its time.
    nodes: BTreeMap<NodeId, Option<u64>>,
    /// Nodes whose subtree moved.
    moved: BTreeSet<NodeId>,
    /// Nodes whose disputed or retracted marks may have changed.
    flags: BTreeSet<NodeId>,
    /// Paths read during the batch, each with whether its walk reached ROOT.
    paths: BTreeMap<NodeId, (String, bool)>,
}

impl Batch<'_> {
    /// `n`'s document is indexed again, as of `at` when its content changed.
    fn again(&mut self, n: NodeId, at: Option<u64>) {
        let e = self.nodes.entry(n).or_insert(at);
        if at.is_some() {
            *e = at;
        }
    }

    /// `n`'s path through live names, `/a/b`, and whether it reaches ROOT; `/` for ROOT, and the
    /// names up to the first without one for a node whose walk stops there.
    fn place(&mut self, n: NodeId) -> Result<(String, bool)> {
        if let Some(p) = self.paths.get(&n) {
            return Ok(p.clone());
        }
        let mut names = Vec::new();
        let mut rooted = n == ROOT;
        let mut cur = n;
        for _ in 0..DEPTH_MAX {
            let place: Option<(i64, String)> = self
                .main
                .query_row(
                    "SELECT parent, name FROM st_names WHERE node = ?1 AND alias = 0",
                    [i(cur.0)],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((parent, name)) = place else {
                break;
            };
            names.push(name);
            if u(parent) == ROOT.0 {
                rooted = true;
                break;
            }
            cur = NodeId(u(parent));
        }
        names.reverse();
        let p = (format!("/{}", names.join("/")), rooted);
        self.paths.insert(n, p.clone());
        Ok(p)
    }

    /// Whether `n` lies inside `kb/`: its path starts at the node `kb` under ROOT.
    fn under_kb(&mut self, n: NodeId) -> Result<bool> {
        let (p, rooted) = self.place(n)?;
        Ok(rooted && (p == "/kb" || p.starts_with("/kb/")))
    }

    fn disputed(&self, n: NodeId) -> Result<bool> {
        Ok(self.idx.query_row(
            "SELECT EXISTS (SELECT 1 FROM disputes WHERE node = ?1)",
            [i(n.0)],
            |r| r.get(0),
        )?)
    }

    fn retracted(&self, n: NodeId) -> Result<bool> {
        Ok(self.idx.query_row(
            "SELECT EXISTS (SELECT 1 FROM retracted_nodes WHERE node = ?1)",
            [i(n.0)],
            |r| r.get(0),
        )?)
    }

    /// Adds or replaces the document of `(node, eid)` with the columns `c`.
    fn put(&mut self, node: NodeId, eid: Option<u64>, at: u64, c: &Columns) -> Result<()> {
        let kb = self.under_kb(node)?;
        let disputed = self.disputed(node)?;
        let retracted = self.retracted(node)?;
        let existing: Option<i64> = match eid {
            Some(e) => self
                .idx
                .query_row("SELECT rowid FROM docs WHERE eid = ?1", [i(e)], |r| {
                    r.get(0)
                })
                .optional()?,
            None => self
                .idx
                .query_row(
                    "SELECT rowid FROM docs WHERE node = ?1 AND eid IS NULL",
                    [i(node.0)],
                    |r| r.get(0),
                )
                .optional()?,
        };
        let rowid = match existing {
            Some(r) => {
                self.idx
                    .execute("DELETE FROM search WHERE rowid = ?1", [r])?;
                self.idx.execute(
                    "UPDATE docs SET under_kb = ?2, at = ?3, disputed = ?4, retracted = ?5
                     WHERE rowid = ?1",
                    params![r, kb, i(at), disputed, retracted],
                )?;
                r
            }
            None => {
                self.idx.execute(
                    "INSERT INTO docs (node, eid, under_kb, at, disputed, retracted)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![i(node.0), eid.map(i), kb, i(at), disputed, retracted],
                )?;
                self.idx.last_insert_rowid()
            }
        };
        self.indexer.insert(self.idx, rowid, c)
    }

    /// Removes the document of entry `eid`, if it has one (a hidden entry, D.11.12).
    fn remove_entry(&self, eid: u64) -> Result<()> {
        let r: Option<i64> = self
            .idx
            .query_row("SELECT rowid FROM docs WHERE eid = ?1", [i(eid)], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(r) = r {
            self.idx
                .execute("DELETE FROM search WHERE rowid = ?1", [r])?;
            self.idx.execute("DELETE FROM docs WHERE rowid = ?1", [r])?;
        }
        Ok(())
    }

    fn event(&mut self, e: &StoredEvent) -> Result<()> {
        let at = e.event.at.0;
        match &e.event.ev {
            Ev::NodeCreated { id, .. } => self.again(*id, Some(at)),
            Ev::NodeUpdated { node, patch } => {
                if patch.title.is_some() || patch.body.is_some() || patch.digest.is_some() {
                    self.again(*node, Some(at));
                }
            }
            Ev::KeyAdded { node, .. } | Ev::KeyRemoved { node, .. } => self.again(*node, None),
            Ev::NodeMoved { node, .. } => {
                self.moved.insert(*node);
                self.paths.clear();
            }
            Ev::Appended {
                eid,
                node,
                body,
                text,
                ..
            } => match body {
                EntryBody::System(SystemNote::Closed { .. }) => self.again(*node, Some(at)),
                EntryBody::Object => {
                    self.idx.execute(
                        "INSERT OR IGNORE INTO disputes (kind, id, node) VALUES (?1, ?2, ?3)",
                        params![OBJECTION, i(eid.0), i(node.0)],
                    )?;
                    self.flags.insert(*node);
                }
                EntryBody::Retract {
                    target: BasisTarget::Node(t),
                } => {
                    self.idx.execute(
                        "INSERT OR IGNORE INTO retracted_nodes (node) VALUES (?1)",
                        [i(t.0)],
                    )?;
                    self.flags.insert(*t);
                }
                b if indexed(b) => {
                    self.put(*node, Some(eid.0), at, &Columns::entry(&text.main.0))?;
                }
                _ => {}
            },
            Ev::ObjectionResolved { o, .. } => {
                let n: Option<i64> = self
                    .idx
                    .query_row(
                        "SELECT node FROM disputes WHERE kind = ?1 AND id = ?2",
                        params![OBJECTION, i(o.0)],
                        |r| r.get(0),
                    )
                    .optional()?;
                self.idx.execute(
                    "DELETE FROM disputes WHERE kind = ?1 AND id = ?2",
                    params![OBJECTION, i(o.0)],
                )?;
                if let Some(n) = n {
                    self.flags.insert(NodeId(u(n)));
                }
            }
            Ev::ObjectionMoved { o, to } => {
                let n: Option<i64> = self
                    .idx
                    .query_row(
                        "SELECT node FROM disputes WHERE kind = ?1 AND id = ?2",
                        params![OBJECTION, i(o.0)],
                        |r| r.get(0),
                    )
                    .optional()?;
                self.idx.execute(
                    "UPDATE disputes SET node = ?3 WHERE kind = ?1 AND id = ?2",
                    params![OBJECTION, i(o.0), i(to.0)],
                )?;
                self.flags.insert(*to);
                if let Some(n) = n {
                    self.flags.insert(NodeId(u(n)));
                }
            }
            Ev::SignalOpened { key, .. } if key.kind == SignalKind::Conflict => {
                for (n, other) in [(key.a, key.b), (key.b, key.a)] {
                    self.idx.execute(
                        "INSERT OR IGNORE INTO disputes (kind, id, node) VALUES (?1, ?2, ?3)",
                        params![CONFLICT, i(other.0), i(n.0)],
                    )?;
                    self.flags.insert(n);
                }
            }
            Ev::SignalResolved { key } if key.kind == SignalKind::Conflict => {
                for (n, other) in [(key.a, key.b), (key.b, key.a)] {
                    self.idx.execute(
                        "DELETE FROM disputes WHERE kind = ?1 AND id = ?2 AND node = ?3",
                        params![CONFLICT, i(other.0), i(n.0)],
                    )?;
                    self.flags.insert(n);
                }
            }
            Ev::Hidden {
                target: HideTarget::Entry(eid),
                ..
            } => {
                self.remove_entry(eid.0)?;
                // The node's close summary may be the entry hidden.
                if let Some((n, _)) = entry_node(self.main, *eid)? {
                    self.again(n, Some(at));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Node `n`'s document and the time it was created, or None when it has no content row.
    fn node_doc(&mut self, n: NodeId) -> Result<Option<(Columns, u64)>> {
        let v: Option<String> = self
            .main
            .query_row("SELECT v FROM st_nodes WHERE id = ?1", [i(n.0)], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(v) = v else { return Ok(None) };
        let node: Node = serde_json::from_str(&v).map_err(|e| StoreError::Decode {
            table: "st_nodes",
            key: n.0.to_string(),
            msg: e.to_string(),
        })?;
        let close = StoreSource::new(self.main)
            .close_entry(n)
            .filter(|v| v.hidden.is_none());
        let summary = close.as_ref().map(|v| v.text.main.0.as_str()).unwrap_or("");
        let learnings = close
            .as_ref()
            .and_then(|v| {
                v.text
                    .extra
                    .iter()
                    .find(|(f, _)| *f == TextField::Learnings)
                    .map(|(_, t)| t.0.as_str())
            })
            .unwrap_or("");
        let (path, _) = self.place(n)?;
        // A content row holds no keys: they are rows of `st_keys`, a closed node's too.
        let keys: Vec<String> = self
            .main
            .prepare_cached("SELECT key FROM st_keys WHERE node = ?1 ORDER BY key")?
            .query_map([i(n.0)], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let c = Columns::node(
            &node.title.0,
            &path,
            keys.iter().map(String::as_str),
            &[&node.body.0, &node.digest.0, summary, learnings],
        );
        Ok(Some((c, node.created_at.0)))
    }

    /// The time of node `n`'s document, if it has one.
    fn doc_at(&self, n: NodeId) -> Result<Option<u64>> {
        let at: Option<i64> = self
            .idx
            .query_row(
                "SELECT at FROM docs WHERE node = ?1 AND eid IS NULL",
                [i(n.0)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.map(u))
    }

    /// The nodes of the subtree under `n`, `n` included, through live names.
    fn subtree(&self, n: NodeId) -> Result<Vec<NodeId>> {
        let mut out = Vec::new();
        let mut stack = vec![n];
        let mut st = self
            .main
            .prepare_cached("SELECT node FROM st_names WHERE parent = ?1 AND alias = 0")?;
        while let Some(x) = stack.pop() {
            out.push(x);
            let children: Vec<i64> = st
                .query_map([i(x.0)], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            stack.extend(children.into_iter().map(|c| NodeId(u(c))));
        }
        Ok(out)
    }

    /// The batch's deferred work: moved subtrees (their `kb/` membership, and their node
    /// documents, whose paths changed), node documents, and marks.
    fn finish(&mut self) -> Result<()> {
        let moved = std::mem::take(&mut self.moved);
        for n in moved {
            for x in self.subtree(n)? {
                let kb = self.under_kb(x)?;
                self.idx.execute(
                    "UPDATE docs SET under_kb = ?2 WHERE node = ?1",
                    params![i(x.0), kb],
                )?;
                self.again(x, None);
            }
        }
        let nodes = std::mem::take(&mut self.nodes);
        for (n, at) in nodes {
            if let Some((c, created)) = self.node_doc(n)? {
                let at = match at {
                    Some(at) => at,
                    None => self.doc_at(n)?.unwrap_or(created),
                };
                self.put(n, None, at, &c)?;
            }
        }
        let flags = std::mem::take(&mut self.flags);
        for n in flags {
            let disputed = self.disputed(n)?;
            let retracted = self.retracted(n)?;
            self.idx.execute(
                "UPDATE docs SET disputed = ?2, retracted = ?3 WHERE node = ?1",
                params![i(n.0), disputed, retracted],
            )?;
        }
        Ok(())
    }
}

/// Feeds the events after the search file's `fed`, at most `max`, in one transaction; returns the
/// new `fed`.
///
/// # Errors
///
/// A failed statement on either file, or a row that does not decode.
pub fn feed(idx: &Connection, main: &Connection, max: u32) -> Result<u64> {
    feed_with(idx, main, max, &mut Indexer::new()?)
}

/// [`feed`] with the feeder's `indexer`, which it keeps across batches.
fn feed_with(idx: &Connection, main: &Connection, max: u32, indexer: &mut Indexer) -> Result<u64> {
    let from = fed(idx)?;
    let events = events_after(main, from, max)?;
    let Some(last) = events.last().map(|e| e.seq) else {
        return Ok(from);
    };
    let tx = idx.unchecked_transaction()?;
    let mut b = Batch {
        idx: &tx,
        main,
        indexer,
        nodes: BTreeMap::new(),
        moved: BTreeSet::new(),
        flags: BTreeSet::new(),
        paths: BTreeMap::new(),
    };
    for e in &events {
        b.event(e)?;
    }
    b.finish()?;
    tx.execute("UPDATE fed SET seq = ?1 WHERE one = 0", [i(last)])?;
    tx.commit()?;
    Ok(last)
}

#[derive(Debug, Default)]
struct Progress {
    /// The last `seq` the writer committed, as far as the feeder knows.
    committed: u64,
    /// The last `seq` fed.
    fed: u64,
    /// Stop.
    stop: bool,
    /// The last error, for the log.
    error: Option<String>,
}

#[derive(Debug, Default)]
struct Shared {
    p: Mutex<Progress>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Progress> {
        match self.p.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The running feeder.
#[derive(Debug)]
pub struct Feeder {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Feeder {
    /// Opens (or creates, or rebuilds) the search file at `index` and starts the thread over the
    /// database at `db`, from the search file's `fed` to the end of the history. The search file
    /// names the forum it indexes; one of another forum, or one that has fed past the end of the
    /// history (a database restored from an older copy), is emptied and rebuilt, like one of
    /// another version (D.11.8 step 5; review E4-spec-6).
    ///
    /// # Errors
    ///
    /// When either file cannot be opened.
    pub fn start(db: &Path, index: &Path) -> Result<Feeder> {
        let main = Connection::open_with_flags(
            db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let forum = get_meta(&main, FORUM_ID)?;
        let committed = last_seq(&main)?;
        let (idx, created) = open_index(index)?;
        let theirs = index_forum(&idx)?;
        let why = if created {
            Some("the search file is new")
        } else if forum.is_some() && theirs != forum {
            Some("the search file indexes another forum")
        } else if fed(&idx)? > committed {
            Some("the search file is ahead of the history")
        } else {
            None
        };
        if let Some(why) = why {
            if !created {
                reset_index(&idx, forum.as_deref())?;
            } else if let Some(f) = &forum {
                set_index_forum(&idx, f)?;
            }
            tracing::info!(path = %index.display(), "{why}: rebuilding it from the history");
        }
        let shared = Arc::new(Shared::default());
        {
            let mut p = shared.lock();
            p.fed = fed(&idx)?;
            p.committed = committed;
        }
        let s = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("search-feeder".into())
            .spawn(move || run(&s, &idx, &main))
            .map_err(StoreError::Io)?;
        Ok(Feeder {
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// The writer committed the history up to `seq`.
    pub fn committed(&self, seq: u64) {
        let mut p = self.shared.lock();
        if seq > p.committed {
            p.committed = seq;
            self.shared.cv.notify_all();
        }
    }

    /// The last `seq` fed.
    #[must_use]
    pub fn fed(&self) -> u64 {
        self.shared.lock().fed
    }

    /// Waits until the feeder has fed `seq`, at most `timeout`; whether it has.
    #[must_use]
    pub fn wait_fed(&self, seq: u64, timeout: Duration) -> bool {
        let mut p = self.shared.lock();
        let mut left = timeout;
        while p.fed < seq {
            let start = crate::clock::Mono::now();
            let (g, r) = match self.shared.cv.wait_timeout(p, left) {
                Ok(x) => x,
                Err(poisoned) => poisoned.into_inner(),
            };
            p = g;
            if r.timed_out() {
                return p.fed >= seq;
            }
            left = left.saturating_sub(crate::clock::Mono::now().since(start));
        }
        true
    }

    /// Stops the thread and waits for it.
    pub fn stop(&self) {
        {
            let mut p = self.shared.lock();
            p.stop = true;
            self.shared.cv.notify_all();
        }
        let t = match self.thread.lock() {
            Ok(mut g) => g.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(t) = t {
            let _ = t.join();
        }
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(shared: &Shared, idx: &Connection, main: &Connection) {
    let mut indexer = match Indexer::new() {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!(error = %e, "the search feeder cannot tokenize; search finds nothing new");
            return;
        }
    };
    loop {
        {
            let mut p = shared.lock();
            while !p.stop && p.fed >= p.committed {
                let (g, r) = match shared.cv.wait_timeout(p, IDLE) {
                    Ok(x) => x,
                    Err(poisoned) => poisoned.into_inner(),
                };
                p = g;
                if r.timed_out() {
                    break;
                }
            }
            if p.stop {
                return;
            }
        }
        match feed_with(idx, main, BATCH, &mut indexer) {
            Ok(seq) => {
                let mut p = shared.lock();
                if seq == p.fed {
                    // Nothing new is visible: wait for the writer's next word, or the next look.
                    p.committed = seq;
                }
                p.fed = seq;
                p.error = None;
                shared.cv.notify_all();
            }
            Err(e) => {
                // The failed transaction's words never reached the vocabulary.
                indexer.forget();
                let msg = e.to_string();
                let mut p = shared.lock();
                if p.error.as_deref() != Some(msg.as_str()) {
                    tracing::warn!(error = %msg, "the search feeder failed; it retries");
                }
                p.error = Some(msg);
                drop(p);
                std::thread::sleep(IDLE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Hits;
    use crate::search::{Query, Searcher};
    use crate::store::TxKind;
    use crate::store::testkit::{TempDb, appended, create_node, init, with_session};
    use forum_core::{Eid, Name, Text};
    use std::collections::BTreeSet;

    fn ids(h: &Hits) -> Vec<u64> {
        h.nodes.iter().map(|n| n.0).collect()
    }

    fn titled(t: &str) -> forum_core::NodeInit {
        let mut i = init(1, &[]);
        i.title = Text(t.into());
        i.body = Text::default();
        i
    }

    fn decision() -> EntryBody {
        EntryBody::Decision {
            settles: BTreeSet::new(),
            supersedes: None,
            assumes: BTreeSet::new(),
            evidence: None,
            basis: Vec::new(),
        }
    }

    #[test]
    fn the_feeder_indexes_nodes_and_people_facing_entries_and_tracks_their_marks() {
        let (mut w, s) = with_session();
        let kb = create_node(&mut w, ROOT, "kb", s, titled("knowledge"));
        let lemma = create_node(&mut w, kb, "lemma", s, titled("zstd lemma"));
        let work = create_node(&mut w, ROOT, "work", s, titled("work"));
        let gap = create_node(&mut w, work, "gap", s, titled("summary gap"));
        let other = create_node(&mut w, work, "other", s, titled("zstd tuning"));
        // A decision on `gap` is indexed; a plain note is not.
        let ev = appended(&w, gap, s, decision(), "we chose zstd for the gap");
        w.tx(TxKind::Cmd, vec![ev]);
        let decided = Eid(w.s.counters.next_eid.0 - 1);
        let note = EntryBody::Note {
            to: None,
            until: None,
            recipients: BTreeSet::new(),
            pending_holder: false,
            expects: None,
            reply_to: None,
        };
        let ev = appended(&w, other, s, note, "zstd zstd zstd in a plain note");
        w.tx(TxKind::Cmd, vec![ev]);
        // An objection disputes `gap`.
        let ev = appended(&w, gap, s, EntryBody::Object, "the gap is not fixed");
        w.tx(TxKind::Cmd, vec![ev]);
        let file = TempDb::new("feed");
        let (idx, _) = open_index(&file.path).unwrap();
        let fed_to = feed(&idx, &w.conn, 1_000).unwrap();
        assert_eq!(fed_to, last_seq(&w.conn).unwrap());
        let search = Searcher::new(&file.path);
        let h = search.search(&Query::Hello("zstd".into()));
        assert_eq!(ids(&h.related), [other.0, gap.0], "the disputed node last");
        assert_eq!(ids(&h.knowledge), [lemma.0]);
        let entries: i64 = idx
            .query_row("SELECT count(*) FROM docs WHERE eid IS NOT NULL", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            entries, 1,
            "the decision only: notes and objections are not indexed"
        );
        // Hiding the decision removes its document (contentless delete); retracting the lemma
        // drops it from knowledge; moving `other` into kb/ makes it knowledge.
        w.tx(
            TxKind::Cmd,
            vec![Ev::Hidden {
                target: HideTarget::Entry(decided),
                reason: Text("tidy".into()),
            }],
        );
        let retract = EntryBody::Retract {
            target: BasisTarget::Node(lemma),
        };
        let ev = appended(&w, lemma, s, retract, "refuted");
        w.tx(TxKind::Cmd, vec![ev]);
        w.tx(
            TxKind::Cmd,
            vec![Ev::NodeMoved {
                node: other,
                parent: kb,
                name: Name::new("tuning").unwrap(),
            }],
        );
        feed(&idx, &w.conn, 1_000).unwrap();
        let h = search.search(&Query::Hello("zstd".into()));
        assert_eq!(
            ids(&h.knowledge),
            [other.0],
            "the retracted lemma is left out, the moved node is in"
        );
        assert!(
            ids(&h.related).is_empty(),
            "the decision's document is gone"
        );
        let h = search.search(&Query::Find("zstd".into()));
        assert_eq!(
            ids(&h.related),
            [other.0, lemma.0],
            "the retracted node last"
        );
        // Feeding again with nothing new changes nothing.
        assert_eq!(
            feed(&idx, &w.conn, 1_000).unwrap(),
            last_seq(&w.conn).unwrap()
        );
    }

    #[test]
    fn the_feeder_thread_follows_the_history_and_resumes_after_a_restart() {
        let db = TempDb::new("feeder-db");
        let index = TempDb::new("feeder-index");
        let o = crate::store::testkit::fresh_file(&db, crate::store::testkit::test_params());
        let mut w = crate::store::testkit::World::from_opened(o);
        let s = w.s.counters.next_session;
        let ev = crate::store::testkit::session_opened(&w, "coder");
        w.tx(TxKind::Cmd, vec![ev]);
        create_node(&mut w, ROOT, "alpha", s, titled("quaternion alpha"));
        let f = Feeder::start(&db.path, &index.path).unwrap();
        let end = last_seq(&w.conn).unwrap();
        f.committed(end);
        assert!(f.wait_fed(end, Duration::from_secs(10)));
        let found = Searcher::new(&index.path).search(&Query::Find("quaternion".into()));
        assert_eq!(found.related.nodes.len(), 1);
        f.stop();
        // More history while it is stopped: a new feeder resumes from `fed`.
        create_node(&mut w, ROOT, "beta", s, titled("quaternion beta"));
        let f = Feeder::start(&db.path, &index.path).unwrap();
        let end = last_seq(&w.conn).unwrap();
        assert!(f.wait_fed(end, Duration::from_secs(10)));
        let found = Searcher::new(&index.path).search(&Query::Find("quaternion".into()));
        assert_eq!(found.related.nodes.len(), 2);
    }
}
