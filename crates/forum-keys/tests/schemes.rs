//! Unit tests of the key rules that the golden files do not show well: the length limits, the
//! versions, the K2 oracle, the reserved schemes, and the Unicode version that version 1 is frozen
//! against (design D.11.14, D.12.1).

use forum_keys::{
    KeyError, NormKey, VERSION, is_normal, normalise, normalise_principal, normalise_v,
};

fn key(raw: &str) -> Result<String, KeyError> {
    normalise(raw).map(|n| n.key)
}

/// `name` with its first letter upper-cased (`Api_key`).
fn capitalise(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_ascii_uppercase().to_string() + chars.as_str()
    })
}

/// `name` with its first byte percent-encoded (`%61pi_key`).
fn percent_encode_first(name: &str) -> String {
    name.bytes()
        .next()
        .map_or_else(String::new, |first| format!("%{first:02X}{}", &name[1..]))
}

#[test]
fn the_current_version_is_1() {
    assert_eq!(VERSION, 1);
    assert_eq!(
        normalise("chat:Team/t"),
        Ok(NormKey {
            key: "chat:team/t".into(),
            version: VERSION
        })
    );
    assert_eq!(
        normalise("chat:Team/t"),
        normalise_v(VERSION, "chat:Team/t")
    );
}

#[test]
fn old_versions_stay_callable_and_unknown_ones_are_refused() {
    // Version 1 is callable by number, whatever VERSION is later (K2 checks stored keys with it).
    assert_eq!(
        normalise_v(1, "issue:P#007").map(|n| (n.key, n.version)),
        Ok(("issue:p#7".into(), 1))
    );
    // Version 0 is the core's own (user:e<n> on routing, repeat:<id>:<slot>), not this crate's.
    assert_eq!(normalise_v(0, "chat:a/b"), Err(KeyError::UnknownVersion(0)));
    assert_eq!(normalise_v(2, "chat:a/b"), Err(KeyError::UnknownVersion(2)));
    assert_eq!(
        normalise_v(u16::MAX, ""),
        Err(KeyError::UnknownVersion(u16::MAX))
    );
}

#[test]
fn is_normal_is_the_k2_oracle() {
    assert!(is_normal("chat:team/thread-1", 1));
    assert!(is_normal("user:e12", 1));
    assert!(is_normal("person:mail:bob@corp.example", 1));
    assert!(is_normal("https://example.com/a?b=1", 1));
    // Not in normal form: normalising would change them.
    assert!(!is_normal("chat:Team/thread-1", 1));
    assert!(!is_normal(" chat:team/thread-1", 1));
    assert!(!is_normal("user:12", 1));
    assert!(!is_normal("https://example.com", 1));
    assert!(!is_normal("https://example.com/a?token=x", 1));
    assert!(!is_normal("arxiv:2401.12345v2", 1));
    // Refused keys are never normal.
    assert!(!is_normal("repeat:12:20261001T0900Z", 1));
    assert!(!is_normal("", 1));
    // Unknown versions, the core's version 0 included, accept nothing.
    assert!(!is_normal("chat:team/thread-1", 0));
    assert!(!is_normal("chat:team/thread-1", 2));
    assert!(!is_normal("user:e12", 0));
}

#[test]
fn repeat_is_reserved_for_the_server() {
    // A-90: only the server makes occurrence keys.
    for raw in [
        "repeat:12:20261001T0900Z",
        "Repeat:12:20261001T0900Z",
        " REPEAT:x ",
        "repeat:",
    ] {
        assert_eq!(key(raw), Err(KeyError::BadScheme), "{raw:?}");
    }
    // Only the scheme is reserved, not the word.
    assert_eq!(key("note:repeat:12"), Ok("note:repeat:12".into()));
    assert_eq!(key("repeats:x"), Ok("repeats:x".into()));
}

#[test]
fn user_is_reserved_in_principals_but_names_entries_in_keys() {
    // CS-1: a principal is never a user: key, because user:e12 names a log entry.
    for raw in ["user:e12", "USER:e12", "user:alice", "user:"] {
        assert_eq!(
            normalise_principal(raw),
            Err(KeyError::BadScheme),
            "{raw:?}"
        );
    }
    assert_eq!(normalise_principal(" User "), Ok("user".into()));
    // As a key, user: names an entry (§8.2).
    assert_eq!(key("user:12"), Ok("user:e12".into()));
    // And a person may have the reserved principal as an alias (D.2.4).
    assert_eq!(key("person:User"), Ok("person:user".into()));
    assert_eq!(key("person:user:e12"), Err(KeyError::BadScheme));
}

#[test]
fn message_level_origins_are_keys() {
    // A-61: a request's origin is the message it came in; the normaliser checks the shape only.
    assert_eq!(
        key("chat:General/1712345678.123456/1712345699.000200"),
        Ok("chat:general/1712345678.123456/1712345699.000200".into())
    );
    assert_eq!(
        key("issue:Payments#0042/note_7"),
        Ok("issue:payments#42/note_7".into())
    );
    assert_eq!(
        key("mail:<Reply-2.Abc@Mail.Example>"),
        Ok("mail:Reply-2.Abc@mail.example".into())
    );
    // The message is distinct from its conversation.
    assert_ne!(
        key("chat:general/1712345678.123456/1712345699.000200"),
        key("chat:general/1712345678.123456")
    );
    assert_ne!(key("issue:payments#42/note_7"), key("issue:payments#42"));
}

