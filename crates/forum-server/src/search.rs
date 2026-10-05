//! Text search (design D.11.12, CS-19, CS-37, §18): the search file and its bounded query.
//!
//! The search file is an SQLite file of its own, beside the forum's: a contentless FTS5 index
//! (it stores no copy of a text; the renderer reads texts from the forum) of one document per node
//! and one per people-facing entry (request, `said`, `heard`, question, answer, decision,
//! `from_user`, `to_user`), and the vocabulary of the words indexed. A node's document has four
//! columns (1.9): its title; its path, whose segments the tokenizer cuts at `/ - _ .`; its keys,
//! cut into their scheme's and their value's words; and its text, the body, the digest, and its
//! last close's summary and learnings. An entry's document has its text alone. The tokenizer stems
//! the words `unicode61` cuts (Porter), in the index and in the query alike, so that `resizing`,
//! `resize`, and `resized` are one term. The feeder (`search_feed`) fills the file from the
//! committed events and may lag the writer by seconds.
//!
//! A query runs in the tool handler before its job is queued, on a pool of read-only connections to
//! the search file, never on the writer. Its text is cut into words by the index's tokenizer
//! without stemming (an in-memory FTS5 table with the same options, so that the words are exactly
//! the index's); the [`STOP_WORDS`] are dropped; the first [`TERMS_LOOKED_UP`] distinct words left
//! are stemmed, and those of the same stem are one term. The [`QUERY_TERMS`] terms with the lowest
//! non-zero document frequency, and the next rarest while their lists are short (up to
//! [`QUERY_TERMS_MAX`], [`LISTED_CHEAP`]), are quoted and joined with `OR`, and, for `view find
//! {text}` alone ([`Query::corrects`]), while there is room among them, a term no document has (a
//! typo or a variant: at most [`FUZZY_TERMS`] of them) is replaced by the closest word of the
//! vocabulary that some document has ([`Slot::correct`]); matches are ordered by disputed or
//! retracted last, then `bm25` with the columns' [`WEIGHTS`], then newest first. A frequency is
//! counted in the index up to [`RANKED_MAX`], and a count of [`FREQ_KEPT_MIN`] or more is reused
//! for [`FREQ_FRESH`] by every connection of the pool. One deadline, [`SEARCH_BUDGET`] after the
//! call starts, bounds the whole search: the frequency lookups, the corrections, and the ranked
//! query stop a tenth of the budget before it (a progress handler and a check between lookups stop
//! them), and the ranked query is not tried at all when its terms' lists hold [`RANKED_MAX`]
//! documents or more, since `bm25` alone reads them in full before it ranks a match. Then the
//! search falls back to the rarest term alone, newest first, which reads one page of one list in
//! the tenth kept for it, and marks the result partial (`≥k`). Search never routes, joins, or
//! merges anything (§18): its hits travel with the job for rendering only, and never enter a
//! command.
//!
//! The ranked query serves every scope of a call at once (`hello`'s two read the lists once), and
//! keeps each scope's best matches in a bounded heap of its own rather than in an SQL sort: the
//! bundled SQLite gives all the connections of the process one page cache under one mutex, which
//! every page read and the temporary B-tree of an `ORDER BY … LIMIT` take, so that four and ten
//! concurrent worst-case searches each ran five and twenty-five times slower than one alone
//! (gate G-9). (A memory map would keep the page reads off that mutex too, but each connection's
//! map counts its pages again in the server's resident memory, which gate G-6 bounds.)
//!
//! What a query returns: `hello`, [`RELATED_MAX`] `related` (outside `kb/`) and
//! [`KNOWLEDGE_MAX`] `knowledge` (inside `kb/`, without disputed or retracted nodes);
//! `from_user`, [`RELATED_MAX`] `related`; a `create`, [`RELATED_MAX`] `related` for its title and
//! body (rendered only when it creates, CS-37); `view find {text}`, up to [`FIND_MAX`] matches
//! anywhere, in `related`, saying which words it corrected (`matched resizing for resizng`).
//! No scope holds the user log, `@user`, `@user/decisions`, and `@user/digest` (1.6, §18): the
//! human's message that a `hello` or `from_user` searches for matched itself there, and crowded
//! out the node its work lives on.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use forum_core::{
    Cmd, FindBy, NodeCmd, NodeId, SessionCmd, TalkCmd, USER, USER_DECISIONS, USER_DIGEST, ViewCmd,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::clock::Mono;
use crate::render::{Hits, SearchHits};
use crate::store::Result;

