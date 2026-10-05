//! Words of a node (1.9, §16): the fixed stop-word list, and the words that tidying compares to find
//! possible duplicates.
//!
//! Both are fixed functions of the text: no locale, no dictionary, and no floating point. A node's
//! words are the runs of letters and digits of its title and the start of its body, lower-cased,
//! without stop words and numbers, each cut to a stem: a final `s` dropped (not from `ss`), then its
//! first `STEM` letters, so that `warehouse` and `warehouses`, or `checkout` and `checkouts`, are
//! one word. Two open nodes under one parent are possible duplicates when they share at least
//! `ALIKE_MIN` rare words, a word being rare when at most `RARE_DF` of the nodes compared carry it.
//! In the lab, the two pairs of the seeds that were one piece of work, "Review warehouse costs" and
//! "Why is Snowflake spend up?", and "INC-311: EU checkout returns 500s" and "Checkout errors for EU
//! customers", share no title word or about a third of them, and two rare words each; and each
//! title holds one of those words (`alike`).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;

/// The stop words (1.9, §16), sorted and in lower case: English function words, the fragments a
/// tokenizer cuts from contractions (`s`, `t`, `don`), and the words of a request that say nothing
/// about its subject (`question`, `work`, `please`), which tell two nodes' subjects apart no more
/// than `the` does. Words that carry an incident's meaning (`down`, `out`, `up`, `off`, `over`)
/// are not among them.
pub const STOP_WORDS: [&str; 169] = [
    "a",
    "about",
    "after",
    "again",
    "against",
    "all",
    "also",
    "am",
    "an",
    "and",
    "answer",
    "any",
    "are",
    "aren",
    "as",
    "ask",
    "asked",
    "asks",
    "at",
    "available",
    "be",
    "because",
    "been",
    "before",
    "being",
    "between",
    "both",
    "but",
    "by",
    "can",
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
    "etc",
    "few",
    "for",
    "forum",
    "from",
    "further",
    "had",
    "hadn",
    "has",
    "hasn",
    "have",
    "haven",
    "having",
    "he",
    "hello",
    "help",
    "her",
    "here",
    "hers",
    "herself",
    "hi",
    "him",
    "himself",
    "his",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "isn",
    "it",
    "its",
    "itself",
    "just",
    "ll",
    "m",
    "may",
    "me",
    "might",
    "more",
    "most",
    "must",
    "my",
    "myself",
    "need",
    "needs",
    "no",
    "nor",
    "not",
    "now",
    "of",
    "on",
    "once",
    "only",
    "or",
    "other",
    "our",
    "ours",
    "ourselves",
    "own",
    "pick",
    "please",
    "question",
    "questions",
    "re",
    "s",
    "same",
    "shall",
    "she",
    "should",
    "shouldn",
    "since",
    "so",
    "some",
    "such",
    "t",
    "task",
    "tasks",
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
    "this",
    "those",
    "through",
    "to",
    "too",
    "until",
    "ve",
    "very",
    "via",
    "want",
    "wants",
    "was",
    "wasn",
    "we",
    "were",
    "weren",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whom",
    "why",
    "will",
    "with",
    "won",
    "work",
    "would",
    "wouldn",
    "yet",
    "you",
    "your",
    "yours",
    "yourself",
    "yourselves",
];

/// Whether `w`, a lower-case word, is a stop word.
#[must_use]
pub fn is_stop_word(w: &str) -> bool {
    STOP_WORDS.binary_search(&w).is_ok()
}

/// A stem keeps at most this many letters (1.9): words that share their first five letters are one
/// word.
pub const STEM: usize = 5;

/// The bytes of a body read for its words (1.9): its start, which says what the node is.
pub const BODY_BYTES: usize = 1_024;

/// Distinct words of a node at most, its title's first (1.9).
pub const DOC_WORDS: usize = 64;

/// A word is rare when at most this many of the nodes compared carry it (1.9).
pub const RARE_DF: u32 = 3;

/// Possible duplicates share at least this many rare words (1.9).
pub const ALIKE_MIN: u32 = 2;

/// The stem of a lower-case word, or none for a stop word, a number, or a single letter: a final
/// `s` dropped from a word of four or more letters that does not end in `ss`, then its first
/// `STEM` characters.
#[must_use]
pub fn stem(w: &str) -> Option<String> {
    if is_stop_word(w) || w.chars().all(|c| c.is_ascii_digit()) || w.chars().nth(1).is_none() {
        return None;
    }
    let base = match w.strip_suffix('s') {
        Some(b) if w.chars().nth(3).is_some() && !b.ends_with('s') => b,
        _ => w,
    };
    Some(base.chars().take(STEM).collect())
}