#[test]
fn a_key_is_at_most_512_bytes() {
    let at_limit = format!("note:{}", "a".repeat(507));
    assert_eq!(at_limit.len(), 512);
    assert_eq!(key(&at_limit), Ok(at_limit.clone()));
    assert_eq!(key(&format!("{at_limit}a")), Err(KeyError::TooLong));
    // Bytes, not characters: 'é' is two bytes.
    let multibyte = format!("note:{}", "\u{e9}".repeat(254));
    assert_eq!(multibyte.len(), 513);
    assert_eq!(key(&multibyte), Err(KeyError::TooLong));
    // The limit applies to the normal form, not to the input.
    let padded = format!("{}chat:a/b{}", " ".repeat(600), "\n".repeat(600));
    assert_eq!(key(&padded), Ok("chat:a/b".into()));
    let signed = format!("https://x.example/f?token={}", "t".repeat(600));
    assert_eq!(key(&signed), Ok("https://x.example/f".into()));
    let long_url = format!("https://x.example/{}", "p".repeat(512));
    assert_eq!(key(&long_url), Err(KeyError::TooLong));
    // Lower-casing may lengthen a value: 'İ' (2 bytes) lower-cases to "i̇" (3 bytes).
    let growing = format!("chat:{}/t", "\u{130}".repeat(250));
    assert_eq!(growing.len(), 507);
    assert_eq!(key(&growing), Err(KeyError::TooLong));
    // A later round may shorten a key, and the limit is measured after it: "J\u{30C}" lower-cases
    // to "j\u{30C}" (3 bytes), which NFC composes to 'ǰ' (2 bytes) in the second round.
    let shrinking = format!("chat:{}/t", "J\u{30C}".repeat(170));
    let shrunk = format!("chat:{}/t", "\u{1F0}".repeat(170));
    assert_eq!((shrinking.len(), shrunk.len()), (517, 347));
    assert_eq!(key(&shrinking), Ok(shrunk.clone()));
    assert_eq!(key(&shrunk), Ok(shrunk));
    // Whitespace inside a mail id's angle brackets is not part of the normal form either.
    let padded_id = format!("mail:<x@Y{}>", " ".repeat(600));
    assert_eq!(key(&padded_id), Ok("mail:x@y".into()));
}

#[test]
fn a_principal_key_is_at_most_256_bytes() {
    let at_limit = format!("slack:{}", "u".repeat(250));
    assert_eq!(at_limit.len(), 256);
    assert_eq!(normalise_principal(&at_limit), Ok(at_limit.clone()));
    let over = format!("{at_limit}u");
    assert_eq!(normalise_principal(&over), Err(KeyError::TooLong));
    // A person key holds a whole principal key and stays within the key limit.
    assert_eq!(
        key(&format!("person:{at_limit}")),
        Ok(format!("person:{at_limit}"))
    );
    assert_eq!(key(&format!("person:{over}")), Err(KeyError::TooLong));
    // The limit is measured on the normal form, after every round.
    let shrinking = format!("mail:{}@x", "J\u{30C}".repeat(90));
    assert_eq!(shrinking.len(), 277);
    let shrunk = format!("mail:{}@x", "\u{1F0}".repeat(90));
    assert_eq!(shrunk.len(), 187);
    assert_eq!(normalise_principal(&shrinking), Ok(shrunk.clone()));
    assert_eq!(
        key(&format!("person:{shrinking}")),
        Ok(format!("person:{shrunk}"))
    );
    let padded = format!("mail:<bob@x{}>", " ".repeat(300));
    assert_eq!(normalise_principal(&padded), Ok("mail:bob@x".into()));
}

#[test]
fn credential_parameters_never_survive_in_a_url_key() {
    // CS-35: X-Amz-* and X-Goog-*, and sig, signature, token, access_token, key, api_key, apikey,
    // password, passwd, secret, and client_secret, in any ASCII case.
    let k = key(concat!(
        "https://b.s3.example/o?x-AMZ-date=1&a=1&Sig=2&sig=3",
        "&ACCESS_TOKEN=4&X-Goog-Signature=5&b=6#f"
    ))
    .unwrap();
    assert_eq!(k, "https://b.s3.example/o?a=1&b=6");
    for name in [
        "api_key",
        "apikey",
        "password",
        "passwd",
        "secret",
        "client_secret",
    ] {
        for spelt in [
            name.to_owned(),
            name.to_ascii_uppercase(),
            capitalise(name),
            percent_encode_first(name),
            format!("+{name}"),
            format!("%20{name}"),
            format!("{name}%00"),
            format!("{name}%00x"),
        ] {
            for raw in [
                format!("https://x.example/a?{spelt}=v&b=1"),
                format!("https://x.example/a?b=1&{spelt}=v"),
                format!("https://x.example/a?b=1&{spelt}"),
                format!("https://x.example/a?b=1&{spelt}="),
            ] {
                assert_eq!(key(&raw), Ok("https://x.example/a?b=1".into()), "{raw:?}");
            }
            assert_eq!(
                key(&format!("https://x.example/a?{spelt}=v")),
                Ok("https://x.example/a".into()),
                "{spelt:?}"
            );
        }
    }
    // Only the whole name counts: a longer or a differently punctuated name is kept.
    let near_misses = concat!(
        "https://x.example/a?api-key=1&api_keys=2&apikeys=3&secrets=4&my_secret=5",
        "&client_id=6&passwords=7&pass=8&clientsecret=9"
    );
    assert_eq!(key(near_misses), Ok(near_misses.into()));
    // Non-ASCII look-alikes are not folded: 'K' below is U+212A KELVIN SIGN, percent-encoded.
    assert_eq!(
        key("https://x.example/a?api_%E2%84%AAey=1"),
        Ok("https://x.example/a?api_%E2%84%AAey=1".into())
    );
    // A dropped parameter that is percent-encoded or has no value is still dropped.
    assert_eq!(
        key("https://x.example/?a=1&%6Bey=2&token"),
        Ok("https://x.example/?a=1".into())
    );
    // The user information is not part of the object either: with it, a key names the same object
    // as without it.
    assert_eq!(
        key("https://user:pass@x.example/a"),
        key("https://x.example/a")
    );
    assert_eq!(
        key("https://Alice:s3cret@X.Example:443/a?token=t#f"),
        Ok("https://x.example/a".into())
    );
    assert!(!is_normal("https://user:pass@x.example/a", 1));
    // An empty query, or one left empty, loses its '?'.
    assert_eq!(key("https://x.example/a?"), key("https://x.example/a"));
    assert_eq!(key("https://x.example/a?&"), key("https://x.example/a"));
}

