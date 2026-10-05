//! Property tests of the normalisers (design D.11.14, D.12.1): totality (no panic on any string) and
//! idempotence (`normalise(normalise(x).key) == normalise(x)`), with 10,000 cases each, on three
//! input families: arbitrary strings, keys built from every scheme with values drawn from the
//! characters the rules care about, and URLs with credential parameters and user information. A
//! fourth family puts a control character, a line or paragraph separator, or an unassigned code
//! point inside a key; a fifth puts a refused default-ignorable code point anywhere in one, its
//! edges included; and a sixth puts any refused character into the path, query, or fragment of a
//! URL key, which is checked in its ASCII normal form. A seventh builds queries from pairs and
//! pieces whose credentials are known, written after `&` or after `&amp;` escaped once or more, and
//! checks the URL key against what a model keeps of them, and against the key of the same query
//! HTML-escaped again.

use std::borrow::Cow;
use std::ops::RangeInclusive;

use forum_keys::{
    KeyError, NormKey, VERSION, is_normal, normalise, normalise_principal, normalise_v,
};
use proptest::prelude::*;

/// `[a-z][a-z0-9+.-]{0,15}`, the shape of a scheme in a normal form.
fn is_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && scheme.len() <= 16
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'))
}

/// Arbitrary strings, every code point possible (control characters, combining marks, and
/// surrogate-free planes alike).
fn any_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(any::<char>(), 0..96).prop_map(String::from_iter)
}

/// Characters that the rules treat specially, plus some that Unicode case mapping and NFC treat
/// specially, so that random values reach every branch. Characters that the rules refuse wherever
/// they occur are in [`REFUSED`] and [`INVISIBLE`] instead, so that most values get past the
/// character check; format characters and default-ignorable ones that the rules accept (U+0600,
/// and the variation selectors U+180B, U+FE0F, and U+E0100) are here.
const SPECIAL: &[char] = &[
    '/',
    '#',
    '<',
    '>',
    '@',
    ':',
    '?',
    '&',
    ';',
    '[',
    '=',
    '%',
    '+',
    '.',
    '-',
    '_',
    'v',
    'V',
    'e',
    'E',
    '0',
    '1',
    '9',
    'a',
    'f',
    'A',
    'F',
    'g',
    'x',
    'X',
    'z',
    ' ',
    '\u{a0}',
    '\u{3000}',
    '\u{301}',
    '\u{308}',
    '\u{327}',
    '\u{345}',
    '\u{130}',
    '\u{131}',
    '\u{3a3}',
    '\u{3c2}',
    '\u{1fbc}',
    '\u{212a}',
    '\u{212b}',
    '\u{c5}',
    '\u{e9}',
    '\u{df}',
    '\u{1e9e}',
    '\u{fb01}',
    '\u{ff21}',
    '\u{600}',
    '\u{180b}',
    '\u{fe0f}',
    '\u{e0100}',
    '\u{e000}',
    '\u{a7ce}',
    '\u{10400}',
];

/// Control characters (`Cc`), the line and paragraph separators, and code points that Unicode 17.0
/// leaves unassigned (noncharacters among them): refused anywhere inside a key or a principal key.
/// The whitespace among them is trimmed at the edges.
const REFUSED: &[char] = &[
    '\u{0}',
    '\t',
    '\n',
    '\u{b}',
    '\r',
    '\u{1b}',
    '\u{1f}',
    '\u{7f}',
    '\u{85}',
    '\u{9f}',
    '\u{2028}',
    '\u{2029}',
    '\u{378}',
    '\u{fdd0}',
    '\u{fffe}',
    '\u{2fa1e}',
    '\u{3347a}',
    '\u{e0080}',
    '\u{10ffff}',
];

