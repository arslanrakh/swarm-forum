//! Synthetic text: a fixed vocabulary of pronounceable words, drawn with a head-heavy (about Zipf)
//! distribution, so that titles, bodies, checkpoints, and search intents share words the way a
//! real forum's do, and a search for common words is the worst case of the search (gate G-9).

use crate::rng::Rng;

const ONSETS: [&str; 16] = [
    "b", "c", "d", "f", "g", "k", "l", "m", "n", "p", "r", "s", "t", "v", "z", "st",
];
const VOWELS: [&str; 6] = ["a", "e", "i", "o", "u", "ai"];
const CODAS: [&str; 8] = ["", "n", "r", "s", "l", "x", "th", "nd"];

/// The vocabulary's size.
pub const WORDS: usize = 4_096;

/// The `i`-th word of the vocabulary (two or three syllables, unique for `i < WORDS`).
#[must_use]
pub fn word(i: usize) -> String {
    let mut x = i % WORDS;
    let mut w = String::new();
    // Three syllables of 16 × 6 × 8 = 768 forms would repeat; the index picks each part.
    for k in 0..2 {
        w.push_str(ONSETS[x % ONSETS.len()]);
        x /= ONSETS.len();
        w.push_str(VOWELS[(x + k) % VOWELS.len()]);
        if k == 1 {
            w.push_str(CODAS[x % CODAS.len()]);
        }
    }
    w.push_str(&format!("{}", i % WORDS));
    w
}

/// `n` words drawn from the vocabulary, head-heavy, separated by spaces.
pub fn words(rng: &mut Rng, n: usize) -> String {
    let mut out = String::new();
    for k in 0..n {
        if k > 0 {
            out.push(' ');
        }
        out.push_str(&word(rng.zipf(WORDS)));
    }
    out
}

/// A worst-case search intent: the most common words of the vocabulary.
#[must_use]
pub fn common_words(n: usize) -> String {
    (0..n).map(word).collect::<Vec<_>>().join(" ")
}

/// A cheap search intent with hits: the rarest words of the vocabulary, each in a few documents
/// of a filled forum, so that a search finds a page of them over short posting lists (gate G-9's
/// control).
#[must_use]
pub fn rare_words(n: usize) -> String {
    (WORDS.saturating_sub(n)..WORDS)
        .map(word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A body of about `bytes` bytes.
pub fn body(rng: &mut Rng, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes.saturating_add(16));
    while out.len() < bytes {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word(rng.zipf(WORDS)));
    }
    out.truncate(bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_unique_and_plain() {
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..WORDS {
            let w = word(i);
            assert!(w.chars().all(|c| c.is_ascii_alphanumeric()), "{w}");
            assert!(seen.insert(w));
        }
        let mut r = Rng::new(1, 1);
        assert_eq!(words(&mut r, 5).split(' ').count(), 5);
        assert_eq!(body(&mut r, 1_000).len(), 1_000);
        assert_eq!(common_words(3).split(' ').count(), 3);
        assert_eq!(rare_words(3).split(' ').count(), 3);
        assert!(rare_words(1).ends_with(&format!("{}", WORDS - 1)));
    }
}