#[test]
fn credentials_are_found_behind_every_separator_and_in_php_spellings() {
    // Every "&amp;" in a query reads as '&' first; then the query splits into pairs at '&', and a
    // pair into pieces at ';'. A name is read as PHP reads it: decoded, without the spaces in front
    // of it, ended at its first NUL, and with every space, '.', and '[' read as '_'.
    let names = [
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
        "X-Amz-Signature",
        "x-goog-credential",
    ];
    for name in names {
        let spellings = [
            name.to_owned(),
            name.to_ascii_uppercase(),
            name.replace('_', "."),
            name.replace('_', "["),
            name.replace('_', "%2E"),
            name.replace('_', "%5B"),
            name.replace('_', "+"),
            name.replace('_', "%20"),
            capitalise(&name.replace('_', ".")),
            format!("+{name}"),
            format!("%20%20{name}"),
            format!("+%20{}", name.replace('_', "+")),
            format!("{name}%00"),
            format!("{name}%00x"),
            format!("{name}%00%00[]"),
            format!("+{}%00+", name.replace('_', ".")),
        ];
        for spelt in spellings {
            let mut cases = Vec::new();
            // Behind '&', "&amp;", or "&amp;amp;", the credential is a pair of its own, and only it
            // is dropped; the pairs kept are joined by '&'.
            for separator in ["&", "&amp;", "&amp;amp;"] {
                cases.extend([
                    (format!("a=1{separator}{spelt}=v"), "a=1".to_owned()),
                    (
                        format!("a=1{separator}{spelt}=v{separator}b=2"),
                        "a=1&b=2".to_owned(),
                    ),
                    (
                        format!("a=1{separator}{spelt}{separator}b=2"),
                        "a=1&b=2".to_owned(),
                    ),
                    (format!("c=3{separator}{spelt}=v;b=2"), "c=3".to_owned()),
                ]);
            }
            // Behind ';', the credential and every piece after it in its pair are dropped; a pair
            // that starts with it is dropped whole.
            cases.extend([
                (format!("a=1;{spelt}=v"), "a=1".to_owned()),
                (format!("a=1;{spelt}=v;b=2"), "a=1".to_owned()),
                (format!("a=1;{spelt};b=2&c=3"), "a=1&c=3".to_owned()),
                (format!("{spelt}=v;b=2&c=3"), "c=3".to_owned()),
                (format!("c=3&{spelt}=v;b=2"), "c=3".to_owned()),
                (format!("{spelt}=v&c=3"), "c=3".to_owned()),
                (spelt.clone(), String::new()),
            ]);
            for (raw, expected) in cases {
                let url = format!("https://x.example/p?{raw}");
                let want = if expected.is_empty() {
                    "https://x.example/p".to_owned()
                } else {
                    format!("https://x.example/p?{expected}")
                };
                assert_eq!(key(&url), Ok(want), "{url:?}");
            }
        }
    }
    // What is kept is kept as it is: the pairs are joined by '&' again, and a piece keeps its ';'.
    assert_eq!(
        key("https://x.example/?token=1;a=2&amp;b=3&c=4"),
        Ok("https://x.example/?b=3&c=4".into())
    );
    assert_eq!(
        key("https://x.example/?a=1;token=2&b=3"),
        Ok("https://x.example/?a=1&b=3".into())
    );
    // The first pair has no '&' in front of it, so a leading "amp" is a piece of its own.
    assert_eq!(
        key("https://x.example/?amp;token=1"),
        Ok("https://x.example/?amp".into())
    );
    // A query without "&amp;", credentials, or empty pairs is left as it is, whatever its
    // separators.
    for query in [
        "a=1;b=2",
        "a=1&b=2;c=3",
        "amp;b",
        "amp;amp;b=1&c=2",
        "a=1;amp;b=2",
        "a&amp=1",
        "a=1&Amp;b=2",
        "a=1;;b=2;",
        ";a=1",
        "a=1&;",
    ] {
        let url = format!("https://x.example/?{query}");
        assert_eq!(key(&url), Ok(url.clone()), "{url:?}");
    }
    // '%26' and '%3B' are data, not separators.
    for encoded in [
        "https://x.example/?a=%26token%3D1%3Bkey",
        "https://x.example/?a=1%3Btoken=2",
        "https://x.example/?a=1%26amp;b=2",
    ] {
        assert_eq!(key(encoded), Ok(encoded.into()));
    }
    assert_eq!(
        key("https://x.example/?a=1%26amp;token=2"),
        Ok("https://x.example/?a=1%26amp".into())
    );
    // Every other spelling is kept: PHP's arrays, a doubled or leading '.', a space after the name,
    // a name behind another whitespace character or behind a NUL, a non-breaking space, and other
    // punctuation.
    let kept = concat!(
        "https://x.example/?api[key]=1&api_key[]=3&api.key[]=4&api..key=5&.token=6&token.=7",
        "&api-key=8&api/key=9&api:key=10&api_key+=11&token%20=12&%09api_key=13&%0Atoken=14",
        "&%00api_key=15&api%00_key=16&%C2%A0token=17&api%C2%A0key=18&_token=19&api__key=20"
    );
    assert_eq!(key(kept), Ok(kept.into()));
}