/// Blocks that Unicode 17.0 assigns in full: Latin with IPA, the combining diacritics, and the
/// start of Greek; Cyrillic; Latin Extended Additional; Glagolitic; CJK; Cherokee's lower case;
/// Hangul; private use; Deseret; and emoji, without the default-ignorable U+00AD and U+034F. They
/// hold case pairs (including ones outside the BMP), compositions, and characters without case.
const ASSIGNED: &[RangeInclusive<char>] = &[
    '\u{a0}'..='\u{ac}',
    '\u{ae}'..='\u{34e}',
    '\u{350}'..='\u{377}',
    '\u{400}'..='\u{52f}',
    '\u{1e00}'..='\u{1eff}',
    '\u{2c00}'..='\u{2c5f}',
    '\u{4e00}'..='\u{9fff}',
    '\u{ab70}'..='\u{abbf}',
    '\u{ac00}'..='\u{d7a3}',
    '\u{e000}'..='\u{f8ff}',
    '\u{10400}'..='\u{1044f}',
    '\u{1f300}'..='\u{1f64f}',
];

/// The default-ignorable code points that Unicode 17.0 assigns, without the variation selectors
/// U+180B to U+180D, U+180F, U+FE00 to U+FE0F, and U+E0100 to U+E01EF (145 in all): refused
/// anywhere in a key or a principal key, its edges included, because none of them is whitespace,
/// except in an http or https key, whose ASCII normal form is checked instead.
const INVISIBLE: &[RangeInclusive<char>] = &[
    '\u{ad}'..='\u{ad}',
    '\u{34f}'..='\u{34f}',
    '\u{61c}'..='\u{61c}',
    '\u{115f}'..='\u{1160}',
    '\u{17b4}'..='\u{17b5}',
    '\u{180e}'..='\u{180e}',
    '\u{200b}'..='\u{200f}',
    '\u{202a}'..='\u{202e}',
    '\u{2060}'..='\u{2064}',
    '\u{2066}'..='\u{206f}',
    '\u{3164}'..='\u{3164}',
    '\u{feff}'..='\u{feff}',
    '\u{ffa0}'..='\u{ffa0}',
    '\u{1bca0}'..='\u{1bca3}',
    '\u{1d173}'..='\u{1d17a}',
    '\u{e0001}'..='\u{e0001}',
    '\u{e0020}'..='\u{e007f}',
];

/// Whether no normal form may hold `c`: a refused default-ignorable code point, or a line or
/// paragraph separator.
fn is_refused_anywhere(c: char) -> bool {
    INVISIBLE.iter().any(|range| range.contains(&c)) || matches!(c, '\u{2028}' | '\u{2029}')
}

fn invisible_char() -> impl Strategy<Value = char> {
    proptest::char::ranges(Cow::Borrowed(INVISIBLE))
}

/// Whether `raw` is an http or https key: the character rules then read its normal form.
fn is_url(raw: &str) -> bool {
    raw.trim().split_once(':').is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
}

/// The names of the credential parameters, which no URL key keeps, compared in ASCII lower case
/// after reading the name as PHP reads it ([`php_name`]).
const CREDENTIAL_NAMES: &[&str] = &[
    "sig",
    "signature",
    "token",
    "access_token",
    "key",
    "api_key",
    "apikey",
    "password",
    "passwd",
    "secret",
    "client_secret",
];

/// A decoded name as PHP reads it: the spaces (U+0020) in front of it stripped, the name ended at
/// its first NUL, and every space, `.`, and `[` read as `_`.
fn php_name(decoded: &str) -> String {
    let name = decoded.trim_start_matches(' ');
    let name = match name.find('\0') {
        Some(nul) => &name[..nul],
        None => name,
    };
    name.replace([' ', '.', '['], "_")
}

/// Whether a piece of a query is a credential: its decoded name, read as PHP reads it and in ASCII
/// lower case, is one of [`CREDENTIAL_NAMES`] or starts with `x-amz-` or `x-goog-`.
fn is_credential_piece(piece: &str) -> bool {
    url::form_urlencoded::parse(piece.as_bytes())
        .next()
        .is_some_and(|(name, _)| {
            let lower = php_name(&name).to_ascii_lowercase();
            lower.starts_with("x-amz-")
                || lower.starts_with("x-goog-")
                || CREDENTIAL_NAMES.contains(&lower.as_str())
        })
}

