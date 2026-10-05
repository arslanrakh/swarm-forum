//! The recall test set of text search (1.9, D.11.12): the lab's seeded forums, as they stood when
//! the agents of its fourth round (it3) started (`recall.json`, extracted from the round's exports:
//! each node's path, title, body, digest, keys, last close's summary and learnings, and marks, and
//! the people-facing entries), and the searches that the lab's agents made in them, each with the
//! nodes it should have found: in the top 3 of `related` or `knowledge`, or the top 20 of `view
//! find {text}`. The misses that 1.9 answers, typos, and searches that must find nothing.

use std::collections::BTreeMap;
use std::time::Duration;

use rusqlite::{Connection, params};
use serde::Deserialize;

use super::{Columns, Indexer, Query, Searcher, open_index};
use crate::store::testkit::TempDb;

#[derive(Deserialize)]
struct Forum {
    nodes: Vec<Node>,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Node {
    id: u64,
    path: String,
    title: String,
    body: String,
    digest: String,
    keys: Vec<String>,
    summary: String,
    learnings: String,
    at: u64,
    kb: bool,
    disputed: bool,
    retracted: bool,
}

#[derive(Deserialize)]
struct Entry {
    eid: u64,
    node: u64,
    text: String,
    at: u64,
}

/// Which hits a case looks at.
#[derive(Clone, Copy, Debug)]
enum Seen {
    /// `related`, the top 3 (`view find {text}`: the top 20).
    Related,
    /// `knowledge`, the top 3.
    Knowledge,
}

/// A search and the nodes it must find.
struct Case {
    forum: &'static str,
    query: fn(String) -> Query,
    text: &'static str,
    seen: Seen,
    want: &'static [&'static str],
    why: &'static str,
}

fn hello(t: String) -> Query {
    Query::Hello(t)
}

fn find(t: String) -> Query {
    Query::Find(t)
}

fn from_user(t: String) -> Query {
    Query::FromUser(t)
}

const L2: &str = "l2-solo-month";
const L3: &str = "l3-company-quarter";
const L4: &str = "l4-mature-year";
const L6: &str = "l6-incident";