#[test]
fn a_prefix_matches_arrays_and_a_whole_name_does_not() {
    // PHP files `password[]=x` and `token[x]=y` under the arrays `password` and `token`, but a
    // credential name matches the whole name only, so an array of one is kept.
    let kept = concat!(
        "https://x.example/?password[]=hunter2&token[x]=S&api_key[]=1&api.key[]=2",
        "&Secret[0]=3&client_secret%5B%5D=4&b=1"
    );
    assert_eq!(key(kept), Ok(kept.into()));
    // A name that starts with x-amz- or x-goog- is a credential whatever follows the prefix, an
    // array's brackets included, however they are spelt.
    for name in [
        "X-Amz-Date[]",
        "x-amz-signature[0]",
        "X-Amz-Date%5B%5D",
        "X-Goog-Credential[]",
        "x-goog-meta[k]",
        "+X-Amz-Date[]",
    ] {
        for raw in [
            format!("https://x.example/?{name}=1&b=1"),
            format!("https://x.example/?b=1&{name}=1"),
            format!("https://x.example/?b=1;{name}=1;c=2"),
        ] {
            assert_eq!(key(&raw), Ok("https://x.example/?b=1".into()), "{raw:?}");
        }
    }
}

#[test]
fn html_escaped_ampersands_are_read_as_ampersands_in_one_round() {
    // A URL copied from a page, with every '&' of its query escaped as "&amp;", is the URL itself,
    // and so it is when the page escaped it twice or more: a '&' and every "amp;" after it in a row
    // are one '&'.
    for query in [
        "a=1&b=2",
        "a=1&b=2&token=3&c=4",
        "a=1;token=2&b=3",
        "a=1&&b=2&",
        "&a=1",
        "a=1&;b=2",
        "amp;b=1&c=2",
        "q=a;;b&r=1",
    ] {
        let plain = key(&format!("https://x.example/p?{query}"));
        for depth in 1..=4 {
            let escaped = query.replace('&', &format!("&{}", "amp;".repeat(depth)));
            assert_eq!(
                key(&format!("https://x.example/p?{escaped}")),
                plain,
                "{escaped:?}"
            );
        }
    }
    // However deep the nesting, it is decoded in one round, and the normal form's query holds no
    // "&amp;": the normaliser's round limit is never reached.
    for depth in [1, 3, 4, 5, 16, 1_000, 100_000] {
        let raw = format!(
            "https://x.example/p?a=1&{}token=2&{}b=3",
            "amp;".repeat(depth),
            "amp;".repeat(depth)
        );
        assert_eq!(
            key(&raw),
            Ok("https://x.example/p?a=1&b=3".into()),
            "depth {depth}"
        );
    }
    // A key is measured after the decoding.
    let long = format!("https://x.example/p?a=1&{}b=2", "amp;".repeat(200));
    assert!(long.len() > 512);
    assert_eq!(key(&long), Ok("https://x.example/p?a=1&b=2".into()));
    // "&amp;" in a path is not a separator, and is kept.
    assert_eq!(
        key("https://x.example/a&amp;b?c=1&amp;d=2"),
        Ok("https://x.example/a&amp;b?c=1&d=2".into())
    );
    assert_ne!(
        key("https://x.example/a&amp;b"),
        key("https://x.example/a&b")
    );
}

#[test]
fn no_html_entity_but_amp_is_read() {
    // `&#038;` (as WordPress's esc_url writes '&'), `&#38;`, and `&#x26;` end the query at their
    // '#', which starts a fragment, and the fragment is stripped, so two pages of one post share a
    // key.
    for entity in ["&#038;", "&#38;", "&#x26;", "&#X26;"] {
        for page in ["2", "3"] {
            assert_eq!(
                key(&format!("https://blog.example/?p=1{entity}page={page}")),
                Ok("https://blog.example/?p=1".into()),
                "{entity}"
            );
        }
        assert_eq!(
            key(&format!("https://blog.example/?p=1{entity}token=S")),
            Ok("https://blog.example/?p=1".into()),
            "{entity}"
        );
    }
    // Any other entity is data: `?q=&quot;x&quot;` is not `?q="x"`, which is `?q=%22x%22`.
    let quoted = "https://x.example/?q=&quot;x&quot;&lt;&gt;";
    assert_eq!(key(quoted), Ok(quoted.into()));
    assert_eq!(
        key("https://x.example/?q=\"x\""),
        Ok("https://x.example/?q=%22x%22".into())
    );
    // Its '&' splits the query all the same, so a credential after one is still a pair of its own.
    assert_eq!(
        key("https://x.example/?a=1&quot;&token=S&lt;b=2"),
        Ok("https://x.example/?a=1&quot;&lt;b=2".into())
    );
}

#[test]
fn nothing_of_a_credential_value_is_kept_however_a_server_splits_the_query() {
    // To a server that splits at '&' alone, `password=hunter;2` is one parameter, whose value is
    // `hunter;2`; to one that splits at ';' too, `password=hunter` is. Neither part is kept.
    for (raw, want) in [
        (
            "https://x.example/a?password=hunter;2&b=1",
            "https://x.example/a?b=1",
        ),
        ("https://x.example/a?token=abc;def", "https://x.example/a"),
        (
            "https://x.example/a?sig=AAAA;BBBB=CC",
            "https://x.example/a",
        ),
        (
            "https://x.example/a?a=1;token=2;3&b=4",
            "https://x.example/a?a=1&b=4",
        ),
        (
            "https://x.example/a?a=1&amp;token=2;3&b=4",
            "https://x.example/a?a=1&b=4",
        ),
        (
            "https://x.example/a?a=1&;token=2",
            "https://x.example/a?a=1",
        ),
        ("https://x.example/a?;token=2", "https://x.example/a"),
        (
            "https://x.example/a?a=1;+api+key=2;3&b=4",
            "https://x.example/a?a=1&b=4",
        ),
    ] {
        assert_eq!(key(raw), Ok(want.into()), "{raw:?}");
    }
    // And no kept piece takes a ';' that sat inside a credential's value, which would make the key
    // that of another URL.
    assert_eq!(
        key("https://x.example/a?a=1&token=2;b=3"),
        Ok("https://x.example/a?a=1".into())
    );
    assert_ne!(
        key("https://x.example/a?a=1&token=2;b=3"),
        key("https://x.example/a?a=1;b=3")
    );
}