/// The pieces of a query that holds no `&amp;`, pair by pair: split at `&` into pairs, and each
/// pair split at `;`.
fn pieces(query: &str) -> Vec<Vec<&str>> {
    query
        .split('&')
        .map(|pair| pair.split(';').collect())
        .collect()
}

/// `query` with every `&` HTML-escaped `depth` times: `&amp;` once, `&amp;amp;` twice.
fn escape_ampersands(query: &str, depth: usize) -> String {
    query.replace('&', &format!("&{}", "amp;".repeat(depth)))
}

/// What marks the value of a credential, and of every piece after one in its pair, in
/// [`modelled_query`]: no URL key may hold it.
const MARK: &str = "S3CR3T";

/// A query built from pairs of pieces, and the query that a URL key keeps of it (`None` when it
/// keeps none). Each pair is written after `&`, or after `&amp;` escaped zero to two more times
/// (`&amp;amp;amp;`), and holds one to three pieces, each a name (a credential's, in one of its
/// spellings, or another, `amp` among them) and an optional value. Every `&amp;` reads as `&`, so
/// in a pair after the first, the `amp;` written after the `&`, and pieces `amp` in front of
/// another piece, which spell `amp;` as well, go with the `&`; the first pair has no `&` in front of
/// it, so each of its `amp;`s is a piece `amp`. A pair is kept up to its first credential piece,
/// without the `;` in front of it; a pair that the credential starts is dropped, and so is a pair
/// left empty. Every value from the first credential of a pair on is marked with [`MARK`].
fn modelled_query() -> impl Strategy<Value = (String, Option<String>)> {
    let name = proptest::sample::select(
        &[
            ("token", true),
            ("Api.Key", true),
            ("api[key", true),
            ("%74oken", true),
            ("X-Amz-Date", true),
            ("client_secret", true),
            ("PASSWORD", true),
            ("+api_key", true),
            ("%20Token", true),
            ("api+key", true),
            ("Access%20Token", true),
            ("key%00x", true),
            ("+%20api.key%00", true),
            ("a", false),
            ("page", false),
            ("keys", false),
            ("api-key", false),
            ("api_key[]", false),
            ("api_key+", false),
            ("%09api_key", false),
            ("%00token", false),
            ("amp", false),
            ("x", false),
            ("", false),
        ][..],
    );
    let piece = (name, proptest::option::of("[a-z0-9=.]{0,4}"));
    let pair = (0..4_usize, proptest::collection::vec(piece, 1..4));
    proptest::collection::vec(pair, 1..5).prop_map(|pairs| {
        let mut query = String::new();
        let mut kept = Vec::new();
        for (index, (entities, pieces)) in pairs.into_iter().enumerate() {
            if index > 0 {
                query.push('&');
            }
            let mut written = Vec::new();
            let mut first_credential = None;
            for (at, ((name, credential), value)) in pieces.into_iter().enumerate() {
                if credential && first_credential.is_none() {
                    first_credential = Some(at);
                }
                written.push(match (value, first_credential.is_some()) {
                    (Some(value), true) => format!("{name}={MARK}{value}"),
                    (None, true) => format!("{name}={MARK}"),
                    (Some(value), false) => format!("{name}={value}"),
                    (None, false) => name.to_owned(),
                });
            }
            query.push_str(&"amp;".repeat(entities));
            query.push_str(&written.join(";"));
            // The pieces that the rules read: in the first pair, each "amp;" written in front of
            // the pieces is a piece "amp"; after a '&', it goes with the '&', and so does every
            // piece "amp" that another piece follows.
            let (read, first) = if index == 0 {
                let mut read = vec!["amp".to_owned(); entities];
                read.extend(written);
                (read, first_credential.map(|at| at + entities))
            } else {
                let skip = written[..written.len() - 1]
                    .iter()
                    .take_while(|piece| *piece == "amp")
                    .count();
                let read: Vec<String> = written.into_iter().skip(skip).collect();
                (read, first_credential.map(|at| at - skip))
            };
            let pair = match first {
                None => read.join(";"),
                Some(at) => read[..at].join(";"),
            };
            if !pair.is_empty() {
                kept.push(pair);
            }
        }
        let kept = (!kept.is_empty()).then(|| kept.join("&"));
        (query, kept)
    })
}