/// The version of the search file's schema: a file of another version is rebuilt (D.11.8 step 5).
/// 2 (1.9): a node's columns, the stemming tokenizer, and the vocabulary.
pub const SEARCH_VERSION: u32 = 2;
/// A query's time budget (D.11.12).
pub const SEARCH_BUDGET: Duration = Duration::from_millis(50);
/// Terms a query keeps whatever their lists: the rarest (D.11.12), 4 as in 1.8. A query whose
/// rarest lists are long reads no more than 1.8's did: a floor of 6 (1.9's first draft) bought
/// nothing on the recall set that [`QUERY_TERMS_MAX`] does not, and made gate G-9's cheap search
/// fetch 3,110 pages rather than 2,653.
pub const QUERY_TERMS: usize = 4;
/// Terms a query keeps at most: past its [`QUERY_TERMS`] rarest, the next rarest are kept too
/// while their lists, all the terms' together, hold fewer than [`LISTED_CHEAP`] documents (1.9).
/// In a small forum, most of a query's words are each in a document or two, and the rarest were
/// its phrasing and filler (`about`, `owner`, `priya`, `16`; `agent`, `asked`, `last`, `week`),
/// not its subject (`postgres`, `upgrade`; `refund retries`, in 10 and 4 documents: the lab's it3
/// l3); BM25 weighs every word kept by its rarity. On the recall set, the 4 rarest alone find 25
/// of 28 nodes, the 6 rarest 27, and either with these 28.
pub const QUERY_TERMS_MAX: usize = 12;
/// Documents of the kept terms' lists below which a query keeps more than its rarest
/// [`QUERY_TERMS`]: every match costs the ranked query a row of `docs` and the document's size,
/// about 40 µs at the 99th percentile with ten concurrent searches (gate G-9's smoke run, through
/// the shared page cache), so a few hundred, which a small forum's words do not reach. A query that
/// keeps more than 4 terms reads fewer matches than this.
pub const LISTED_CHEAP: u32 = 500;
/// Distinct words of a query text, other than stop words, that are looked up at all: a 64 KiB
/// body has thousands, and a lookup walks the term's document list, so its cost grows with the
/// corpus; the rarest terms among the first 32 bound it, within the query's deadline (review
/// E4-spec-7).
pub const TERMS_LOOKED_UP: usize = 32;
/// Documents of its terms' lists a ranked query may read: `bm25` reads each term's whole list
/// before it ranks the first match (to weigh the term), and ranking reads every match, so past
/// this many the query would not end within the budget (about 0.2 µs a document on a laptop core,
/// more under load), and the search falls back at once. A frequency is counted up to it, no
/// further: a full count of a common term reads its whole list, uninterrupted, at any size of
/// the index (review G-9).
pub const RANKED_MAX: u32 = 100_000;
/// How long a frequency counted is reused, by every connection of the pool: the same common words
/// recur in most queries, and theirs are the costly counts; the index lags the writer by seconds
/// anyway (review G-9).
pub const FREQ_FRESH: Duration = Duration::from_secs(60);
/// A frequency below this is counted afresh each time: the count costs little, and a new word is
/// found at once.
pub const FREQ_KEPT_MIN: u32 = 1_000;
/// Frequencies kept at most.
const FREQ_KEPT_MAX: usize = 4_096;
/// The `meta` key of the forum a search file indexes (review E4-spec-6), and of the forum's own
/// identity in the forum's database.
pub const FORUM_ID: &str = "forum_id";
/// `related` hits of `hello`, `from_user`, and `create`.
pub const RELATED_MAX: usize = 3;
/// `knowledge` hits of `hello`.
pub const KNOWLEDGE_MAX: usize = 3;
/// `view find {text}` hits.
pub const FIND_MAX: usize = 20;
/// Rows a query reads at most while it collects distinct nodes.
const ROWS_MAX: u32 = 1_000;
/// The tokenizer of the index (D.11.12): `unicode61` cuts words at every character that is not a
/// letter or a digit, folds case and diacritics, and Porter stems them (1.9).
pub const TOKENIZE: &str = "porter unicode61 remove_diacritics 2";
/// The index's tokenizer without stemming: it cuts a query into the words that stop words and
/// corrections are matched against, and the documents into the words of the vocabulary.
pub const WORDS_TOKENIZE: &str = "unicode61 remove_diacritics 2";
/// The `bm25` weights of a node document's columns, in their order: text 1, title 6, path 4, and
/// keys 2 (1.9). A word of the title says more of what the node is than one of its body, where it may be an aside; the
/// path names the work too, but its parents' segments are shared by their whole subtree; a key's
/// words are mostly its scheme and an id. On the lab's recall set (`search/recall.rs`) every
/// choice finds all 28 nodes in the top 3 (or 20), but the order differs: with equal weights the
/// person `/@people/priya-raman` and the parent `/platform` come before `/platform/pg16-upgrade`
/// in 4 searches (mean reciprocal rank 0.875); a title weight of 4 leaves 1 of them (0.929), and
/// of 6 or more none (0.946, the only second places those of searches that want two nodes),
/// with the path's weight from 1 to 6 and the keys' from 1 to 4 (keys as heavy as the title lose
/// one again: 0.929). An entry's document has its text alone.
///
/// The text is the first column: a position list marks each change of column, from the first,
/// and most of a document's words are its text's. Last, it took gate G-9's smoke index of 50,000
/// nodes from 10.3 MB (1.8's, one column) to 17.6; first, to 12.8, the rest the path's words.
pub const WEIGHTS: [f64; 4] = [1.0, 6.0, 4.0, 2.0];
/// Words a query drops before it chooses its rarest terms (1.9): English function words, and the
/// filler of chat and of agents' intents. A rare one outranked the words of the subject: the lab's
/// `hello` kept `about` and dropped `postgres` and `upgrade` (it3 l3), and a human's `morning!`
/// matched `every monday morning` (it3 l2). Sorted, so that a binary search finds a word.
pub const STOP_WORDS: &[&str] = &[
    "a",
    "about",
    "above",
    "actually",
    "after",
    "afternoon",
    "again",
    "ago",
    "ahead",
    "all",
    "already",
    "also",
    "although",
    "always",
    "am",
    "an",
    "and",
    "another",
    "any",
    "anyone",
    "anything",
    "anyway",
    "are",
    "aren",
    "around",
    "as",
    "at",
    "be",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "both",
    "btw",
    "but",
    "by",
    "can",
    "cannot",
    "cheers",
    "cool",
    "could",
    "couldn",
    "d",
    "did",
    "didn",
    "do",
    "does",
    "doesn",
    "doing",
    "don",
    "during",
    "each",
    "eg",
    "either",
    "else",
    "etc",
    "even",
    "evening",
    "ever",
    "every",
    "few",
    "for",
    "from",
    "fyi",
    "get",
    "gets",
    "go",
    "going",
    "good",
    "got",
    "great",
    "had",
    "hadn",
    "has",
    "hasn",
    "have",
    "haven",
    "having",
    "he",
    "hello",
    "her",
    "here",
    "hers",
    "herself",
    "hey",
    "hi",
    "him",
    "himself",
    "his",
    "how",
    "however",
    "i",
    "ie",
    "if",
    "in",
    "into",
    "is",
    "isn",
    "it",
    "its",
    "itself",
    "just",
    "kindly",
    "later",
    "let",
    "lets",
    "ll",
    "lot",
    "m",
    "maybe",
    "me",
    "might",
    "mine",
    "more",
    "morning",
    "most",
    "much",
    "must",
    "my",
    "myself",
    "nice",
    "no",
    "nor",
    "not",
    "now",
    "of",
    "off",
    "oh",
    "ok",
    "okay",
    "on",
    "once",
    "only",
    "onto",
    "or",
    "other",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "perhaps",
    "please",
    "pls",
    "quite",
    "re",
    "really",
    "s",
    "same",
    "shall",
    "she",
    "should",
    "shouldn",
    "since",
    "so",
    "some",
    "someone",
    "something",
    "soon",
    "still",
    "such",
    "sure",
    "t",
    "than",
    "thank",
    "thanks",
    "that",
    "the",
    "their",
    "theirs",
    "them",
    "themselves",
    "then",
    "there",
    "these",
    "they",
    "thing",
    "things",
    "this",
    "those",
    "though",
    "through",
    "thx",
    "to",
    "today",
    "tomorrow",
    "tonight",
    "too",
    "under",
    "until",
    "up",
    "upon",
    "us",
    "ve",
    "very",
    "via",
    "was",
    "wasn",
    "we",
    "well",
    "were",
    "weren",
    "what",
    "whatever",
    "when",
    "where",
    "whether",
    "which",
    "while",
    "who",
    "whom",
    "whose",
    "why",
    "will",
    "with",
    "within",
    "without",
    "won",
    "would",
    "wouldn",
    "yeah",
    "yep",
    "yes",
    "yet",
    "you",
    "your",
    "yours",
    "yourself",
    "yourselves",
];
/// Query words that no document has which are corrected, at most, in a `view find {text}` (1.9,
/// [`Query::corrects`]): each costs a few lookups of the vocabulary and of frequencies.
pub const FUZZY_TERMS: usize = 2;
/// The shortest word corrected: a word of 3 letters is 1 edit away from many others (`rss`,
/// `css`, `rsa`), so its correction would be a guess.
pub const FUZZY_MIN: usize = 4;
/// From this many letters on, a correction may be 2 edits away; below it, 1.
pub const FUZZY_FAR: usize = 5;
/// Words of the vocabulary read per lookup: one lookup reads the words of one length that begin,
/// or end, with the query word's first, or last, two letters, so a correction costs at most
/// `2 × 5` lookups of this many rows.
pub const FUZZY_ROWS: u32 = 500;
/// Words at the closest distance whose frequency is looked up, at most.
const FUZZY_CANDIDATES: usize = 8;
/// The shortest and the longest word the vocabulary holds.
const WORD_LEN: (usize, usize) = (3, 32);
/// Words the feeder remembers having put in the vocabulary, at most: a word already there costs
/// it no statement.
const WORDS_KNOWN_MAX: usize = 65_536;

/// The search file's tables (D.11.12), and the feeder's own bookkeeping: `disputes` (the open
/// objections and conflict signals that make a node disputed) and `retracted_nodes`. `words` is
/// the vocabulary (1.9): every word of 3 to 32 letters, without a digit, that is no stop word, of
/// every document ever indexed, by its first and its last two letters and its length, the
/// candidates of a correction. It only grows: a word whose documents are gone is found by a
/// correction with no document, and passed over.
pub const SCHEMA: &str = "
CREATE VIRTUAL TABLE search USING fts5(text, title, path, keys, content = '',
                     contentless_delete = 1, tokenize = 'porter unicode61 remove_diacritics 2');
CREATE TABLE docs (rowid INTEGER PRIMARY KEY, node INTEGER NOT NULL,
                   eid INTEGER,
                   under_kb INTEGER NOT NULL, at INTEGER NOT NULL,
                   disputed INTEGER NOT NULL DEFAULT 0,
                   retracted INTEGER NOT NULL DEFAULT 0);
CREATE UNIQUE INDEX docs_entry ON docs(eid) WHERE eid IS NOT NULL;
CREATE UNIQUE INDEX docs_node ON docs(node) WHERE eid IS NULL;
CREATE INDEX docs_by_node ON docs(node);
CREATE TABLE words (word TEXT PRIMARY KEY, head TEXT NOT NULL, tail TEXT NOT NULL,
                    len INTEGER NOT NULL) WITHOUT ROWID;
CREATE INDEX words_head ON words(head, len);
CREATE INDEX words_tail ON words(tail, len);
CREATE TABLE fed (one INTEGER PRIMARY KEY CHECK (one = 0), seq INTEGER NOT NULL);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE disputes (kind INTEGER NOT NULL, id INTEGER NOT NULL, node INTEGER NOT NULL,
                       PRIMARY KEY (kind, id, node)) WITHOUT ROWID;
CREATE INDEX disputes_node ON disputes(node);
CREATE TABLE retracted_nodes (node INTEGER PRIMARY KEY);
";

/// What a call searches for, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// `session hello`: its intent and the human's message.
    Hello(String),
    /// `talk from_user`: the human's message.
    FromUser(String),
    /// `node create`: its title and body.
    Create(String),
    /// `view find {text}`.
    Find(String),
}

impl Query {
    /// The query of a command, if it searches (D.8.1).
    #[must_use]
    pub fn of(cmd: &Cmd) -> Option<Query> {
        let text = |parts: &[Option<&str>]| {
            let t = parts
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            (!t.trim().is_empty()).then_some(t)
        };
        match cmd {
            Cmd::Session(SessionCmd::Hello {
                intent, from_user, ..
            }) => text(&[
                intent.as_ref().map(|t| t.0.as_str()),
                from_user.as_ref().map(|f| f.text.0.as_str()),
            ])
            .map(Query::Hello),
            Cmd::Talk(TalkCmd::FromUser { text: t, .. }) => {
                text(&[Some(t.0.as_str())]).map(Query::FromUser)
            }
            Cmd::Node(NodeCmd::Create(a)) => text(&[
                Some(a.title.0.as_str()),
                a.body.as_ref().map(|b| b.0.as_str()),
            ])
            .map(Query::Create),
            Cmd::View(ViewCmd::Find {
                by: FindBy::Text(t),
            }) => text(&[Some(t.0.as_str())]).map(Query::Find),
            _ => None,
        }
    }

    fn text(&self) -> &str {
        match self {
            Query::Hello(t) | Query::FromUser(t) | Query::Create(t) | Query::Find(t) => t,
        }
    }

    /// Whether the search corrects a word that no document has (1.9): only `view find {text}`,
    /// whose text an agent typed as a query. A `hello`'s intent and human's message, a
    /// `from_user`'s message, and a `create`'s title and body are prose, whose ordinary words a
    /// correction rewrote into others (lab, it4 l6).
    #[must_use]
    pub fn corrects(&self) -> bool {
        matches!(self, Query::Find(_))
    }
}

/// Whether `w`, a word as the tokenizer cuts it, is a stop word.
#[must_use]
pub fn stop_word(w: &str) -> bool {
    STOP_WORDS.binary_search(&w).is_ok()
}