#[test]
fn only_empty_pairs_between_ampersands_are_dropped() {
    // An empty piece around a ';' is kept, so each of these URLs keeps a key of its own.
    for (one, other) in [
        ("?style=color:red;", "?style=color:red"),
        ("?q=a;;b", "?q=a;b"),
        ("?;a=1", "?a=1"),
        ("?a=1&;", "?a=1"),
        ("?;", ""),
    ] {
        let one = format!("https://x.example/a{one}");
        let other = format!("https://x.example/a{other}");
        assert_eq!(key(&one), Ok(one.clone()), "{one:?}");
        assert_eq!(key(&other), Ok(other.clone()), "{other:?}");
    }
    // An empty pair, between two '&' or at either end of the query, is dropped, after "&amp;" is
    // read as '&', and a query left empty loses its '?'.
    for (raw, want) in [
        ("?&a=1&&b=2&", "?a=1&b=2"),
        ("?&", ""),
        ("?&&", ""),
        ("?", ""),
        ("?&amp;", ""),
        ("?a=1&amp;", "?a=1"),
        ("?a=1&amp;&amp;b=2", "?a=1&b=2"),
        ("?a=1&amp;amp;&b=2", "?a=1&b=2"),
        ("?a=1&amp;;b=2", "?a=1&;b=2"),
    ] {
        assert_eq!(
            key(&format!("https://x.example/a{raw}")),
            Ok(format!("https://x.example/a{want}")),
            "{raw:?}"
        );
    }
}

#[test]
fn research_keys_name_one_object() {
    // CS-36: one paper or one statement is one key.
    assert_eq!(key("arxiv:2401.12345v2"), key("arxiv:2401.12345"));
    assert_eq!(key("arxiv:arXiv:2401.12345v3"), key("arxiv:2401.12345"));
    assert_eq!(key("arxiv:Hep-Th/9901001v2"), key("arxiv:hep-th/9901001"));
    assert_eq!(
        key("doi:https://doi.org/10.1000/XYZ"),
        key("doi:10.1000/xyz")
    );
    assert_eq!(key("doi:doi.org/10.1000/XYZ"), key("doi:10.1000/xyz"));
    assert_eq!(key("lean:ABCDEF0123"), key("lean:abcdef0123"));
    assert_eq!(key("run:ABCDEF0123"), key("run:abcdef0123"));
    assert_ne!(key("lean:abcdef0123"), key("run:abcdef0123"));
    assert!(matches!(key("lean:not-hex"), Err(KeyError::BadValue(_))));
    assert!(matches!(key("run:12g4"), Err(KeyError::BadValue(_))));
}

#[test]
fn schemes() {
    // `[a-z][a-z0-9+.-]{0,15}` after lower-casing.
    for ok in ["a", "x9", "a+b.c-d", "abcdefghijklmnop", "Note", "A+B"] {
        assert_eq!(
            key(&format!("{ok}:x")),
            Ok(format!("{}:x", ok.to_ascii_lowercase())),
            "{ok:?}"
        );
    }
    for bad in [
        "1chat",
        "+a",
        "ch_at",
        "ch at",
        "ch\u{e4}t",
        "abcdefghijklmnopq",
    ] {
        assert_eq!(
            key(&format!("{bad}:x")),
            Err(KeyError::BadScheme),
            "{bad:?}"
        );
    }
    assert_eq!(key(":x"), Err(KeyError::NoScheme));
    // The default conversation schemes (CS-36) are schemes.
    for conversation in ["chat:team/t", "issue:p#1", "mail:id@x"] {
        assert!(is_normal(conversation, 1), "{conversation:?}");
    }
}

#[test]
fn a_person_key_names_what_is_wrong_with_its_principal() {
    // A value without a scheme is refused as a person key, not as a key without a scheme.
    for raw in ["person:bob", "person::bob", "person: bob"] {
        match key(raw) {
            Err(KeyError::BadValue(why)) => assert!(why.starts_with("person:"), "{raw:?}: {why}"),
            other => panic!("{raw:?} gave {other:?}, expected BadValue"),
        }
    }
    // The reserved scheme and a malformed one keep their own errors.
    assert_eq!(key("person:user:e12"), Err(KeyError::BadScheme));
    assert_eq!(key("person:1x:y"), Err(KeyError::BadScheme));
}

/// The default-ignorable code points that Unicode 17.0 assigns, without the variation selectors
/// (U+180B to U+180D, U+180F, U+FE00 to U+FE0F, and U+E0100 to U+E01EF): 145 in all, by kind, each
/// kind with a word of its refusal.
const REFUSED_INVISIBLE: [(u32, u32, &str); 19] = [
    (0xAD, 0xAD, "soft hyphen"),
    (0x34F, 0x34F, "combining grapheme joiner"),
    (0x61C, 0x61C, "bidirectional control"),
    (0x115F, 0x1160, "Hangul filler"),
    (0x17B4, 0x17B5, "Khmer inherent vowel"),
    (0x180E, 0x180E, "Mongolian vowel separator"),
    (0x200B, 0x200D, "zero-width format character"),
    (0x200E, 0x200F, "bidirectional control"),
    (0x202A, 0x202E, "bidirectional control"),
    (0x2060, 0x2064, "zero-width format character"),
    (0x2066, 0x2069, "bidirectional control"),
    (0x206A, 0x206F, "deprecated format character"),
    (0x3164, 0x3164, "Hangul filler"),
    (0xFEFF, 0xFEFF, "zero-width format character"),
    (0xFFA0, 0xFFA0, "Hangul filler"),
    (0x1BCA0, 0x1BCA3, "shorthand format control"),
    (0x1D173, 0x1D17A, "musical symbol format control"),
    (0xE0001, 0xE0001, "tag character"),
    (0xE0020, 0xE007F, "tag character"),
];