fn assigned_char() -> impl Strategy<Value = char> {
    proptest::char::ranges(Cow::Borrowed(ASSIGNED))
}

fn special_text(max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            4 => proptest::sample::select(SPECIAL),
            1 => assigned_char(),
        ],
        0..max,
    )
    .prop_map(String::from_iter)
}

/// Every scheme with a rule of its own, a reserved one, and some ordinary ones, in mixed case.
const SCHEMES: &[&str] = &[
    "chat", "issue", "mail", "user", "person", "repeat", "http", "https", "arxiv", "doi", "lean",
    "run", "slack", "note", "x", "CHAT", "Mail", "HTTPS", "ArXiv", "DOI", "Person", "1bad", "b_ad",
    "",
];

/// `<scheme>:<value>`, either with a value built from the special characters (sometimes after a
/// nested principal or `arXiv:` prefix) or with a value shaped like its scheme's, so that every
/// scheme's accepting path is reached as well as its refusals; sometimes with a little noise
/// appended, and sometimes padded with whitespace.
fn scheme_key() -> impl Strategy<Value = String> {
    let random = (
        proptest::sample::select(SCHEMES),
        prop_oneof![
            Just(String::new()),
            Just("arXiv:".to_string()),
            Just("https://doi.org/".to_string()),
            Just("mail:".to_string()),
            Just("user".to_string()),
            Just("<".to_string()),
        ],
        special_text(40),
    )
        .prop_map(|(scheme, prefix, value)| format!("{scheme}:{prefix}{value}"));
    let shaped = (
        shaped_key(),
        prop_oneof![3 => Just(String::new()), 1 => special_text(3)],
    )
        .prop_map(|(key, noise)| format!("{key}{noise}"));
    (
        prop_oneof![random, shaped],
        prop_oneof![Just(""), Just(" "), Just("\n\t"), Just("\u{3000}")],
    )
        .prop_map(|(key, pad)| format!("{pad}{key}{pad}"))
}

/// A key whose value has its scheme's shape, in mixed case and with the odd non-ASCII letter.
fn shaped_key() -> impl Strategy<Value = String> {
    prop_oneof![
        "(chat|CHAT):[A-Za-z0-9\u{c9}\u{130}.-]{1,8}/[A-Za-z0-9.]{1,8}(/[A-Za-z0-9.]{1,8})?",
        "issue:[A-Za-z/\u{130}]{1,8}#[0-9]{1,6}(/[A-Za-z0-9_-]{1,8})?",
        "mail:<?[A-Za-z0-9.]{1,8}@[A-Za-z.\u{c9}]{1,8}>?",
        "(user|USER):e?[0-9]{1,22}",
        "person:( ?user| ?USER|mail:[A-Za-z@.\u{c9}]{1,8}|slack:[A-Za-z0-9/]{1,8}|user:e1|[a-z]{1,4})",
        "arxiv:(arXiv:)?([0-9]{4}[.][0-9]{4,5}|[a-zA-Z-]{2,8}([.][A-Z]{2})?/[0-9]{7})(v[0-9]{1,2}){0,2}",
        "doi:(https://doi[.]org/|doi[.]org/|HTTPS://DOI[.]ORG/)?10[.][0-9]{4}/[A-Za-z0-9.\u{c4}]{1,10}",
        "(lean|run|LEAN):[0-9a-fA-F]{1,64}",
    ]
}

/// A value shaped like a principal key, the reserved `user` and the refused `user:` scheme included.
fn principal_key() -> impl Strategy<Value = String> {
    "( ?(user|USER|User) ?|(mail|MAIL):[A-Za-z0-9.\u{c9}]{1,8}@[A-Za-z.]{1,8}|(slack|team):[A-Za-z0-9/]{1,10}|user:e[0-9]{1,3})"
}