/// Whether the vocabulary holds `w`: 3 to 32 letters, no digit, and no stop word. A word with a
/// digit is a number, a version, or an id, which a correction would turn into another (`pg17`
/// into `pg16`).
fn vocabulary_word(w: &str) -> bool {
    let n = w.chars().count();
    (WORD_LEN.0..=WORD_LEN.1).contains(&n) && w.chars().all(char::is_alphabetic) && !stop_word(w)
}

/// A word's first and last two letters, and its length in letters: its places in the vocabulary.
fn shape(w: &str) -> (String, String, usize) {
    let cs: Vec<char> = w.chars().collect();
    let head = cs.iter().take(2).collect();
    let tail = cs[cs.len().saturating_sub(2)..].iter().collect();
    (head, tail, cs.len())
}

/// The optimal string alignment distance of `a` and `b`: insertions, deletions, substitutions, and
/// swaps of two adjacent letters, each one edit; `None` when it exceeds `max`.
fn distance(a: &[char], b: &str, max: usize) -> Option<usize> {
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let m = b.len();
    let mut before: Vec<usize> = vec![0; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur: Vec<usize> = vec![0; m + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut low = i;
        for j in 1..=m {
            let same = a[i - 1] == b[j - 1];
            let mut d = (prev[j] + 1)
                .min(cur[j - 1] + 1)
                .min(prev[j - 1] + usize::from(!same));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(before[j - 2] + 1);
            }
            cur[j] = d;
            low = low.min(d);
        }
        if low > max {
            return None;
        }
        std::mem::swap(&mut before, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[m] <= max).then_some(prev[m])
}

/// A document's columns (1.9, D.11.12): a node's title, path, keys, and text; an entry's text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Columns {
    /// The node's title.
    pub title: String,
    /// The node's path, `/a/b-c`: the tokenizer cuts it into its segments' words.
    pub path: String,
    /// The node's keys, one a line: the tokenizer cuts each into its scheme's and value's words.
    pub keys: String,
    /// A node's body, digest, and last close's summary and learnings; an entry's text.
    pub text: String,
}

impl Columns {
    /// A node's document: its title, path, keys, and its `texts` (the body, the digest, the last
    /// close's summary and learnings), those that are not empty one a line.
    #[must_use]
    pub fn node<'a>(
        title: &str,
        path: &str,
        keys: impl IntoIterator<Item = &'a str>,
        texts: &[&str],
    ) -> Columns {
        Columns {
            title: title.to_owned(),
            path: path.to_owned(),
            keys: keys.into_iter().collect::<Vec<_>>().join("\n"),
            text: texts
                .iter()
                .filter(|t| !t.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// An entry's document: its text alone.
    #[must_use]
    pub fn entry(text: &str) -> Columns {
        Columns {
            text: text.to_owned(),
            ..Columns::default()
        }
    }
}

/// Writes documents into the search file, and their new words into its vocabulary (D.11.12): the
/// feeder's, kept across its batches, with the words it put there lately.
pub struct Indexer {
    /// An in-memory table with [`WORDS_TOKENIZE`], which cuts a document into its words.
    tok: Connection,
    /// Words the vocabulary holds, at most [`WORDS_KNOWN_MAX`].
    known: BTreeSet<String>,
}

impl Indexer {
    /// An indexer with its tokenizer.
    ///
    /// # Errors
    ///
    /// When the in-memory tokenizer cannot be made.
    pub fn new() -> Result<Indexer> {
        let tok = Connection::open_in_memory()?;
        tok.execute_batch(&format!(
            "CREATE VIRTUAL TABLE w USING fts5(t, tokenize = '{WORDS_TOKENIZE}');
             CREATE VIRTUAL TABLE w_v USING fts5vocab(w, 'row');"
        ))?;
        Ok(Indexer {
            tok,
            known: BTreeSet::new(),
        })
    }

    /// Forgets the words it put in the vocabulary: a transaction that put them was rolled back.
    pub fn forget(&mut self) {
        self.known.clear();
    }

    /// Indexes document `rowid` of the search file `idx` with its columns `c`, and adds their
    /// words to the vocabulary.
    ///
    /// # Errors
    ///
    /// A failed statement.
    pub fn insert(&mut self, idx: &Connection, rowid: i64, c: &Columns) -> Result<()> {
        idx.prepare_cached(
            "INSERT INTO search (rowid, text, title, path, keys) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![rowid, c.text, c.title, c.path, c.keys])?;
        let all = [&c.title, &c.path, &c.keys, &c.text]
            .iter()
            .filter(|t| !t.is_empty())
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.tok.execute("INSERT INTO w (t) VALUES (?1)", [all])?;
        let words: Vec<String> = self
            .tok
            .prepare_cached("SELECT term FROM w_v")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        self.tok.execute("DELETE FROM w", [])?;
        let mut put = idx.prepare_cached(
            "INSERT OR IGNORE INTO words (word, head, tail, len) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for w in words {
            if !vocabulary_word(&w) || self.known.contains(&w) {
                continue;
            }
            let (head, tail, len) = shape(&w);
            put.execute(params![w, head, tail, i64::try_from(len).unwrap_or(0)])?;
            if self.known.len() >= WORDS_KNOWN_MAX {
                self.known.clear();
            }
            self.known.insert(w);
        }
        Ok(())
    }
}

/// The nodes of the user log, which no scope holds (1.6, §18): `view user_log` and `view
/// requests` read them.
const USER_LOG: [NodeId; 3] = [USER, USER_DECISIONS, USER_DIGEST];

/// Whether a document's node is outside the user log, which no scope holds (1.6).
fn searched(n: NodeId) -> bool {
    !USER_LOG.contains(&n)
}

/// The SQL condition of [`searched`] on `d.node`.
fn searched_sql() -> String {
    let ids: Vec<String> = USER_LOG.iter().map(|n| n.0.to_string()).collect();
    format!("d.node NOT IN ({})", ids.join(", "))
}

/// Where a search looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// Outside `kb/`.
    Related,
    /// Inside `kb/`, without disputed or retracted nodes.
    Knowledge,
    /// Anywhere.
    Anywhere,
}

impl Scope {
    /// Whether the scope holds a document, from its columns of `docs`.
    fn sql(self) -> &'static str {
        match self {
            Scope::Related => "d.under_kb = 0",
            Scope::Knowledge => "(d.under_kb = 1 AND d.disputed = 0 AND d.retracted = 0)",
            Scope::Anywhere => "1",
        }
    }

    /// Whether the scope holds a document inside `kb/` or not, disputed or retracted or not.
    fn holds(self, under_kb: bool, bad: bool) -> bool {
        match self {
            Scope::Related => !under_kb,
            Scope::Knowledge => under_kb && !bad,
            Scope::Anywhere => true,
        }
    }
}

/// The scopes a query searches, each with the distinct nodes it shows at most.
fn scopes(q: &Query) -> &'static [(Scope, usize)] {
    match q {
        Query::Hello(_) => &[
            (Scope::Related, RELATED_MAX),
            (Scope::Knowledge, KNOWLEDGE_MAX),
        ],
        Query::FromUser(_) | Query::Create(_) => &[(Scope::Related, RELATED_MAX)],
        Query::Find(_) => &[(Scope::Anywhere, FIND_MAX)],
    }
}

/// One match of a ranked query, ordered as D.11.12 orders matches: disputed or retracted last,
/// then by `bm25` (lower is better), then newest first, then the later document first.
#[derive(Clone, Copy, Debug)]
struct Match {
    bad: bool,
    rank: f64,
    at: i64,
    doc: i64,
    node: NodeId,
}

impl Ord for Match {
    fn cmp(&self, o: &Match) -> Ordering {
        self.bad
            .cmp(&o.bad)
            .then(self.rank.total_cmp(&o.rank))
            .then(o.at.cmp(&self.at))
            .then(o.doc.cmp(&self.doc))
    }
}

impl PartialOrd for Match {
    fn partial_cmp(&self, o: &Match) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl PartialEq for Match {
    fn eq(&self, o: &Match) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Match {}

/// The best [`ROWS_MAX`] matches of one scope (the worst of them on top of the heap), and how
/// many it was offered: the rows an `ORDER BY … LIMIT` would return, kept without SQLite's sort.
struct Best {
    k: usize,
    heap: BinaryHeap<Match>,
    offered: u32,
}

impl Best {
    fn new(k: usize) -> Best {
        Best {
            k,
            heap: BinaryHeap::new(),
            offered: 0,
        }
    }

    fn offer(&mut self, m: Match) {
        self.offered = self.offered.saturating_add(1);
        if self.heap.len() < ROWS_MAX as usize {
            self.heap.push(m);
        } else if self.heap.peek().is_some_and(|worst| m < *worst) {
            self.heap.pop();
            self.heap.push(m);
        }
    }

    fn hits(self) -> Hits {
        let truncated = self.offered >= ROWS_MAX;
        let mut d = Distinct::new(self.k);
        for m in self.heap.into_sorted_vec() {
            if d.add(m.node) {
                break;
            }
        }
        d.hits(truncated)
    }
}

/// The first `k` distinct nodes of a scope's rows, in order, and whether another follows them.
struct Distinct {
    k: usize,
    nodes: Vec<NodeId>,
    more: bool,
}

impl Distinct {
    fn new(k: usize) -> Distinct {
        Distinct {
            k,
            nodes: Vec::new(),
            more: false,
        }
    }

    /// Takes the next row's node; whether the scope has seen all it shows, and one more.
    fn add(&mut self, n: NodeId) -> bool {
        if !self.more && !self.nodes.contains(&n) {
            if self.nodes.len() == self.k {
                self.more = true;
            } else {
                self.nodes.push(n);
            }
        }
        self.more
    }

    /// The hits; partial when another node follows them, or when the rows were `truncated`.
    fn hits(self, truncated: bool) -> Hits {
        Hits {
            nodes: self.nodes,
            partial: self.more || truncated,
        }
    }
}

fn node(v: i64) -> NodeId {
    NodeId(u64::try_from(v).unwrap_or(0))
}

/// Whether a statement failed because the progress handler stopped it.
fn interrupted(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _)
        if f.code == rusqlite::ErrorCode::OperationInterrupted)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Document frequencies counted lately, shared by the pool (D.11.12): each a count of at least
/// [`FREQ_KEPT_MIN`], reused for [`FREQ_FRESH`]; at most [`FREQ_KEPT_MAX`] of them. By stem: the
/// words of one stem are one term.
#[derive(Debug, Default)]
struct Freq(Mutex<BTreeMap<String, (u32, Mono)>>);

impl Freq {
    fn get(&self, term: &str) -> Option<u32> {
        let now = Mono::now();
        lock(&self.0)
            .get(term)
            .filter(|(_, at)| now.since(*at) < FREQ_FRESH)
            .map(|(n, _)| *n)
    }