/// The cases that must find their nodes.
const CASES: &[Case] = &[
    // The misses of the lab's memory analysis.
    Case {
        forum: L2,
        query: hello,
        text: "Check photo resizing progress and apply user's AVIF format decision.\nmorning! where \
               are we with the photo resizing? and for the format question: let's go with AVIF \
               actually, the size savings are worth it.",
        seen: Seen::Related,
        want: &["/tidepool/image-resizing"],
        why: "it0 l2 A: photo resizing against Resize station photos on upload",
    },
    Case {
        forum: L2,
        query: hello,
        text: "Report status of photo resizing work; record Sam's AVIF decision\nmorning! where are \
               we with the photo resizing? and for the format question: let's go with AVIF \
               actually, the size savings are worth it.",
        seen: Seen::Related,
        want: &["/tidepool/image-resizing"],
        why: "it1 l2 A",
    },
    Case {
        forum: L2,
        query: hello,
        text: "Check photo resizing progress and resolve the format question with the user's AVIF \
               choice.\nmorning! where are we with the photo resizing? and for the format question: \
               let's go with AVIF actually, the size savings are worth it.",
        seen: Seen::Related,
        want: &["/tidepool/image-resizing"],
        why: "it3 l2 A (it2's was it0's)",
    },
    Case {
        forum: L2,
        query: from_user,
        text: "morning! where are we with the photo resizing? and for the format question: let's go \
               with AVIF actually, the size savings are worth it.",
        seen: Seen::Related,
        want: &["/tidepool/image-resizing"],
        why: "the human's words alone",
    },
    Case {
        forum: L2,
        query: hello,
        text: "Recall last month's RSS crash resolution, track recurrence, and add Disqus migration \
               investigation task.\nCan you remind me what we did about the RSS crash last month? \
               A user says the app crashes on the feed again. Also add a task to look into moving \
               comments off Disqus.",
        seen: Seen::Related,
        want: &["/tidepool/comments-provider", "/tidepool/rss-crash"],
        why: "it1 l2 B: moving comments off Disqus against Move comments off Disqus",
    },
    Case {
        forum: L2,
        query: hello,
        text: "Answer Sam's question about last month's RSS crash; add task about moving comments \
               off Disqus\nCan you remind me what we did about the RSS crash last month? A user \
               says the app crashes on the feed again. Also add a task to look into moving \
               comments off Disqus.",
        seen: Seen::Related,
        want: &["/tidepool/comments-provider", "/tidepool/rss-crash"],
        why: "it3 l2 B",
    },
    Case {
        forum: L3,
        query: hello,
        text: "Answer Priya's question about Postgres 16 upgrade owner and prod date",
        seen: Seen::Related,
        want: &["/platform/pg16-upgrade"],
        why: "it3 l3 A: kept about, owner, priya, 16 and dropped postgres and upgrade",
    },
    Case {
        forum: L3,
        query: hello,
        text: "Answer Priya Raman's Postgres 16 upgrade ownership and production timing request in \
               thread T3301",
        seen: Seen::Related,
        want: &["/platform/pg16-upgrade"],
        why: "it1 l3 A",
    },
    Case {
        forum: L3,
        query: hello,
        text: "Answer Priya Raman in thread chat:platform/T3301 about Postgres 16 upgrade owner and \
               production timing",
        seen: Seen::Related,
        want: &["/platform/pg16-upgrade"],
        why: "it2 l3 A",
    },
    Case {
        forum: L2,
        query: find,
        text: "RSS",
        seen: Seen::Related,
        want: &["/tidepool/rss-crash"],
        why: "it1 l2 B: RSS is only in the node's path",
    },
    // Typos and variants.
    Case {
        forum: L4,
        query: find,
        text: "snowflak",
        seen: Seen::Related,
        want: &["/data/review-warehouse-spend"],
        why: "a typo",
    },
    Case {
        forum: L3,
        query: find,
        text: "postgre",
        seen: Seen::Related,
        want: &["/platform/pg16-upgrade"],
        why: "a variant",
    },
    Case {
        forum: L2,
        query: find,
        text: "resizng",
        seen: Seen::Related,
        want: &["/tidepool/image-resizing"],
        why: "a typo",
    },
    Case {
        forum: L3,
        query: find,
        text: "kubernets",
        seen: Seen::Related,
        want: &["/platform/k8s-1-31"],
        why: "a typo",
    },
    Case {
        forum: L2,
        query: find,
        text: "notifcation",
        seen: Seen::Related,
        want: &["/tidepool/high-tide-push"],
        why: "a typo",
    },
    Case {
        forum: L2,
        query: find,
        text: "onbaording",
        seen: Seen::Related,
        want: &["/tidepool/onboarding-copy"],
        why: "two letters swapped",
    },
    Case {
        forum: L2,
        query: find,
        text: "disqs",
        seen: Seen::Related,
        want: &["/tidepool/comments-provider"],
        why: "a short typo",
    },
    Case {
        forum: L3,
        query: find,
        text: "chargback",
        seen: Seen::Related,
        want: &["/payments/chargeback-report"],
        why: "a typo",
    },
    // What the lab's searches found, which must stay found.
    Case {
        forum: L3,
        query: hello,
        text: "Resume refund retries work and apply user's answer of 3 retries max.\nhey, can you \
               pick up the refund retries work? I answered the question the agent asked last week: \
               3 retries max.",
        seen: Seen::Related,
        want: &["/payments/refund-retries"],
        why: "it3 l3 B",
    },
    Case {
        forum: L4,
        query: hello,
        text: "Investigate recurring CSV export time offsets, locate previous fixes and fix root \
               cause with regression coverage\nCustomers say the CSV exports show the wrong times \
               again, off by an hour or two. I'm pretty sure we fixed this before? Please fix it \
               properly this time.",
        seen: Seen::Related,
        want: &["/support-tools/csv-export-timezone"],
        why: "it3 l4 B",
    },
    Case {
        forum: L4,
        query: find,
        text: "CSV export wrong times timezone",
        seen: Seen::Related,
        want: &["/support-tools/csv-export-timezone"],
        why: "it1 l4 B",
    },
    Case {
        forum: L4,
        query: hello,
        text: "Orient new data team member on nightly pipeline operations, warehouse ownership, \
               and current data team work.\nHi, first day on the data team. Could you give me a \
               quick orientation: how do we run the nightly pipelines here, who owns the \
               warehouse, and what is the data team working on right now?",
        seen: Seen::Knowledge,
        want: &["/kb/nightly-pipelines", "/kb/warehouse"],
        why: "it3 l4 A: the kb records a newcomer needs",
    },
    Case {
        forum: L6,
        query: hello,
        text: "Incident lead for EU checkout 500s since ~16:08 UTC; gather picture, then fix\nI \
               just got paged: EU checkout is throwing 500s, apparently since about 16:08 UTC. I'm \
               the incident lead now. Get me the picture: what do we know, who's doing what, what \
               changed? Then let's fix it.",
        seen: Seen::Related,
        want: &["/incidents/inc-311-eu-checkout"],
        why: "it3 l6 A",
    },
    Case {
        forum: L6,
        query: hello,
        text: "Handle incident thread T7001 message M7 asking for customer update amid 140 support \
               tickets.",
        seen: Seen::Related,
        want: &["/incidents/checkout-errors-eu"],
        why: "it3 l6 B: the thread's key, chat:incidents/T7001",
    },
    Case {
        forum: L3,
        query: find,
        text: "infra#212",
        seen: Seen::Related,
        want: &["/platform/pg16-upgrade"],
        why: "a key's value",
    },
];

/// Searches that must find nothing.
type Nothing = (
    &'static str,
    fn(String) -> Query,
    &'static str,
    &'static str,
);