/// URLs whose query mixes credential parameters (in assorted cases and spellings) with ordinary
/// ones and empty pairs, separated by `&`, `;`, `&amp;`, and `&amp;amp;`, some with a user name and
/// a password.
fn url_key() -> impl Strategy<Value = String> {
    let name = proptest::sample::select(
        &[
            "X-Amz-Signature",
            "x-amz-credential",
            "X-AMZ-DATE",
            "X-Goog-Signature",
            "x-goog-date",
            "sig",
            "signature",
            "token",
            "access_token",
            "key",
            "%74oken",
            "Sig",
            "Token",
            "SIGNATURE",
            "Access_Token",
            "KEY",
            "api_key",
            "apikey",
            "password",
            "passwd",
            "secret",
            "client_secret",
            "API_KEY",
            "ApiKey",
            "Password",
            "PASSWD",
            "Secret",
            "Client_Secret",
            "%61pi_key",
            "api%5Fkey",
            "p%61ssword",
            "api.key",
            "api[key",
            "API.KEY",
            "Client.Secret",
            "access[token",
            "api%2Ekey",
            "api%5Bkey",
            "api+key",
            "api%20key",
            "+api_key",
            "%20token",
            "+X-Amz-Date",
            "key%00",
            "Secret%00x",
            "page",
            "id",
            "keys",
            "x-amz",
            "x-goog",
            "Policy",
            "api-key",
            "apikeys",
            "secrets",
            "client_id",
            "passwords",
            "api[key]",
            "api_key[]",
            "api_key+",
            "%09token",
            "%00key",
            "amp",
            "",
        ][..],
    );
    let pair = (
        proptest::sample::select(&["&", "&", ";", "&amp;", "&amp;amp;"][..]),
        name,
        special_text(6),
    )
        .prop_map(|(separator, n, v)| {
            if v.is_empty() {
                format!("{separator}{n}")
            } else {
                format!("{separator}{n}={v}")
            }
        });
    (
        proptest::sample::select(&["http", "https", "HTTPS", "Http"][..]),
        proptest::sample::select(
            &[
                "example.com",
                "EXAMPLE.org:443",
                "x.example:80",
                "B\u{fc}cher.example",
                "[::1]",
                "a b",
                "user:pass@x.example",
                ":secret@x.example",
                "u@x.example",
                "U%3Ax:p%40ss@X.example:443",
            ][..],
        ),
        special_text(12),
        proptest::collection::vec(pair, 0..6),
        proptest::option::of(special_text(6)),
    )
        .prop_map(|(scheme, host, path, pairs, fragment)| {
            let mut url = format!("{scheme}://{host}/{path}");
            if !pairs.is_empty() {
                url.push('?');
                url.push_str(&pairs.concat());
            }
            if let Some(fragment) = fragment {
                url.push('#');
                url.push_str(&fragment);
            }
            url
        })
}

/// Every input family at once.
fn key_input() -> impl Strategy<Value = String> {
    prop_oneof![any_text(), scheme_key(), url_key()]
}

/// The properties every key normal form has.
fn check_key(raw: &str) -> Result<(), TestCaseError> {
    // Totality: these calls return, whatever the input.
    let first = normalise(raw);
    let _ = normalise_v(0, raw);
    let _ = is_normal(raw, VERSION);
    let Ok(NormKey { key, version }) = first.clone() else {
        return Ok(());
    };
    prop_assert_eq!(version, VERSION);
    // Idempotence, and K2's oracle on the normal form.
    prop_assert_eq!(
        normalise(&key),
        first.clone(),
        "not idempotent on {:?}",
        raw
    );
    prop_assert!(is_normal(&key, VERSION));
    // The shape of every normal form.
    prop_assert!(key.len() <= 512);
    prop_assert_eq!(key.trim(), key.as_str());
    prop_assert!(!key.chars().any(char::is_control), "{:?}", key);
    prop_assert!(!key.chars().any(is_refused_anywhere), "{:?}", key);
    let (scheme, value) = key.split_once(':').expect("a normal form has a scheme");
    prop_assert!(is_scheme(scheme), "bad scheme in {:?}", key);
    if scheme == "http" || scheme == "https" {
        prop_assert!(key.is_ascii(), "{:?} gave {:?}", raw, key);
    }
    prop_assert!(!value.is_empty());
    prop_assert_ne!(scheme, "repeat");
    // Surrounding whitespace never matters.
    prop_assert_eq!(normalise(&format!(" \t{raw}\n ")), first);
    Ok(())
}