    fn put(&self, term: &str, n: u32) {
        if n < FREQ_KEPT_MIN {
            return;
        }
        let now = Mono::now();
        let mut m = lock(&self.0);
        if m.len() >= FREQ_KEPT_MAX && !m.contains_key(term) {
            m.retain(|_, (_, at)| now.since(*at) < FREQ_FRESH);
            if m.len() >= FREQ_KEPT_MAX {
                return;
            }
        }
        m.insert(term.to_owned(), (n, now));
    }
}

/// Opens (or creates) the search file at `path` for the feeder: WAL, the schema of
/// [`SEARCH_VERSION`]. A file of another version is emptied and created afresh, and its `fed` is
/// 0, so the feeder rebuilds it from the history (D.11.8 step 5). Returns the connection and
/// whether the file was (re)created.
///
/// # Errors
///
/// When the file cannot be opened or written.
pub fn open_index(path: &Path) -> Result<(Connection, bool)> {
    let conn = Connection::open(path)?;
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'search_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap_or(None);
    if version.as_deref() == Some(SEARCH_VERSION.to_string().as_str()) {
        return Ok((conn, false));
    }
    reset_index(&conn, None)?;
    Ok((conn, true))
}

/// Empties the search file and creates its schema afresh, of [`SEARCH_VERSION`], for the forum
/// `forum` when given; its `fed` is 0, so the feeder rebuilds it from the history (D.11.8 step 5).
///
/// # Errors
///
/// When the file cannot be written.
pub fn reset_index(conn: &Connection, forum: Option<&str>) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    // The vocabulary table of an older file first, then the index (which drops its shadow
    // tables), then the rest.
    for t in [
        "search_vocab",
        "search",
        "docs",
        "words",
        "fed",
        "meta",
        "disputes",
        "retracted_nodes",
    ] {
        tx.execute_batch(&format!("DROP TABLE IF EXISTS {t}"))?;
    }
    tx.execute_batch(SCHEMA)?;
    tx.execute("INSERT INTO fed (one, seq) VALUES (0, 0)", [])?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('search_version', ?1)",
        [SEARCH_VERSION.to_string()],
    )?;
    if let Some(f) = forum {
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            [FORUM_ID, f],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// The forum the search file indexes, if it names one.
///
/// # Errors
///
/// A failed query.
pub fn index_forum(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [FORUM_ID], |r| {
            r.get(0)
        })
        .optional()?)
}

/// Records that the search file indexes the forum `forum`.
///
/// # Errors
///
/// A failed statement.
pub fn set_index_forum(conn: &Connection, forum: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [FORUM_ID, forum],
    )?;
    Ok(())
}

/// The last `seq` the feeder fed into the search file.
///
/// # Errors
///
/// A failed query.
pub fn fed(conn: &Connection) -> Result<u64> {
    let v: i64 = conn.query_row("SELECT seq FROM fed WHERE one = 0", [], |r| r.get(0))?;
    Ok(u64::try_from(v).unwrap_or(0))
}

/// A term of a query: its stem, the index's term, which says whether two words are one term and
/// keys its frequency; and the first word of the query with that stem, which the query matches
/// (the index's tokenizer stems it again, so a stem itself may not be the index's term).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    stem: String,
    word: String,
}

/// A term the query keeps: its document frequency, its stem, the word matched, and, for a
/// correction, the query's word it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pick {
    df: u32,
    stem: String,
    word: String,
    for_word: Option<String>,
}

impl Pick {
    fn exact(df: u32, t: &Term) -> Pick {
        Pick {
            df,
            stem: t.stem.clone(),
            word: t.word.clone(),
            for_word: None,
        }
    }
}

/// One connection of the pool: the search file, read-only, and the tokenizers.
struct Slot {
    db: Connection,
    /// In-memory tables: `raw` with [`WORDS_TOKENIZE`] and `stem` with [`TOKENIZE`].
    tok: Connection,
}

impl Slot {
    fn open(path: &Path) -> Result<Slot> {
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let tok = Connection::open_in_memory()?;
        tok.execute_batch(&format!(
            "CREATE VIRTUAL TABLE raw USING fts5(t, tokenize = '{WORDS_TOKENIZE}');
             CREATE VIRTUAL TABLE raw_v USING fts5vocab(raw, 'instance');
             CREATE VIRTUAL TABLE stem USING fts5(t, tokenize = '{TOKENIZE}');
             CREATE VIRTUAL TABLE stem_v USING fts5vocab(stem, 'instance');"
        ))?;
        Ok(Slot { db, tok })
    }

    /// The terms of `text`: its distinct words as the index's tokenizer cuts them, in order of
    /// first occurrence, without stop words, at most [`TERMS_LOOKED_UP`]; stemmed, the words of
    /// one stem one term.
    fn terms(&self, text: &str) -> Result<Vec<Term>> {
        self.tok.execute("DELETE FROM raw", [])?;
        self.tok
            .execute("INSERT INTO raw (t) VALUES (?1)", [text])?;
        let all: Vec<String> = self
            .tok
            .prepare_cached("SELECT term FROM raw_v ORDER BY offset")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut seen = BTreeSet::new();
        let words: Vec<String> = all
            .into_iter()
            .filter(|w| !stop_word(w) && seen.insert(w.clone()))
            .take(TERMS_LOOKED_UP)
            .collect();
        let stems = self.stems(&words)?;
        let mut seen = BTreeSet::new();
        Ok(words
            .into_iter()
            .zip(stems)
            .filter(|(_, s)| seen.insert(s.clone()))
            .map(|(word, stem)| Term { stem, word })
            .collect())
    }