fn refused_invisible() -> impl Iterator<Item = (char, &'static str)> {
    REFUSED_INVISIBLE.iter().flat_map(|&(first, last, rule)| {
        (first..=last).filter_map(move |cp| char::from_u32(cp).map(|c| (c, rule)))
    })
}

#[test]
fn default_ignorables_are_refused_even_at_the_edges() {
    assert_eq!(refused_invisible().count(), 145);
    for (c, rule) in refused_invisible() {
        let cp = u32::from(c);
        for raw in [
            format!("note:a{c}b"),
            format!("{c}note:x"),
            format!("note:x{c}"),
            format!(" \t{c}note:x\n"),
            format!("note:x{c} "),
            format!("no{c}te:x"),
            format!("chat:team/{c}t"),
            format!("issue:p#1{c}"),
            format!("mail:<a@b{c}>"),
            format!("user:e1{c}"),
            format!("person:slack:{c}x"),
            format!("doi:10.1/{c}"),
            format!("http{c}s://x.example/"),
            c.to_string(),
        ] {
            // The refusal names its kind, not merely BadValue.
            assert!(
                matches!(key(&raw), Err(KeyError::BadValue(why)) if why.contains(rule)),
                "U+{cp:04X} in {raw:?} gave {:?}",
                key(&raw)
            );
        }
        for raw in [
            format!("slack:a{c}b"),
            format!("{c}slack:x"),
            format!("slack:x{c}"),
            format!(" {c}slack:x "),
            format!("mail:bob{c}@x"),
            format!("https://x.example/{c}"),
            format!("{c}user"),
            format!("user{c}"),
        ] {
            assert!(
                matches!(
                    normalise_principal(&raw),
                    Err(KeyError::BadValue(why)) if why.contains(rule)
                ),
                "U+{cp:04X} in {raw:?}"
            );
        }
    }
    // A look-alike of a key is not that key: it is refused rather than stored beside it.
    assert_eq!(key("chat:team/t"), Ok("chat:team/t".into()));
    assert!(key("chat:team/\u{200B}t").is_err());
    assert!(key("chat:team/t\u{AD}").is_err());
    assert_eq!(normalise_principal("user"), Ok("user".into()));
    assert!(normalise_principal("us\u{200B}er").is_err());
    assert!(normalise_principal("user\u{E0041}").is_err());
    assert!(normalise_principal("\u{3164}").is_err());
    // An emoji sequence joined by U+200D ZERO WIDTH JOINER, and a subdivision flag spelt with tag
    // characters, are refused with them.
    for raw in [
        "note:\u{1F469}\u{200D}\u{1F4BB}",
        "note:\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
    ] {
        assert!(matches!(key(raw), Err(KeyError::BadValue(_))), "{raw:?}");
    }
}

#[test]
fn the_variation_selectors_and_visible_format_characters_are_kept() {
    // The variation selectors (the `Variation_Selector` property) choose a glyph for the character
    // before them, so a key may need one; they are the only default-ignorable code points kept.
    let selectors: Vec<char> = ('\u{180B}'..='\u{180D}')
        .chain(['\u{180F}'])
        .chain('\u{FE00}'..='\u{FE0F}')
        .chain('\u{E0100}'..='\u{E01EF}')
        .collect();
    assert_eq!(selectors.len(), 260);
    for c in selectors {
        let cp = u32::from(c);
        assert_eq!(
            key(&format!("note:a{c}b")),
            Ok(format!("note:a{c}b")),
            "U+{cp:04X}"
        );
        assert_eq!(
            key(&format!("note:x{c}")),
            Ok(format!("note:x{c}")),
            "U+{cp:04X}"
        );
        assert_eq!(
            normalise_principal(&format!("team:Pay{c}ments")),
            Ok(format!("team:Pay{c}ments")),
            "U+{cp:04X}"
        );
        // Before the scheme, it is part of a malformed scheme.
        assert_eq!(
            key(&format!("{c}note:x")),
            Err(KeyError::BadScheme),
            "U+{cp:04X}"
        );
    }
    assert_eq!(
        key("note:\u{2764}\u{FE0F}"),
        Ok("note:\u{2764}\u{FE0F}".into())
    );
    // A Mongolian free variation selector chooses the form of the Mongolian letter before it.
    assert_eq!(
        key("chat:\u{1828}\u{180B}/t"),
        Ok("chat:\u{1828}\u{180B}/t".into())
    );
    assert_eq!(
        normalise_principal("team:\u{1820}\u{180C}"),
        Ok("team:\u{1820}\u{180C}".into())
    );
    // One that chooses no glyph for the character before it shows nothing, and is kept all the
    // same: such a key reads like the key without it, and is another key (D.11.14).
    for c in ['\u{180B}', '\u{180F}', '\u{FE00}', '\u{E0100}'] {
        let stray = format!("team:alice{c}");
        assert_eq!(normalise_principal(&stray), Ok(stray.clone()));
        assert_ne!(
            normalise_principal(&stray),
            normalise_principal("team:alice")
        );
        assert_eq!(key(&format!("note:{c}x")), Ok(format!("note:{c}x")));
        assert_ne!(key(&format!("note:x{c}")), key("note:x"));
    }
    // The Mongolian vowel separator is not a variation selector, and is refused.
    assert!(matches!(
        key("note:\u{1820}\u{180E}\u{1820}"),
        Err(KeyError::BadValue(why)) if why.contains("Mongolian vowel separator")
    ));
    // Visible format characters are kept.
    assert_eq!(key("note:\u{600}1"), Ok("note:\u{600}1".into()));
    assert_eq!(
        normalise_principal("team:\u{FFF9}x\u{FFFB}"),
        Ok("team:\u{FFF9}x\u{FFFB}".into())
    );
}