const NOTHING: &[Nothing] = &[
    (L2, find, "quaternion", "a word no document has"),
    (L2, find, "photosynthesis", "no word near it"),
    (L2, find, "kafka", "no word near it"),
    (L3, find, "Disqus", "another forum's word"),
    (
        L3,
        find,
        "pg17",
        "a word with a digit is never corrected: pg16 is another thing",
    ),
    (L2, find, "rsx", "a word of 3 letters is never corrected"),
    (
        L2,
        from_user,
        "Hey, thanks! Morning. Can you please just go ahead?",
        "stop words only",
    ),
];

/// A search file holding `forum`, its documents' columns as the feeder makes them.
fn index(forum: &Forum) -> TempDb {
    let db = TempDb::new("search-recall");
    let (conn, _) = open_index(&db.path).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let mut ix = Indexer::new().unwrap();
    let marks: BTreeMap<u64, (bool, bool, bool)> = forum
        .nodes
        .iter()
        .map(|n| (n.id, (n.kb, n.disputed, n.retracted)))
        .collect();
    for n in &forum.nodes {
        let c = Columns::node(
            &n.title,
            &n.path,
            n.keys.iter().map(String::as_str),
            &[&n.body, &n.digest, &n.summary, &n.learnings],
        );
        put(&tx, &mut ix, (n.id, None, n.at), marks[&n.id], &c);
    }
    for e in &forum.entries {
        let m = marks.get(&e.node).copied().unwrap_or_default();
        put(
            &tx,
            &mut ix,
            (e.node, Some(e.eid), e.at),
            m,
            &Columns::entry(&e.text),
        );
    }
    tx.commit().unwrap();
    db
}

/// Indexes the document of `(node, eid, at)`, with its marks `(kb, disputed, retracted)`.
fn put(
    conn: &Connection,
    ix: &mut Indexer,
    (node, eid, at): (u64, Option<u64>, u64),
    (kb, disputed, retracted): (bool, bool, bool),
    c: &Columns,
) {
    conn.execute(
        "INSERT INTO docs (node, eid, under_kb, at, disputed, retracted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            node as i64,
            eid.map(|e| e as i64),
            kb,
            at as i64,
            disputed,
            retracted
        ],
    )
    .unwrap();
    ix.insert(conn, conn.last_insert_rowid(), c).unwrap();
}

#[test]
fn the_lab_searches_find_their_nodes() {
    let forums: BTreeMap<String, Forum> =
        serde_json::from_str(include_str!("recall.json")).unwrap();
    let files: BTreeMap<&str, TempDb> = forums
        .iter()
        .map(|(name, f)| (name.as_str(), index(f)))
        .collect();
    let paths = |forum: &str, hits: &[forum_core::NodeId]| -> Vec<String> {
        let p: BTreeMap<u64, &str> = forums[forum]
            .nodes
            .iter()
            .map(|n| (n.id, n.path.as_str()))
            .collect();
        hits.iter()
            .map(|n| {
                p.get(&n.0)
                    .map_or_else(|| format!("#{}", n.0), |s| (*s).to_string())
            })
            .collect()
    };
    let (mut found, mut wanted, mut ranks) = (0, 0, 0.0);
    let mut report = Vec::new();
    for c in CASES {
        let s = Searcher::with_budget(&files[c.forum].path, Duration::from_secs(60));
        let h = s.search(&(c.query)(c.text.to_string()));
        let hits = match c.seen {
            Seen::Related => &h.related,
            Seen::Knowledge => &h.knowledge,
        };
        let got = paths(c.forum, &hits.nodes);
        for w in c.want {
            wanted += 1;
            let rank = got.iter().position(|g| g == w);
            let ok = rank.is_some();
            found += usize::from(ok);
            ranks += rank.map_or(0.0, |r| 1.0 / (r as f64 + 1.0));
            report.push(format!(
                "{} {w} for {:?} in {:?} ({}): {got:?}",
                if ok { "found" } else { "MISS " },
                c.text.chars().take(40).collect::<String>(),
                c.seen,
                c.why,
            ));
        }
    }
    let mut quiet = 0;
    for (forum, query, text, why) in NOTHING {
        let s = Searcher::with_budget(&files[forum].path, Duration::from_secs(60));
        let h = s.search(&query((*text).to_string()));
        let got = paths(forum, &h.related.nodes);
        quiet += usize::from(got.is_empty());
        report.push(format!(
            "{} {text:?} ({why}): {got:?}",
            if got.is_empty() { "none " } else { "FOUND" }
        ));
    }
    println!("{}", report.join("\n"));
    println!(
        "recall {found}/{wanted}, mean reciprocal rank {:.3}; searches that must find nothing: \
         {quiet}/{}",
        ranks / f64::from(u32::try_from(wanted).unwrap_or(1)),
        NOTHING.len()
    );
    assert_eq!((found, quiet), (wanted, NOTHING.len()));
}