    /// The stems of `words`, in order: each word stemmed by the index's tokenizer, all in one
    /// row (a word as the tokenizer cut it is one token), or one row each should their tokens
    /// not line up.
    fn stems(&self, words: &[String]) -> Result<Vec<String>> {
        self.tok.execute("DELETE FROM stem", [])?;
        self.tok.execute(
            "INSERT INTO stem (rowid, t) VALUES (0, ?1)",
            [words.join(" ")],
        )?;
        let one: Vec<String> = self
            .tok
            .prepare_cached("SELECT term FROM stem_v ORDER BY offset")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        if one.len() == words.len() {
            return Ok(one);
        }
        self.tok.execute("DELETE FROM stem", [])?;
        let mut by_doc: BTreeMap<i64, String> = BTreeMap::new();
        {
            let mut put = self
                .tok
                .prepare_cached("INSERT INTO stem (rowid, t) VALUES (?1, ?2)")?;
            for (i, w) in (1_i64..).zip(words) {
                put.execute(params![i, w])?;
            }
            let mut st = self
                .tok
                .prepare_cached("SELECT doc, term FROM stem_v ORDER BY doc, offset")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                by_doc.entry(r.get(0)?).or_insert(r.get(1)?);
            }
        }
        Ok((1_i64..)
            .zip(words)
            .map(|(i, w)| by_doc.remove(&i).unwrap_or_else(|| w.clone()))
            .collect())
    }

    /// The document frequency of `word`'s term, counted in the index up to [`RANKED_MAX`]; `None`
    /// when the progress handler stopped the count.
    fn count(&self, word: &str) -> Result<Option<u32>> {
        let mut st = self.db.prepare_cached(
            "SELECT count(*) FROM (SELECT 1 FROM search WHERE search MATCH ?1 LIMIT ?2)",
        )?;
        match st.query_row(params![quote(word), RANKED_MAX], |r| r.get::<_, i64>(0)) {
            Ok(n) => Ok(Some(u32::try_from(n).unwrap_or(RANKED_MAX))),
            Err(e) if interrupted(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The frequency of `stem`, written `word`: reused from `freq`, or counted and kept there.
    fn frequency(&self, stem: &str, word: &str, freq: &Freq) -> Result<Option<u32>> {
        if let Some(n) = freq.get(stem) {
            return Ok(Some(n));
        }
        let n = self.count(word)?;
        if let Some(n) = n {
            freq.put(stem, n);
        }
        Ok(n)
    }

    /// The terms the query keeps, rarest first, each with its frequency (counted up to
    /// [`RANKED_MAX`], or reused from `freq`), among those looked up before `deadline`: the
    /// [`QUERY_TERMS`] with the lowest non-zero frequency, then, when the query `corrects` and
    /// while there is room, the corrections of terms with none, at most [`FUZZY_TERMS`], in the
    /// query's order. A count reads the term's list, so it is under the query's deadline too, and
    /// stops there with the terms looked up so far, the first always (review E4-spec-7), its
    /// frequency then taken as [`RANKED_MAX`].
    fn rarest(
        &self,
        terms: &[Term],
        corrects: bool,
        freq: &Freq,
        deadline: Mono,
    ) -> Result<Vec<Pick>> {
        // Setting a handler fails only on a closed connection, whose query then fails too.
        let _ = self
            .db
            .progress_handler(1_000, Some(move || Mono::now() > deadline));
        let out = (|| -> Result<Vec<Pick>> {
            let mut picks: Vec<Pick> = Vec::new();
            let mut none: Vec<&Term> = Vec::new();
            for (i, t) in terms.iter().enumerate() {
                if i > 0 && Mono::now() > deadline {
                    break;
                }
                match self.frequency(&t.stem, &t.word, freq)? {
                    Some(0) => none.push(t),
                    Some(n) => picks.push(Pick::exact(n, t)),
                    None => {
                        // The fallback needs a term.
                        if i == 0 {
                            picks.push(Pick::exact(RANKED_MAX, t));
                        }
                        break;
                    }
                }
            }
            picks.sort_by(|a, b| a.df.cmp(&b.df).then_with(|| a.stem.cmp(&b.stem)));
            let mut listed = 0_u32;
            let kept = picks
                .iter()
                .enumerate()
                .take_while(|(i, p)| {
                    listed = listed.saturating_add(p.df);
                    *i < QUERY_TERMS || listed < LISTED_CHEAP
                })
                .count();
            picks.truncate(kept.min(QUERY_TERMS_MAX));
            let fuzzy = if corrects { FUZZY_TERMS } else { 0 };
            for t in none.into_iter().take(fuzzy) {
                if picks.len() >= QUERY_TERMS || Mono::now() > deadline {
                    break;
                }
                let taken: BTreeSet<&str> = picks.iter().map(|p| p.stem.as_str()).collect();
                if let Some(p) = self.correct(t, &taken, freq, deadline)? {
                    picks.push(p);
                }
            }
            Ok(picks)
        })();
        let _ = self.db.progress_handler(0, None::<fn() -> bool>);
        out
    }

    /// The correction of `t`, a term no document has (1.9): the word of the vocabulary closest to
    /// its word, at most 2 edits away for a word of [`FUZZY_FAR`] letters or more and 1 below,
    /// whose term some document has and is not `taken`; the closest first, then the one in more
    /// documents, then the first in order. A word of fewer than [`FUZZY_MIN`] letters, or with a
    /// digit, is not corrected. The candidates are the words of each length within reach that
    /// begin with the word's first two letters, or end with its last two, [`FUZZY_ROWS`] of
    /// each at most, so a typo in both the first and the last two letters is not corrected.
    /// Before `deadline`; `None` past it.
    fn correct(
        &self,
        t: &Term,
        taken: &BTreeSet<&str>,
        freq: &Freq,
        deadline: Mono,
    ) -> Result<Option<Pick>> {
        let w: Vec<char> = t.word.chars().collect();
        let n = w.len();
        if n < FUZZY_MIN || n > WORD_LEN.1 + 2 || !w.iter().all(|c| c.is_alphabetic()) {
            return Ok(None);
        }
        let far = if n >= FUZZY_FAR { 2 } else { 1 };
        let (head, tail, _) = shape(&t.word);
        let mut near: BTreeSet<(usize, String)> = BTreeSet::new();
        let read = (|| -> rusqlite::Result<()> {
            let mut by_head = self
                .db
                .prepare_cached("SELECT word FROM words WHERE head = ?1 AND len = ?2 LIMIT ?3")?;
            let mut by_tail = self
                .db
                .prepare_cached("SELECT word FROM words WHERE tail = ?1 AND len = ?2 LIMIT ?3")?;
            for len in n.saturating_sub(far).max(WORD_LEN.0)..=n + far {
                let len = i64::try_from(len).unwrap_or(0);
                for (st, key) in [(&mut by_head, &head), (&mut by_tail, &tail)] {
                    if Mono::now() > deadline {
                        return Ok(());
                    }
                    let mut rows = st.query(params![key, len, FUZZY_ROWS])?;
                    while let Some(r) = rows.next()? {
                        let v: String = r.get(0)?;
                        if let Some(d) = distance(&w, &v, far).filter(|d| *d > 0) {
                            near.insert((d, v));
                        }
                    }
                }
            }
            Ok(())
        })();
        match read {
            Err(e) if interrupted(&e) => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok(()) => {}
        }
        for d in 1..=far {
            let words: Vec<String> = near
                .iter()
                .filter(|(k, _)| *k == d)
                .map(|(_, v)| v.clone())
                .take(FUZZY_CANDIDATES)
                .collect();
            if words.is_empty() {
                continue;
            }
            let stems = self.stems(&words)?;
            let mut best: Option<Pick> = None;
            for (v, s) in words.into_iter().zip(stems) {
                if taken.contains(s.as_str()) || Mono::now() > deadline {
                    continue;
                }
                let Some(df) = self.frequency(&s, &v, freq)? else {
                    break;
                };
                if df > 0 && best.as_ref().is_none_or(|b| df > b.df) {
                    best = Some(Pick {
                        df,
                        stem: s,
                        word: v,
                        for_word: Some(t.word.clone()),
                    });
                }
            }
            if best.is_some() {
                return Ok(best);
            }
        }
        Ok(None)
    }

    /// The ranked query over the match expression `m`, for every scope of the call in one reading
    /// of the lists, until `deadline`: each scope's matches in D.11.12's order, its first `k`
    /// distinct nodes of the best [`ROWS_MAX`]; `None` when the deadline stopped it.
    fn ranked(&self, m: &str, scopes: &[(Scope, usize)], deadline: Mono) -> Option<Vec<Hits>> {
        // Setting a handler fails only on a closed connection, whose query then fails too.
        let _ = self
            .db
            .progress_handler(1_000, Some(move || Mono::now() > deadline));
        let out = (|| -> rusqlite::Result<Vec<Hits>> {
            let within = scopes
                .iter()
                .map(|(s, _)| s.sql())
                .collect::<Vec<_>>()
                .join(" OR ");
            let [text, title, path, keys] = WEIGHTS;
            let mut st = self.db.prepare_cached(&format!(
                "SELECT d.node, d.under_kb, d.disputed OR d.retracted,
                        bm25(search, {text:?}, {title:?}, {path:?}, {keys:?}), d.at, d.rowid
                 FROM search JOIN docs d ON d.rowid = search.rowid
                 WHERE search MATCH ?1 AND ({within}) AND {}",
                searched_sql()
            ))?;
            let mut best: Vec<Best> = scopes.iter().map(|(_, k)| Best::new(*k)).collect();
            let mut rows = st.query([m])?;
            while let Some(r) = rows.next()? {
                let (under_kb, bad) = (r.get(1)?, r.get(2)?);
                let m = Match {
                    bad,
                    rank: r.get(3)?,
                    at: r.get(4)?,
                    doc: r.get(5)?,
                    node: node(r.get(0)?),
                };
                for ((s, _), b) in scopes.iter().zip(best.iter_mut()) {
                    if s.holds(under_kb, bad) {
                        b.offer(m);
                    }
                }
            }
            Ok(best.into_iter().map(Best::hits).collect())
        })();
        let _ = self.db.progress_handler(0, None::<fn() -> bool>);
        out.ok()
    }

    /// The fallback: the newest page of `word`'s term's list, at most [`ROWS_MAX`] documents
    /// newest first (the index's own order, no sort), read until each scope has its first `k`
    /// distinct nodes and knows whether another follows; partial (`≥k`). It reads that page
    /// whatever the deadline: a page is its bound.
    fn newest(&self, word: &str, scopes: &[(Scope, usize)]) -> Vec<Hits> {
        let mut picks: Vec<Distinct> = scopes.iter().map(|(_, k)| Distinct::new(*k)).collect();
        // A failed read finds what it found: search is advisory.
        let _ = (|| -> rusqlite::Result<()> {
            let mut st = self.db.prepare_cached(
                "SELECT d.node, d.under_kb, d.disputed OR d.retracted
                 FROM (SELECT rowid FROM search WHERE search MATCH ?1
                       ORDER BY rowid DESC LIMIT ?2) s
                 JOIN docs d ON d.rowid = s.rowid
                 ORDER BY s.rowid DESC",
            )?;
            let mut rows = st.query(params![quote(word), ROWS_MAX])?;
            while let Some(r) = rows.next()? {
                let (n, under_kb, bad) = (node(r.get(0)?), r.get(1)?, r.get(2)?);
                if !searched(n) {
                    continue;
                }
                let mut done = true;
                for ((s, _), p) in scopes.iter().zip(picks.iter_mut()) {
                    let full = if s.holds(under_kb, bad) {
                        p.add(n)
                    } else {
                        p.more
                    };
                    done &= full;
                }
                if done {
                    break;
                }
            }
            Ok(())
        })();
        picks.into_iter().map(|p| p.hits(true)).collect()
    }

    /// The hits of the terms `picks` (rarest first) in each of `scopes`: the ranked query until
    /// `deadline`, unless their lists hold [`RANKED_MAX`] documents or more or the deadline has
    /// passed; else the fallback, one page of the rarest term's list. With the corrections the
    /// query used, when it found anything.
    fn find(
        &self,
        picks: &[Pick],
        scopes: &[(Scope, usize)],
        deadline: Mono,
    ) -> (Vec<Hits>, Vec<String>) {
        let Some(first) = picks.first() else {
            return (vec![Hits::default(); scopes.len()], Vec::new());
        };
        let matched = |used: &[Pick]| -> Vec<String> {
            used.iter()
                .filter_map(|p| p.for_word.as_ref().map(|f| format!("{} for {f}", p.word)))
                .collect()
        };
        let listed: u64 = picks.iter().map(|p| u64::from(p.df)).sum();
        let (hits, mut used) = 'h: {
            if listed < u64::from(RANKED_MAX) && Mono::now() < deadline {
                let m = picks
                    .iter()
                    .map(|p| quote(&p.word))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                if let Some(h) = self.ranked(&m, scopes, deadline) {
                    break 'h (h, matched(picks));
                }
            }
            (
                self.newest(&first.word, scopes),
                matched(std::slice::from_ref(first)),
            )
        };
        if hits.iter().all(|h| h.nodes.is_empty()) {
            used.clear();
        }
        (hits, used)
    }
}

/// An FTS5 string: the term in double quotes, so agents cannot use FTS syntax.
fn quote(t: &str) -> String {
    format!("\"{}\"", t.replace('"', "\"\""))
}

/// The pool of read-only connections to the search file (D.11.2), and the frequencies they
/// counted lately.
pub struct Searcher {
    path: PathBuf,
    pool: Mutex<Vec<Slot>>,
    freq: Freq,
    budget: Duration,
}

impl Searcher {
    /// A pool over the search file at `path` (which the feeder has created).
    #[must_use]
    pub fn new(path: &Path) -> Searcher {
        Searcher::with_budget(path, SEARCH_BUDGET)
    }

    /// A pool whose searches have `budget` (tests set one no query meets).
    #[must_use]
    pub fn with_budget(path: &Path, budget: Duration) -> Searcher {
        Searcher {
            path: path.to_path_buf(),
            pool: Mutex::new(Vec::new()),
            freq: Freq::default(),
            budget,
        }
    }

    fn take(&self) -> Result<Slot> {
        let slot = lock(&self.pool).pop();
        match slot {
            Some(s) => Ok(s),
            None => Slot::open(&self.path),
        }
    }

    fn give(&self, s: Slot) {
        lock(&self.pool).push(s);
    }

    /// The hits of `q` (D.11.12). One deadline, the budget after the call starts, bounds the
    /// whole call: the rarest terms, looked up once, their corrections, and the ranked query, one
    /// for all the call's scopes, stop a tenth of the budget before it; the fallback, one page of
    /// one list, takes that tenth (reviews E4-spec-7, G-9). A search that fails (a missing or
    /// damaged file) finds nothing: search is advisory (§18).
    #[must_use]
    pub fn search(&self, q: &Query) -> SearchHits {
        let ranked_by = Mono::now().plus(self.budget.saturating_sub(self.budget / 10));
        let Ok(slot) = self.take() else {
            return SearchHits::default();
        };
        let picks = slot
            .terms(q.text())
            .and_then(|t| slot.rarest(&t, q.corrects(), &self.freq, ranked_by))
            .unwrap_or_default();
        let (hits, matched) = slot.find(&picks, scopes(q), ranked_by);
        self.give(slot);
        let mut hits = hits.into_iter();
        let related = hits.next().unwrap_or_default();
        let knowledge = hits.next().unwrap_or_default();
        SearchHits {
            related,
            knowledge,
            matched,
        }
    }
}

#[cfg(test)]
mod recall;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testkit::TempDb;

    /// A document: `(node, eid, under_kb, at, disputed, retracted, text)`.
    type Doc<'a> = (u64, Option<u64>, bool, u64, bool, bool, &'a str);

    /// Indexes `docs` directly, their text in the text column.
    fn index(conn: &Connection, docs: &[Doc<'_>]) {
        for (node, eid, kb, at, disputed, retracted, text) in docs {
            conn.execute(
                "INSERT INTO docs (node, eid, under_kb, at, disputed, retracted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    *node as i64,
                    eid.map(|e| e as i64),
                    *kb,
                    *at as i64,
                    *disputed,
                    *retracted
                ],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO search (rowid, text) VALUES (?1, ?2)",
                params![rowid, text],
            )
            .unwrap();
        }
    }

    /// Indexes node documents with their columns, and their words in the vocabulary, as the
    /// feeder does: `(node, at, columns)`.
    fn index_columns(conn: &Connection, docs: &[(u64, u64, Columns)]) {
        let mut ix = Indexer::new().unwrap();
        for (node, at, c) in docs {
            conn.execute(
                "INSERT INTO docs (node, eid, under_kb, at) VALUES (?1, NULL, 0, ?2)",
                params![*node as i64, *at as i64],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            ix.insert(conn, rowid, c).unwrap();
        }
    }

    fn nodes(h: &Hits) -> Vec<u64> {
        h.nodes.iter().map(|n| n.0).collect()
    }

    fn picked(p: &[Pick]) -> Vec<(u32, &str)> {
        p.iter().map(|p| (p.df, p.word.as_str())).collect()
    }

    fn pick(df: u32, word: &str) -> Pick {
        Pick {
            df,
            stem: word.to_owned(),
            word: word.to_owned(),
            for_word: None,
        }
    }

    #[test]
    fn a_query_is_an_or_over_its_rarest_terms() {
        let db = TempDb::new("search-rarest");
        let (conn, created) = open_index(&db.path).unwrap();
        assert!(created);
        // "summary" is everywhere, "gap" in two documents, "zstd" and "Navier" in one each.
        let mut docs = Vec::new();
        for i in 0..30 {
            docs.push((
                100 + i,
                None,
                false,
                i,
                false,
                false,
                "the summary of a node",
            ));
        }
        docs.push((
            10,
            None,
            false,
            50,
            false,
            false,
            "summary gap in the zstd layer",
        ));
        docs.push((11, None, false, 51, false, false, "a gap in the summary"));
        docs.push((12, None, false, 52, false, false, "Navier–Stokes notes"));
        docs.push((
            13,
            None,
            true,
            53,
            false,
            false,
            "Navier–Stokes smoothness lemma",
        ));
        docs.push((
            14,
            None,
            true,
            54,
            true,
            false,
            "Navier–Stokes disputed lemma",
        ));
        index(&conn, &docs);
        let s = Searcher::new(&db.path);
        let slot = s.take().unwrap();
        // The tokenizer is the index's: case and diacritics fold, punctuation splits; the words
        // are stemmed, and the words of one stem are one term, the first word kept.
        let words = |text: &str| -> Vec<(String, String)> {
            slot.terms(text)
                .unwrap()
                .into_iter()
                .map(|t| (t.word, t.stem))
                .collect()
        };
        assert_eq!(
            words("Café SUMMARY, café-gap! Summaries"),
            [
                ("cafe".into(), "cafe".into()),
                ("summary".into(), "summari".into()),
                ("gap".into(), "gap".into())
            ]
        );
        // Stop words are dropped before anything is looked up.
        assert_eq!(
            words("Hey, can you please look at the gap?"),
            [("look".into(), "look".into()), ("gap".into(), "gap".into())]
        );
        assert!(words("Thanks! Could you, please?").is_empty());
        let terms = slot
            .terms("the summary gap in zstd and navier and an unknown word")
            .unwrap();
        let rare = slot
            .rarest(
                &terms,
                true,
                &s.freq,
                Mono::now().plus(Duration::from_secs(60)),
            )
            .unwrap();
        // Rarest first, ties by the term; frequency 0 (`unknown`, `word`) is left out, and the
        // vocabulary is empty here, so nothing corrects them.
        assert_eq!(
            picked(&rare),
            [(1, "zstd"), (2, "gap"), (3, "navier"), (32, "summary")]
        );
        s.give(slot);
        // Relevance: the document with more rare terms first.
        let h = s.search(&Query::Create("zstd summary gap".into()));
        assert_eq!(nodes(&h.related)[..2], [10, 11]);
        // Knowledge is inside kb/ and leaves disputed nodes out; related is outside kb/.
        let h = s.search(&Query::Hello("Navier–Stokes".into()));
        assert_eq!(nodes(&h.related), [12]);
        assert_eq!(nodes(&h.knowledge), [13]);
        assert!(!h.related.partial && !h.knowledge.partial);
        // view find looks everywhere, the disputed node last.
        let h = s.search(&Query::Find("navier".into()));
        assert_eq!(nodes(&h.related).last(), Some(&14));
        assert_eq!(h.related.nodes.len(), 3);
        // More matches than shown: partial (≥k).
        let h = s.search(&Query::FromUser("summary".into()));
        assert_eq!(h.related.nodes.len(), RELATED_MAX);
        assert!(h.related.partial);
        // Nothing indexed matches: no hits.
        assert_eq!(
            s.search(&Query::Find("quaternion".into())),
            SearchHits::default()
        );
    }

    /// 1.9: a node's title, path, and keys are columns of their own, weighted above its text; a
    /// word is found by its stem, and the path's and keys' words are found as words.
    #[test]
    fn titles_paths_and_keys_weigh_more_than_text_and_words_are_stemmed() {
        let db = TempDb::new("search-columns");
        let (conn, _) = open_index(&db.path).unwrap();
        let none: [&str; 0] = [];
        index_columns(
            &conn,
            &[
                (
                    20,
                    1,
                    Columns::node(
                        "Resize station photos on upload",
                        "/tidepool/image-resizing",
                        ["user:e33"],
                        &["Station photos are uploaded at full size."],
                    ),
                ),
                // Newer, with the same words in its text only.
                (
                    21,
                    2,
                    Columns::node(
                        "Weekly dependency update",
                        "/tidepool/deps-weekly",
                        none,
                        &["Check packages; photo resizing in the API may need Pillow."],
                    ),
                ),
                (
                    22,
                    3,
                    Columns::node(
                        "Fix the crash on an empty NOAA feed item",
                        "/tidepool/rss-crash",
                        ["issue:tidepool-app#41"],
                        &["FeedParser crashes on an item without <pubDate>."],
                    ),
                ),
            ],
        );
        let s = Searcher::with_budget(&db.path, Duration::from_secs(60));
        // `resizing` and `photo` are the title's `Resize` and `photos`: stemmed alike.
        let h = s.search(&Query::FromUser(
            "where are we with the photo resizing?".into(),
        ));
        assert_eq!(nodes(&h.related), [20, 21], "the title's match first");
        // A word of the path alone, and a key's words.
        assert_eq!(nodes(&s.search(&Query::Find("RSS".into())).related), [22]);
        assert_eq!(
            nodes(&s.search(&Query::Find("app#41".into())).related),
            [22]
        );
        assert!(
            s.search(&Query::Find("rss".into())).matched.is_empty(),
            "nothing corrected"
        );
    }

    /// 1.9: a word no document has is replaced by the closest word of the vocabulary that a
    /// document has, within the bounds, and the hits say so.
    #[test]
    fn a_word_no_document_has_is_corrected_within_its_bounds() {
        let db = TempDb::new("search-fuzzy");
        let (conn, _) = open_index(&db.path).unwrap();
        let none: [&str; 0] = [];
        let doc = |title: &str, path: &str| Columns::node(title, path, none, &[]);
        index_columns(
            &conn,
            &[
                (30, 1, doc("Resize station photos", "/app/image-resizing")),
                (31, 2, doc("Why is Snowflake spend up?", "/data/spend")),
                (
                    32,
                    3,
                    doc("Upgrade Postgres 14 to 16", "/platform/pg16-upgrade"),
                ),
                // "match" in two documents, "batch" in one: both 1 edit from "xatch".
                (33, 4, doc("match jobs", "/jobs/a")),
                (34, 5, doc("match loads", "/jobs/b")),
                (35, 6, doc("batch rules", "/jobs/c")),
                (36, 7, doc("the rss feed", "/feed")),
            ],
        );
        let s = Searcher::with_budget(&db.path, Duration::from_secs(60));
        // Two letters dropped from a word of 7: 2 edits.
        let h = s.search(&Query::Find("resizng".into()));
        assert_eq!(nodes(&h.related), [30]);
        assert_eq!(h.matched, ["resizing for resizng"]);
        // Two letters swapped: 1 edit.
        let h = s.search(&Query::Find("snowlfake".into()));
        assert_eq!(
            (nodes(&h.related), h.matched),
            (vec![31], vec!["snowflake for snowlfake".into()])
        );
        // A stem the index has needs no correction: `postgre` is `postgres`'s stem.
        let h = s.search(&Query::Find("postgre".into()));
        assert_eq!((nodes(&h.related), h.matched.is_empty()), (vec![32], true));
        // Of two words 1 edit away, the one in more documents ("match", in 2), though "batch"
        // comes first in order; and the closer one before either.
        let h = s.search(&Query::Find("xatch".into()));
        assert_eq!(h.matched, ["match for xatch"]);
        assert_eq!(nodes(&h.related), [34, 33]);
        let h = s.search(&Query::Find("bxtch".into()));
        assert_eq!(
            (nodes(&h.related), h.matched),
            (vec![35], vec!["batch for bxtch".into()])
        );
        // Not corrected: a word with a digit, one of 3 letters, one too far (3 edits, or 2 in
        // a word of 4 letters), and one whose first two and last two letters are both wrong.
        for q in ["pg17", "rsx", "snwfalk", "bxtk", "xesizinx"] {
            assert_eq!(
                s.search(&Query::Find(q.into())),
                SearchHits::default(),
                "{q}"
            );
        }
        // A word found exactly comes first: a correction only fills the room the rarest terms
        // leave (QUERY_TERMS).
        let slot = s.take().unwrap();
        let far = Mono::now().plus(Duration::from_secs(60));
        let many = "batch loads jobs rules feed station photos snowflake resizng";
        let rare = slot
            .rarest(&slot.terms(many).unwrap(), true, &s.freq, far)
            .unwrap();
        assert_eq!(
            rare.len(),
            8,
            "every word found, their lists short (LISTED_CHEAP)"
        );
        assert!(rare.iter().all(|p| p.for_word.is_none()), "{rare:?}");
        let rare = slot
            .rarest(&slot.terms("match resizng").unwrap(), true, &s.freq, far)
            .unwrap();
        assert_eq!(picked(&rare), [(2, "match"), (1, "resizing")]);
        // Past the deadline, nothing is corrected.
        let rare = slot
            .rarest(&slot.terms("resizng").unwrap(), true, &s.freq, Mono::now())
            .unwrap();
        assert!(rare.iter().all(|p| p.for_word.is_none()), "{rare:?}");
        s.give(slot);
    }

    /// 1.9 (lab, it4 l6): only `view find {text}`, a query an agent typed, corrects a word; the
    /// searches of a `hello`, a `from_user`, and a `create`, whose words are prose, never do, so
    /// their hits never say `fuzzy`, and their words found as written still find their nodes.
    #[test]
    fn only_view_find_corrects_a_word() {
        let db = TempDb::new("search-fuzzy-find-only");
        let (conn, _) = open_index(&db.path).unwrap();
        let none: [&str; 0] = [];
        let doc = |title: &str, path: &str| Columns::node(title, path, none, &[]);
        index_columns(
            &conn,
            &[
                (30, 1, doc("Resize station photos", "/app/image-resizing")),
                (31, 2, doc("Weekly report", "/team/report")),
            ],
        );
        let s = Searcher::with_budget(&db.path, Duration::from_secs(60));
        let h = s.search(&Query::Find("resizng".into()));
        assert_eq!(
            (nodes(&h.related), h.matched),
            (vec![30], vec!["resizing for resizng".into()])
        );
        for q in [
            Query::Hello("resizng".into()),
            Query::FromUser("resizng".into()),
            Query::Create("resizng".into()),
        ] {
            assert_eq!(s.search(&q), SearchHits::default(), "{q:?}");
        }
        for q in [
            Query::Hello("Check the station photos resizng\nhow is the resizng going?".into()),
            Query::FromUser("how are the photos? resizng done?".into()),
            Query::Create("Photos resizng follow-up".into()),
        ] {
            let h = s.search(&q);
            assert_eq!(nodes(&h.related), [30], "{q:?}");
            assert!(h.matched.is_empty(), "{q:?}: {:?}", h.matched);
        }
        // A human's ordinary word that no document has stays as written: `reprot` is not
        // rewritten into `report`.
        let h = s.search(&Query::Hello("where is the weekly reprot?".into()));
        assert_eq!((nodes(&h.related), h.matched.is_empty()), (vec![31], true));
        let h = s.search(&Query::Hello("reprot".into()));
        assert_eq!(h, SearchHits::default());
    }

    #[test]
    fn the_distance_counts_insertions_deletions_substitutions_and_swaps() {
        let d = |a: &str, b: &str, max| distance(&a.chars().collect::<Vec<_>>(), b, max);
        assert_eq!(d("resizng", "resizing", 2), Some(1));
        assert_eq!(d("resizng", "resize", 2), Some(2));
        assert_eq!(d("onbaording", "onboarding", 2), Some(1));
        assert_eq!(d("kubernets", "kubernetes", 2), Some(1));
        assert_eq!(d("snowflak", "snowflake", 1), Some(1));
        assert_eq!(d("abc", "abc", 0), Some(0));
        assert_eq!(d("disqs", "disqus", 1), Some(1));
        assert_eq!(d("snwfalk", "snowflake", 2), None);
        assert_eq!(d("ab", "abcde", 2), None, "lengths too far apart");
        assert_eq!(d("café", "cafe", 1), Some(1), "letters, not bytes");
    }

    #[test]
    fn stop_words_are_sorted_and_the_vocabulary_holds_words_of_letters() {
        assert!(STOP_WORDS.windows(2).all(|w| w[0] < w[1]));
        for w in [
            "about", "please", "thanks", "can", "could", "would", "should", "morning",
        ] {
            assert!(stop_word(w), "{w}");
        }
        for w in ["hey", "hi", "just", "actually", "also", "still"] {
            assert!(stop_word(w), "{w}");
        }
        assert!(!stop_word("postgres") && !stop_word("upgrade"));
        assert!(vocabulary_word("resizing"));
        for w in ["pg16", "ab", "about", &"x".repeat(33), "v2"] {
            assert!(!vocabulary_word(w), "{w}");
        }
        assert_eq!(shape("résumé"), ("ré".into(), "mé".into(), 6));
        // The indexer puts each document's new words in the vocabulary, once.
        let db = TempDb::new("search-words");
        let (conn, _) = open_index(&db.path).unwrap();
        let c = Columns::node(
            "Upgrade Postgres 14 to 16",
            "/platform/pg16-upgrade",
            ["issue:infra#212"],
            &["About the upgrade: postgres"],
        );
        index_columns(&conn, &[(40, 1, c.clone()), (41, 2, c)]);
        let words: Vec<String> = conn
            .prepare("SELECT word FROM words ORDER BY word")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(words, ["infra", "issue", "platform", "postgres", "upgrade"]);
    }

    #[test]
    fn past_its_budget_a_search_falls_back_to_the_rarest_term_newest_first() {
        let db = TempDb::new("search-budget");
        let (conn, _) = open_index(&db.path).unwrap();
        // One document inside kb/, the oldest, so outside the newest page of the list.
        let mut docs = vec![(
            10,
            None,
            true,
            0,
            false,
            false,
            "alpha in the knowledge base",
        )];
        for i in 0..4_000_u64 {
            docs.push((
                1_000 + i,
                None,
                false,
                i,
                false,
                false,
                "alpha beta gamma delta",
            ));
        }
        let tx = conn.unchecked_transaction().unwrap();
        index(&tx, &docs);
        tx.commit().unwrap();
        // Within the budget (a generous one: a debug build, beside the other tests), the ranked
        // query finds the best matches (all equal here but the one in kb/, so the newest first by
        // `at`), each scope its own: the knowledge document is found, however many related ones
        // rank above it.
        let s = Searcher::with_budget(&db.path, Duration::from_secs(60));
        let h = s.search(&Query::Find("alpha".into())).related;
        assert_eq!(h.nodes.len(), FIND_MAX);
        assert_eq!(h.nodes[0], NodeId(4_999));
        let h = s.search(&Query::Hello("alpha".into()));
        assert_eq!(nodes(&h.related), [4_999, 4_998, 4_997]);
        assert!(h.related.partial);
        assert_eq!(nodes(&h.knowledge), [10]);
        assert!(!h.knowledge.partial);
        // Lists of RANKED_MAX documents or more: the ranked query is not tried, and the fallback
        // reads one page of the list, which the knowledge document is not in (≥k).
        let slot = s.take().unwrap();
        let far = Mono::now().plus(Duration::from_secs(60));
        let hello = scopes(&Query::Hello(String::new()));
        let (h, _) = slot.find(&[pick(4_001, "alpha")], hello, far);
        assert_eq!(nodes(&h[1]), [10], "ranked");
        let (h, _) = slot.find(&[pick(RANKED_MAX, "alpha")], hello, far);
        assert_eq!(nodes(&h[0]), [4_999, 4_998, 4_997]);
        assert!(h[0].partial && h[1].partial && h[1].nodes.is_empty());
        s.give(slot);
        // A budget no ranked query meets: the progress handler stops it, and the fallback reads
        // the rarest term's newest documents and says ≥k.
        let s = Searcher::with_budget(&db.path, Duration::ZERO);
        let h = s.search(&Query::Create("alpha beta".into())).related;
        assert_eq!(nodes(&h), [4_999, 4_998, 4_997]);
        assert!(h.partial);
        let h = s.search(&Query::Hello("alpha".into()));
        assert_eq!(nodes(&h.related), [4_999, 4_998, 4_997]);
        assert!(h.knowledge.nodes.is_empty() && h.knowledge.partial);
    }

    /// 1.6 (§18): no scope holds the user log, `@user`, `@user/decisions`, and `@user/digest`, in
    /// the ranked query and in the fallback alike: the human's message that a `hello` searches for
    /// matched itself there. In the lab, `/@user` came first in the related list of every `hello`,
    /// and the node the work lived on was missed.
    #[test]
    fn the_user_log_is_never_a_match() {
        let db = TempDb::new("search-user-log");
        let (conn, _) = open_index(&db.path).unwrap();
        let docs = vec![
            (
                USER.0,
                Some(40),
                false,
                40,
                false,
                false,
                "resize photos to avif",
            ),
            (
                USER_DECISIONS.0,
                Some(41),
                false,
                41,
                false,
                false,
                "avif or webp photos",
            ),
            (
                USER_DIGEST.0,
                Some(42),
                false,
                42,
                false,
                false,
                "photos resized to avif",
            ),
            (
                20,
                None,
                false,
                10,
                false,
                false,
                "resize station photos on upload",
            ),
        ];
        index(&conn, &docs);
        let s = Searcher::with_budget(&db.path, Duration::from_secs(60));
        let h = s.search(&Query::Hello("resize photos to avif".into()));
        assert_eq!(nodes(&h.related), [20]);
        let h = s.search(&Query::Find("avif".into()));
        assert!(h.related.nodes.is_empty(), "{h:?}");
        // The fallback, past its budget, leaves them out too.
        let s = Searcher::with_budget(&db.path, Duration::ZERO);
        let h = s.search(&Query::Find("photos".into()));
        assert_eq!(nodes(&h.related), [20]);
    }

    /// A frequency of [`FREQ_KEPT_MIN`] or more is reused for [`FREQ_FRESH`]; a lower one is
    /// counted afresh.
    #[test]
    fn common_frequencies_are_reused_and_rare_ones_counted_afresh() {
        let f = Freq::default();
        f.put("rare", FREQ_KEPT_MIN - 1);
        f.put("common", FREQ_KEPT_MIN);
        assert_eq!(f.get("rare"), None);
        assert_eq!(f.get("common"), Some(FREQ_KEPT_MIN));
        assert_eq!(f.get("other"), None);
        // Full of fresh ones: none is displaced, and the new one is not kept.
        let old = Mono::now();
        {
            let mut m = lock(&f.0);
            m.clear();
            for i in 0..FREQ_KEPT_MAX {
                m.insert(format!("t{i}"), (FREQ_KEPT_MIN, old));
            }
        }
        f.put("new", FREQ_KEPT_MIN);
        assert_eq!(f.get("new"), None, "the cache is full of fresh ones");
        assert_eq!(f.get("t0"), Some(FREQ_KEPT_MIN));
    }

    /// The frequency lookups are under the query's deadline, made once per call, and over at most
    /// [`TERMS_LOOKED_UP`] terms: a long text of common words no longer costs a lookup per word
    /// before the budget even starts (review E4-spec-7: 1.5 s for a 50 ms budget at 200,000
    /// documents).
    #[test]
    fn the_rarest_term_lookup_is_within_the_budget() {
        let db = TempDb::new("search-lookup-budget");
        let (conn, _) = open_index(&db.path).unwrap();
        // 3,000 documents of the same 300 common words, and one with a rare word.
        let common: Vec<String> = (0..300).map(|i| format!("w{i}")).collect();
        let text = common.join(" ");
        let tx = conn.unchecked_transaction().unwrap();
        let mut docs: Vec<Doc<'_>> = (0..3_000_u64)
            .map(|i| (1_000 + i, None, false, i, false, false, text.as_str()))
            .collect();
        docs.push((9, None, false, 5_000, false, false, "w0 quaternion"));
        index(&tx, &docs);
        tx.commit().unwrap();
        let s = Searcher::new(&db.path);
        let slot = s.take().unwrap();
        // At most TERMS_LOOKED_UP distinct terms are looked up.
        let mut query = common[..257].join(" ");
        query.push_str(" quaternion");
        let terms = slot.terms(&query).unwrap();
        assert_eq!(terms.len(), TERMS_LOOKED_UP);
        // Past the deadline, only the first term is looked up: the fallback has a term.
        let rare = slot
            .rarest(&terms, false, &Freq::default(), Mono::now())
            .unwrap();
        assert_eq!(
            rare.iter().map(|p| p.word.as_str()).collect::<Vec<_>>(),
            ["w0"]
        );
        // Every lookup with no deadline, for comparison.
        let start = Mono::now();
        let all = slot
            .rarest(
                &terms,
                false,
                &Freq::default(),
                Mono::now().plus(Duration::from_secs(600)),
            )
            .unwrap();
        let unbounded = Mono::now().since(start);
        assert_eq!(all.len(), QUERY_TERMS);
        s.give(slot);
        // A `hello` of 257 common words: the call stays near its budget, whatever the lookups
        // would cost, and still answers (the fallback, ≥k).
        let budget = Duration::from_millis(5);
        let s = Searcher::with_budget(&db.path, budget);
        let start = Mono::now();
        let h = s.search(&Query::Hello(common[..257].join(" ")));
        let took = Mono::now().since(start);
        assert!(!h.related.nodes.is_empty());
        assert!(
            took < budget + 2 * SEARCH_BUDGET + Duration::from_millis(400),
            "{took:?} (every lookup: {unbounded:?})"
        );
    }

    #[test]
    fn a_file_of_another_version_is_created_afresh() {
        let db = TempDb::new("search-version");
        let (conn, created) = open_index(&db.path).unwrap();
        assert!(created);
        conn.execute("UPDATE fed SET seq = 42", []).unwrap();
        drop(conn);
        let (conn, created) = open_index(&db.path).unwrap();
        assert!(!created);
        assert_eq!(fed(&conn).unwrap(), 42);
        conn.execute(
            "UPDATE meta SET value = '1' WHERE key = 'search_version'",
            [],
        )
        .unwrap();
        drop(conn);
        let (conn, created) = open_index(&db.path).unwrap();
        assert!(created, "a file of 1.8 (version 1) is rebuilt");
        assert_eq!(
            fed(&conn).unwrap(),
            0,
            "the feeder rebuilds it from the history"
        );
    }
}