#[test]
fn line_and_paragraph_separators_are_refused_inside_and_trimmed_at_the_edges() {
    for c in ['\u{2028}', '\u{2029}'] {
        let cp = u32::from(c);
        for raw in [
            format!("note:a{c}b"),
            format!("chat:team/t{c}INJECTED: x"),
            format!("no{c}te:x"),
            format!("person:slack:x{c}y"),
        ] {
            assert!(
                matches!(key(&raw), Err(KeyError::BadValue(why)) if why.contains("U+2028 or U+2029")),
                "U+{cp:04X} in {raw:?}"
            );
        }
        assert!(
            matches!(
                normalise_principal(&format!("slack:a{c}b")),
                Err(KeyError::BadValue(why)) if why.contains("U+2028 or U+2029")
            ),
            "U+{cp:04X}"
        );
        // They are whitespace, so they are trimmed at the edges, like a line feed.
        assert_eq!(key(&format!("{c}note:x{c}")), Ok("note:x".into()));
        assert_eq!(
            normalise_principal(&format!("{c}slack:x{c}")),
            Ok("slack:x".into())
        );
    }
}

#[test]
fn url_keys_are_checked_in_their_ascii_normal_form() {
    // A host whose punycode holds U+200C or U+200D is accepted however it is sent.
    let persian = "https://xn--mgbn2ecje63gr19l.example/";
    for raw in [
        "https://\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}.example/",
        "https://%D9%85%DB%8C%E2%80%8C%D8%AE%D9%88%D8%A7%D9%87%D9%85.example/",
        persian,
    ] {
        assert_eq!(key(raw), Ok(persian.into()), "{raw:?}");
    }
    assert_eq!(
        key("https://\u{915}\u{94D}\u{200D}\u{937}.example/"),
        Ok("https://xn--11b2ezcw70k.example/".into())
    );
    // Every refused character is percent-encoded in a path and a query, and stripped with a
    // fragment; in a host the parser drops it or refuses the URL.
    let others = [
        '\u{0}', '\u{7F}', '\u{85}', '\u{9F}', '\u{2028}', '\u{2029}', '\u{378}',
    ];
    for c in refused_invisible().map(|(c, _)| c).chain(others) {
        let cp = u32::from(c);
        let encoded: String = c.to_string().bytes().map(|b| format!("%{b:02X}")).collect();
        assert_eq!(
            key(&format!("https://x.example/a{c}b?q={c}#f{c}")),
            Ok(format!("https://x.example/a{encoded}b?q={encoded}")),
            "U+{cp:04X}"
        );
        // The parser drops it from a host, or refuses the host, and the refusal names it.
        match key(&format!("https://x{c}.example/")) {
            Ok(normal) => assert_eq!(normal, "https://x.example/", "U+{cp:04X}"),
            Err(e) => assert_eq!(
                Err(e),
                normalise_principal(&format!("note:a{c}b")),
                "U+{cp:04X}"
            ),
        }
    }
    // The parser drops a tab, a carriage return, and a line feed wherever they are, and C0 control
    // characters and spaces at the end of the key; inside, it percent-encodes a control character.
    assert_eq!(
        key("https://x.example/a\tb\r\nc?d=\n1"),
        Ok("https://x.example/abc?d=1".into())
    );
    for (raw, want) in [
        ("https://x.example/a\u{1}", "https://x.example/a"),
        ("https://x.example/a?x=1\u{1F}", "https://x.example/a?x=1"),
        ("https://x.example/a\u{1} \u{1F}", "https://x.example/a"),
        ("https://x.example/a\u{1}b", "https://x.example/a%01b"),
        ("https://x.example/a\u{0}b", "https://x.example/a%00b"),
        ("https://x.example/a\u{85}b", "https://x.example/a%C2%85b"),
    ] {
        assert_eq!(key(raw), Ok(want.into()), "{raw:?}");
    }
    // A refused character before the scheme's ':' makes the scheme another one, which is checked
    // as it is sent.
    assert!(matches!(
        key("http\u{200B}s://x.example/"),
        Err(KeyError::BadValue(_))
    ));
    // Every URL normal form is ASCII.
    for raw in [
        "https://B\u{FC}cher.example/\u{E9}t\u{E9}?q=\u{263A}",
        "https://x.example/a%E2%80%8Bb?c=%E2%80%AE",
        "https://ex\u{AD}ample.com/\u{AD}",
        "https://x\u{3164}.example/\u{E0041}?\u{2066}=\u{200B}",
    ] {
        let normal = key(raw).unwrap();
        assert!(normal.is_ascii(), "{raw:?} gave {normal:?}");
        assert!(is_normal(&normal, 1), "{normal:?}");
    }
}