/// The words of a text, lower-cased, in order of first occurrence: its maximal runs of letters and
/// digits.
fn runs(t: &str, mut f: impl FnMut(String) -> bool) {
    let mut cur = String::new();
    for c in t.chars() {
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() && !f(core::mem::take(&mut cur)) {
            return;
        }
    }
    if !cur.is_empty() {
        f(cur);
    }
}

/// Whether a text names a pull request (1.10, §20.9 rule 8): among its words (runs of letters and
/// digits, lower-cased), `pr` or `prs`, or `pull` or `merge` followed by `request` or `requests`,
/// as in "opened PR #215", "PR#215", or "the merge request". What a release's `intent_done` says
/// is the only text the next calls read for it: a `handoff` release after a pull request was
/// opened offered the finished work to the next worker (the lab's fifth round, l7).
#[must_use]
pub fn names_pull_request(t: &str) -> bool {
    let mut found = false;
    let mut prev = String::new();
    runs(t, |w| {
        let req = w == "request" || w == "requests";
        if w == "pr" || w == "prs" || (req && (prev == "pull" || prev == "merge")) {
            found = true;
            return false;
        }
        prev = w;
        true
    });
    found
}

/// The start of `t`, at most `max` bytes, cut at a character boundary.
fn head(t: &str, max: usize) -> &str {
    let mut end = t.len().min(max);
    while !t.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    t.get(..end).unwrap_or("")
}

/// A node's words (1.9, §16): the stems of its title's words, and those of its title and of the
/// first `BODY_BYTES` of its body, at most `DOC_WORDS` distinct ones, the title's first.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Doc {
    /// The title's.
    pub title: BTreeSet<String>,
    /// The title's and the body's.
    pub all: BTreeSet<String>,
}

impl Doc {
    /// Whether the node has no word.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
}

/// A node's words (1.9, §16).
#[must_use]
pub fn doc_words(title: &str, body: &str) -> Doc {
    let mut d = Doc::default();
    runs(title, |w| {
        if let Some(k) = stem(&w) {
            d.title.insert(k.clone());
            d.all.insert(k);
        }
        d.all.len() < DOC_WORDS
    });
    if d.all.len() < DOC_WORDS {
        runs(head(body, BODY_BYTES), |w| {
            if let Some(k) = stem(&w) {
                d.all.insert(k);
            }
            d.all.len() < DOC_WORDS
        });
    }
    d
}

/// How many of the nodes compared carry each word: their document frequencies.
#[must_use]
pub fn doc_freq<'a>(docs: impl IntoIterator<Item = &'a Doc>) -> BTreeMap<String, u32> {
    let mut df: BTreeMap<String, u32> = BTreeMap::new();
    for d in docs {
        for w in &d.all {
            let n = df.entry(w.clone()).or_insert(0);
            *n = n.saturating_add(1);
        }
    }
    df
}