/// The properties every principal normal form has.
fn check_principal(raw: &str) -> Result<(), TestCaseError> {
    let first = normalise_principal(raw);
    let Ok(principal) = first.clone() else {
        return Ok(());
    };
    prop_assert_eq!(
        normalise_principal(&principal),
        first.clone(),
        "not idempotent on {:?}",
        raw
    );
    prop_assert!(principal.len() <= 256);
    prop_assert!(!principal.chars().any(char::is_control), "{:?}", principal);
    prop_assert!(
        !principal.chars().any(is_refused_anywhere),
        "{:?}",
        principal
    );
    if principal != "user" {
        let (scheme, value) = principal.split_once(':').expect("a principal has a scheme");
        prop_assert!(is_scheme(scheme), "bad scheme in {:?}", principal);
        prop_assert_ne!(scheme, "user");
        prop_assert!(!value.is_empty());
    }
    // A person's alias is the principal key itself (D-3). Whitespace that the principal normaliser
    // trims at its edges would sit inside the person key, where a control character is refused, so
    // the value is passed trimmed, as a client writes it.
    prop_assert_eq!(
        normalise(&format!("person:{}", raw.trim())).map(|n| n.key),
        Ok(format!("person:{principal}"))
    );
    Ok(())
}

/// The generator of assigned characters is worth its name: every character of its blocks is
/// accepted inside a value.
#[test]
fn the_assigned_blocks_are_accepted() {
    for c in ASSIGNED.iter().cloned().flatten() {
        assert!(
            normalise(&format!("note:x{c}x")).is_ok(),
            "U+{:04X}",
            u32::from(c)
        );
        assert!(
            normalise_principal(&format!("slack:x{c}x")).is_ok(),
            "U+{:04X}",
            u32::from(c)
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn keys_are_total_and_idempotent(raw in key_input()) {
        check_key(&raw)?;
    }

    #[test]
    fn principals_are_total_and_idempotent(
        raw in prop_oneof![any_text(), scheme_key(), principal_key()]
    ) {
        check_principal(&raw)?;
    }

    #[test]
    fn url_keys_carry_no_credentials(raw in url_key()) {
        if let Ok(NormKey { key, .. }) = normalise(&raw) {
            let url = url::Url::parse(&key).expect("a URL key is a URL");
            prop_assert!(url.fragment().is_none());
            prop_assert_eq!(url.username(), "", "{:?} kept a user name", raw);
            prop_assert!(url.password().is_none(), "{:?} kept a password", raw);
            prop_assert_ne!(url.query(), Some(""), "{:?} kept an empty query", raw);
            // No "&amp;" is left, no pair between two '&' is empty, and no piece of a pair is a
            // credential.
            if let Some(query) = url.query() {
                prop_assert!(!query.contains("&amp;"), "{:?} kept an &amp;", raw);
                prop_assert!(!query.split('&').any(str::is_empty), "{:?} kept an empty pair", raw);
                for piece in pieces(query).into_iter().flatten() {
                    prop_assert!(!is_credential_piece(piece), "{:?} kept {:?}", raw, piece);
                }
            }
        }
    }

    #[test]
    fn url_keys_keep_what_the_model_keeps((query, kept) in modelled_query()) {
        let raw = format!("https://x.example/p?{query}");
        let want = match &kept {
            Some(kept) => format!("https://x.example/p?{kept}"),
            None => "https://x.example/p".to_owned(),
        };
        let got = normalise(&raw).map(|n| n.key);
        prop_assert_eq!(got.as_deref(), Ok(want.as_str()), "{:?}", raw);
        // Nothing of a credential's value is kept, however a server splits the query.
        prop_assert!(!want.contains(MARK));
        // A query with nothing to drop and no "&amp;" is left as it is.
        if !query.split('&').any(str::is_empty)
            && !query.contains(MARK)
            && !query.contains("&amp;")
        {
            prop_assert_eq!(kept.as_deref(), Some(query.as_str()));
        }
        // And the same query HTML-escaped again, once or more, gives the same key.
        for depth in 1..=3 {
            let escaped = format!("https://x.example/p?{}", escape_ampersands(&query, depth));
            prop_assert_eq!(normalise(&escaped).map(|n| n.key), got.clone(), "{:?}", escaped);
        }
    }

    #[test]
    fn control_and_unassigned_code_points_are_refused_inside(
        key in prop_oneof![shaped_key(), principal_key()],
        refused in proptest::sample::select(REFUSED),
        at in any::<proptest::sample::Index>(),
    ) {
        // Somewhere strictly inside, so that trimming cannot remove it.
        let key = key.trim();
        let boundaries: Vec<usize> = key.char_indices().map(|(i, _)| i).skip(1).collect();
        prop_assume!(!boundaries.is_empty());
        let (head, tail) = key.split_at(*at.get(&boundaries));
        let raw = format!("{head}{refused}{tail}");
        prop_assert!(
            matches!(normalise(&raw), Err(KeyError::BadValue(_))),
            "{:?} gave {:?}", raw, normalise(&raw)
        );
        prop_assert!(
            matches!(normalise_principal(&raw), Err(KeyError::BadValue(_))),
            "{:?} gave {:?}", raw, normalise_principal(&raw)
        );
    }

    #[test]
    fn default_ignorables_are_refused_anywhere_but_in_a_url(
        key in prop_oneof![shaped_key(), principal_key(), url_key(), scheme_key()],
        invisible in invisible_char(),
        at in any::<proptest::sample::Index>(),
        pad in prop_oneof![Just(""), Just(" "), Just("\n\t"), Just("\u{3000}")],
    ) {
        // Anywhere: at either edge, inside, and with whitespace around the whole.
        let mut boundaries: Vec<usize> = key.char_indices().map(|(i, _)| i).collect();
        boundaries.push(key.len());
        let (head, tail) = key.split_at(*at.get(&boundaries));
        let raw = format!("{pad}{head}{invisible}{tail}{pad}");
        // Totality and the shape of every normal form hold for these inputs too.
        check_key(&raw)?;
        check_principal(&raw)?;
        // A principal has no URL rule.
        prop_assert!(
            matches!(normalise_principal(&raw), Err(KeyError::BadValue(_))),
            "{:?} gave {:?}", raw, normalise_principal(&raw)
        );
        // A URL key is checked in its normal form, which check_key has found ASCII.
        if !is_url(&raw) {
            prop_assert!(
                matches!(normalise(&raw), Err(KeyError::BadValue(_))),
                "{:?} gave {:?}", raw, normalise(&raw)
            );
            prop_assert!(!is_normal(&raw, VERSION));
        }
    }

    #[test]
    fn url_keys_accept_what_their_parser_encodes(
        base in url_key(),
        refused in prop_oneof![
            invisible_char(),
            proptest::sample::select(REFUSED),
        ],
        at in any::<proptest::sample::Index>(),
    ) {
        // Somewhere after the host: in the path, the query, or the fragment.
        let Some(start) = base.find("://").and_then(|i| base[i + 3..].find('/').map(|j| i + 3 + j))
        else {
            return Ok(());
        };
        let mut boundaries: Vec<usize> = base[start..]
            .char_indices()
            .map(|(i, _)| start + i)
            .skip(1)
            .collect();
        boundaries.push(base.len());
        let (head, tail) = base.split_at(*at.get(&boundaries));
        let raw = format!("{head}{refused}{tail}");
        check_key(&raw)?;
        if normalise(&base).is_ok() {
            let result = normalise(&raw);
            prop_assert!(
                matches!(&result, Ok(NormKey { key, .. }) if key.is_ascii())
                    || result == Err(KeyError::TooLong),
                "{:?} gave {:?}", raw, result
            );
        }
    }

    #[test]
    fn unknown_versions_refuse_everything(version in any::<u16>(), raw in any_text()) {
        prop_assume!(version != 1);
        prop_assert_eq!(normalise_v(version, &raw), Err(KeyError::UnknownVersion(version)));
        prop_assert!(!is_normal(&raw, version));
    }
}