#[test]
fn a_url_host_drops_the_default_ignorables_that_uts_46_ignores() {
    // In a host, UTS 46 ignores every default-ignorable code point but the bidirectional controls,
    // U+200C, U+200D, and the tag characters, the variation selectors included, so the parser drops
    // it; it refuses those others in a Latin label, and the refusal names the character.
    let selectors = ('\u{180B}'..='\u{180D}')
        .chain(['\u{180F}'])
        .chain('\u{FE00}'..='\u{FE0F}')
        .chain('\u{E0100}'..='\u{E01EF}');
    let (mut dropped, mut refused) = (0, 0);
    for c in refused_invisible().map(|(c, _)| c).chain(selectors) {
        let cp = u32::from(c);
        let raw = format!("https://a{c}b.example/");
        let kept_by_uts_46 = matches!(
            c,
            '\u{61C}'
                | '\u{200C}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        );
        if kept_by_uts_46 {
            let why = key(&format!("note:a{c}b"));
            assert!(why.is_err(), "U+{cp:04X}");
            assert_eq!(key(&raw), why, "U+{cp:04X}");
            refused += 1;
        } else {
            assert_eq!(key(&raw), Ok("https://ab.example/".into()), "U+{cp:04X}");
            dropped += 1;
        }
    }
    assert_eq!((dropped, refused), (294, 111));
    // So a Mongolian free variation selector, which a key keeps elsewhere, is dropped from a host.
    assert_eq!(
        key("https://a\u{180B}b.example/a\u{180B}?q=\u{180B}"),
        Ok("https://ab.example/a%E1%A0%8B?q=%E1%A0%8B".into())
    );
}

#[test]
fn a_url_that_the_parser_refuses_names_the_refused_character_it_holds() {
    for (raw, why) in [
        ("https://x\u{202E}.example/", "bidirectional control"),
        ("https://x\u{E0041}.example/", "tag character"),
        ("https://x.example\u{2028}/", "U+2028 or U+2029"),
        ("https://x\u{2029}.example/", "U+2028 or U+2029"),
        ("https://x\u{0}.example/", "control character"),
        ("https://x\u{1}.example/", "control character"),
        ("https://x\u{85}.example/", "control character"),
        ("https://x\u{378}.example/", "unassigned"),
        // U+200C and U+200D are refused in a Latin label, and allowed in a Persian or Devanagari
        // one (above).
        ("https://x\u{200C}.example/", "zero-width format character"),
        ("https://x\u{200D}.example/", "zero-width format character"),
        // Wherever it is in the key, once the parser refuses the key.
        ("http://exa mple.com/\u{AD}", "soft hyphen"),
    ] {
        assert!(
            matches!(key(raw), Err(KeyError::BadValue(text)) if text.contains(why)),
            "{raw:?} gave {:?}",
            key(raw)
        );
    }
    // A key without one is refused as an invalid URL.
    for raw in ["https://", "http://exa mple.com/", "https://[::1/"] {
        assert_eq!(
            key(raw),
            Err(KeyError::BadValue("http and https keys must be valid URLs")),
            "{raw:?}"
        );
    }
}

#[test]
fn control_characters_and_unassigned_code_points_are_refused() {
    for raw in [
        "chat:team/t\nINJECTED: x",
        "chat:team/t\u{2028}INJECTED: x",
        "note:a\u{0}b",
        "note:\u{1b}[31mred",
        "note:a\u{9f}",
        "note:\u{378}",
        "note:\u{10ffff}",
    ] {
        assert!(matches!(key(raw), Err(KeyError::BadValue(_))), "{raw:?}");
        assert!(
            matches!(normalise_principal(raw), Err(KeyError::BadValue(_))),
            "{raw:?}"
        );
    }
    // At the edges, whitespace controls are trimmed like any whitespace.
    assert_eq!(key("\t\u{85}note:x\r\n"), Ok("note:x".into()));
    assert_eq!(normalise_principal("\tslack:x\u{85}"), Ok("slack:x".into()));
}

/// Version 1 and the principal rules lean on Unicode tables outside this crate: case mapping and
/// `White_Space` in `std`, NFC in `unicode-normalization`, and UTS 46 in `url`'s IDNA (held by
/// Cargo.lock and pinned by the golden files' IDNA rows). Code points that Unicode 17.0 leaves
/// unassigned are refused, so a newer table can change a normal form only through a character that
/// 17.0 assigns. Unicode's stability policies make that rare, but not impossible: Unicode 8.0 gave
/// the existing Cherokee letters lower-case mappings.
///
/// When this test fails, the toolchain or `unicode-normalization` has moved to another Unicode
/// version. Check that version's changes to the case mappings, `White_Space`, and the normalisation
/// of characters that 17.0 assigns. If none touches these rules, update the versions here. If one
/// does, keys need a new normaliser version (K2) with this one kept as it is, and principal keys,
/// which have no version, need a decision by the design owner, because a change would split one
/// human into two principals.
#[test]
fn the_unicode_tables_are_version_17() {
    assert_eq!(char::UNICODE_VERSION, (17, 0, 0));
    assert_eq!(unicode_normalization::UNICODE_VERSION, (17, 0, 0));
}

#[test]
fn errors_render_a_hint() {
    for e in [
        KeyError::Empty,
        KeyError::NoScheme,
        KeyError::BadScheme,
        KeyError::BadValue("chat: expected chat:<channel>/<thread>"),
        KeyError::TooLong,
        KeyError::UnknownVersion(7),
    ] {
        assert!(!e.to_string().is_empty(), "{e:?}");
    }
    assert_eq!(
        KeyError::UnknownVersion(7).to_string(),
        "no key normaliser has version 7"
    );
    // Every BadValue names its rule.
    for raw in [
        "chat:x",
        "issue:x",
        "mail:<>",
        "user:x",
        "https://",
        "arxiv:arXiv:",
        "doi:doi.org/",
        "lean:x",
        "note:",
    ] {
        match key(raw) {
            Err(KeyError::BadValue(why)) => assert!(!why.is_empty(), "{raw:?}"),
            other => panic!("{raw:?} gave {other:?}, expected BadValue"),
        }
    }
}