/// How alike two nodes are (1.9, §16): the rare words they share, words that at most `RARE_DF` of
/// the nodes compared carry, when there are at least `ALIKE_MIN` of them and each title holds one
/// of them, so that each node's subject is in the other; else 0. In the lab, "Upgrade Airflow to
/// 2.10" and "Alert when nightly_core publishes late" shared `nightly` and `core`, the dag both
/// work on, and only the second's title held them.
#[must_use]
pub fn alike(a: &Doc, b: &Doc, df: &BTreeMap<String, u32>) -> u32 {
    let rare = |w: &String| df.get(w).is_some_and(|d| *d <= RARE_DF);
    let shared = a.all.intersection(&b.all).filter(|w| rare(w)).count();
    let titled = |x: &Doc, y: &Doc| x.title.iter().any(|w| rare(w) && y.all.contains(w));
    let n = u32::try_from(shared).unwrap_or(u32::MAX);
    if n >= ALIKE_MIN && titled(a, b) && titled(b, a) {
        n
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn the_stop_words_are_sorted_lower_case_and_unique() {
        assert!(STOP_WORDS.windows(2).all(|w| matches!(w, [a, b] if a < b)));
        assert!(
            STOP_WORDS
                .iter()
                .all(|w| w.chars().all(|c| c.is_ascii_lowercase()))
        );
    }

    /// 1.10 (§20.9 rule 8): the lab's `intent_done` names its pull request; words that only
    /// contain the letters, and a request that is not a pull request, do not.
    #[test]
    fn a_text_names_a_pull_request() {
        for t in [
            "Done: pushed v2/storybook and opened PR #215",
            "opened PR#215",
            "prs are open",
            "Opened the pull request",
            "merge-request !42 is open",
            "pull requests 3 and 4",
        ] {
            assert!(names_pull_request(t), "{t}");
        }
        for t in [
            "pushed the branch",
            "a request to pull the data",
            "prod is deployed; sprint review",
            "pull the request from the queue",
            "",
        ] {
            assert!(!names_pull_request(t), "{t}");
        }
    }

    #[test]
    fn stems_drop_a_plural_s_and_keep_five_letters() {
        let st = |w: &str| stem(w);
        assert_eq!(st("warehouses"), st("warehouse"));
        assert_eq!(st("warehouse").as_deref(), Some("wareh"));
        assert_eq!(st("checkouts"), st("checkout"));
        assert_eq!(st("costs").as_deref(), Some("cost"));
        assert_eq!(st("class").as_deref(), Some("class"));
        assert_eq!(st("bus").as_deref(), Some("bus"));
        assert_eq!(st("eu").as_deref(), Some("eu"));
        assert_eq!(st("the"), None);
        assert_eq!(st("500"), None);
        assert_eq!(st("x"), None);
    }

    #[test]
    fn a_node_s_words_are_its_title_s_and_its_body_s_start() {
        let d = doc_words(
            "Answer Priya's question about Postgres 16 upgrade owner",
            "since the pg16 runbook",
        );
        let w: Vec<String> = d.title.iter().cloned().collect();
        assert_eq!(w, ["owner", "postg", "priya", "upgra"]);
        assert!(d.all.contains("runbo") && d.all.contains("pg16") && !d.all.contains("since"));
        assert!(
            doc_words("Pick up available work on the forum", "")
                .all
                .contains("up")
        );
        assert_eq!(doc_words("Ärger mit Übersetzung", "").all.len(), 3);
        assert!(doc_words("  — ", "").is_empty());
        // The body counts up to BODY_BYTES, and DOC_WORDS words at most.
        let long = "alpha ".repeat(300) + "omega";
        assert!(!doc_words("t", &long).all.contains("omega"));
        let many: Vec<String> = (0_u8..100)
            .map(|i| {
                let a = char::from(b'a'.saturating_add(i / 26));
                let b = char::from(b'a'.saturating_add(i % 26));
                alloc::format!("{a}{b}xyz")
            })
            .collect();
        assert_eq!(doc_words(&many.join(" "), "").all.len(), DOC_WORDS);
    }

    /// The lab's two pairs of possible duplicates share two rare words each, and each title holds
    /// one of them; unrelated siblings do not.
    #[test]
    fn the_lab_s_duplicates_are_alike() {
        let costs = doc_words(
            "Review warehouse costs",
            "Snowflake costs grew 40 % this quarter; find the biggest warehouses and queries.",
        );
        let spend = doc_words(
            "Why is Snowflake spend up?",
            "Finance asks why Snowflake spend is up; look at credit usage by warehouse.",
        );
        let airflow = doc_words(
            "Upgrade Airflow to 2.10",
            "Upgrade the scheduler and workers from 2.7 to 2.10; the nightly_core dag must keep \
             running.",
        );
        let sla = doc_words(
            "Alert when nightly_core publishes late",
            "Page the data on-call when the BI schemas are not published by 05:00 UTC.",
        );
        let df = doc_freq([&costs, &spend, &airflow, &sla]);
        assert_eq!(alike(&costs, &spend, &df), 2);
        // They share `nightly` and `core`, but the first title holds neither.
        assert_eq!(alike(&airflow, &sla, &df), 0);
        assert_eq!(alike(&costs, &airflow, &df), 0);
        let inc = doc_words(
            "INC-311: EU checkout returns 500s",
            "EU checkout requests fail with HTTP 500 since 16:05 UTC (about 18 % of EU checkouts).",
        );
        let thread = doc_words("Checkout errors for EU customers", "");
        let df = doc_freq([&inc, &thread]);
        assert_eq!(alike(&inc, &thread, &df), 2);
        // A word that four nodes carry is not rare.
        let df = doc_freq(vec![&inc, &thread, &inc, &thread]);
        assert_eq!(alike(&inc, &thread, &df), 0);
        let klarna = doc_words("Retry Klarna authorisations once", "One retry on timeout.");
        let vat = doc_words(
            "Hotfix vat-svc lookup latency",
            "vat-svc v2.14.0 dropped the rate cache; restore it in v2.14.1.",
        );
        assert_eq!(alike(&klarna, &vat, &doc_freq([&klarna, &vat])), 0);
    }
}
