//! Version 1 of the key normaliser (design D.11.14). Frozen once released: a stored key carries the
//! version it was normalised with, and K2 checks it against this code for ever. A change of any rule
//! here is a new version in a new module, with this one left as it is.
//!
//! Rules: NFC, then the surrounding whitespace trimmed; the character rules of
//! `crate::text::check_chars` (no control character, no U+2028 or U+2029, no default-ignorable code
//! point other than a variation selector, and no code point that Unicode 17.0 leaves unassigned),
//! read on the key as it is sent for every scheme but `http:` and `https:`, whose keys are read in
//! their normal form; the scheme lower-cased and matching `[a-z][a-z0-9+.-]{0,15}`; a non-empty
//! value; a normal form of at most 512 bytes; and per scheme:
//!
//! - `chat:<channel>/<thread>`, or `chat:<channel>/<thread>/<message>` for one message (A-61): the
//!   channel lower-cased, every part non-empty;
//! - `issue:<project>#<n>`, or `issue:<project>#<n>/<comment>` for one ticket comment (A-61): the
//!   project lower-cased, leading zeros of `n` stripped (`#` is part of the value, not a fragment);
//! - `mail:<message-id>`: one pair of angle brackets and the whitespace inside them stripped (a
//!   second pair is refused), the domain part (after the last `@`) lower-cased; a thread is named
//!   by its first message's id, a message by its own (A-61);
//! - `user:<n>` or `user:e<n>`, rendered `user:e<n>`;
//! - `person:<principal key>`: the value normalised by [`crate::normalise_principal`] (D-3);
//! - `repeat:` rejected with [`KeyError::BadScheme`]: only the server makes occurrence keys (A-90);
//! - `http:` and `https:`: parsed as WHATWG URLs (host lower-cased, default port removed), the
//!   fragment and the user name and password stripped, and the query rebuilt with every `&amp;`
//!   read as `&`, and without empty pairs and credential parameters (CS-35; see [`rebuild_query`]
//!   for how a query is decoded and split into pairs and pieces and what is dropped, and
//!   [`CREDENTIAL_PARAMS`] for the names and the spellings matched); an empty query is dropped with
//!   its `?`. The character rules read the normal form, which is ASCII (a punycode host, a
//!   percent-encoded path and query), so a host whose punycode holds U+200C or U+200D, as Persian
//!   and Devanagari names may, is accepted, and so is any character that the URL parser
//!   percent-encodes (a control character inside a path or a query, an unassigned code point, a
//!   default-ignorable one), drops (in a host, a code point that UTS 46 ignores, as it ignores
//!   every default-ignorable code point but the bidirectional controls, U+200C, U+200D, and the
//!   tag characters, so the soft hyphen, U+200B, and the variation selectors go, and
//!   `https://a\u{180B}b.example/` is `https://ab.example/`; a tab, a carriage return, or a line
//!   feed anywhere; and any run of C0 control characters, U+0000 to U+001F, and spaces at the end
//!   of the key), or strips with the fragment. A key that the parser refuses (as it refuses a host
//!   that holds a control character, a bidirectional control, a tag character, U+2028, or an
//!   unassigned code point) is refused with the text of the first refused character in it, read as
//!   it is sent, and as an invalid URL when it holds none;
//! - `arxiv:`: whitespace after the scheme, a leading `arXiv:`, and the version suffix `vN` (or
//!   `VN`) stripped, an old-style archive name lower-cased (CS-36);
//! - `doi:`: lower-cased, and whitespace after the scheme and leading `doi:` labels and resolver
//!   prefixes stripped: `doi.org/` after an optional `http://` or `https://` and an optional `dx.`
//!   (CS-36);
//! - `lean:` and `run:`: a hexadecimal hash, lower-cased (CS-36);
//! - any other scheme: the value as it is.
//!
//! The message forms of `chat:`, `issue:`, and `mail:` are what a request's origin carries (A-61,
//! D.8.8). The normaliser checks their shape and does not tell a message from a conversation by
//! meaning, so the forms are accepted wherever a key is.

use url::{Url, form_urlencoded};

use crate::KeyError;
use crate::principal;
use crate::text::{
    check_chars, clean, fixed_point, is_digits, split_scheme, strip_angle_pair,
    strip_prefix_ignore_case,
};

/// The longest key, in bytes.
const MAX_BYTES: usize = 512;

/// Query parameters that carry credentials and are dropped from URL keys (CS-35; `Signature` is
/// CloudFront's and Google Cloud Storage's older form). A piece of a query ([`rebuild_query`]) is a
/// credential when its name, read as PHP reads it ([`php_name`]), equals one of these in any ASCII
/// case, or starts with one of [`CREDENTIAL_PREFIXES`].
///
/// PHP decodes a name (`%XX` and `+`), strips the spaces in front of it, ends it at its first NUL,
/// and reads a space, a `.`, or an unclosed `[` in it as `_`, so `api.key`, `api[key`, `api+key`,
/// `api%20key`, `+api_key`, `%20api_key`, `api_key%00`, `api_key%00x`, `API.Key`, `api%2Ekey`,
/// `api%5Bkey`, `client.secret`, and `access[token` are matched. No other spelling is: a name that
/// merely contains one of them, or spells it otherwise, is kept, such as `keys`, `secrets`,
/// `api-key`, `api..key`, `.token`, `api_key+`, a name behind another whitespace character
/// (`%09api_key`), and non-ASCII look-alikes; and so are PHP's arrays of these names, `api_key[]`
/// and `api.key[]` (the array `api_key`), `password[]`, and `token[x]`, and `api[key]` (the array
/// `api`). A name that starts with one of [`CREDENTIAL_PREFIXES`] is a credential whatever follows
/// the prefix, so an array of one is dropped: `X-Amz-Date[]`, `x-goog-meta[k]`.
const CREDENTIAL_PARAMS: [&str; 11] = [
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

/// The prefixes of signed-URL parameters (`X-Amz-Signature`, `X-Amz-Credential`, …, and Google
/// Cloud Storage's `X-Goog-Signature`, `X-Goog-Credential`, …), dropped from URL keys in any case
/// (CS-35).
const CREDENTIAL_PREFIXES: [&str; 2] = ["x-amz-", "x-goog-"];

/// What follows the `&` of an HTML-escaped `&amp;`, copied from a page into a URL.
const ENTITY_REST: &str = "amp;";

/// The refusal of a `person:` value that is not a principal key at all.
const PERSON_SHAPE: &str = "person: expected person:user or person:<scheme>:<value>";

/// Normalises `raw` with the version 1 rules. The length limit applies to the normal form, after
/// every round, so a key that a later round shortens is measured as it is stored.
pub(crate) fn normalise(raw: &str) -> Result<String, KeyError> {
    let key = fixed_point(raw, step)?;
    if key.len() > MAX_BYTES {
        return Err(KeyError::TooLong);
    }
    Ok(key)
}

/// One application of the version 1 rules, without the length limit.
fn step(raw: &str) -> Result<String, KeyError> {
    let cleaned = clean(raw);
    if cleaned.is_empty() {
        return Err(KeyError::Empty);
    }
    // The character rules read an http or https key in its normal form, below (or as it is sent
    // when the URL parser refuses it), and every other key as it is sent, before its scheme is
    // looked at.
    let split = split_scheme(&cleaned);
    let is_url = matches!(&split, Ok((scheme, _)) if scheme == "http" || scheme == "https");
    if !is_url {
        check_chars(&cleaned)?;
    }
    let (scheme, value) = split?;
    if scheme == "repeat" {
        return Err(KeyError::BadScheme);
    }
    if value.is_empty() {
        return Err(KeyError::BadValue("the value after the scheme is empty"));
    }
    Ok(match scheme.as_str() {
        "http" | "https" => {
            let key = url_key(&cleaned)?;
            check_chars(&key)?;
            key
        }
        _ => {
            let value = match scheme.as_str() {
                "chat" => chat(value)?,
                "issue" => issue(value)?,
                "mail" => mail(value)?,
                "user" => user(value)?,
                "person" => person(value)?,
                "arxiv" => arxiv(value)?,
                "doi" => doi(value)?,
                "lean" | "run" => hex_hash(value)?,
                _ => value.to_owned(),
            };
            format!("{scheme}:{value}")
        }
    })
}

/// `<channel>/<thread>` or `<channel>/<thread>/<message>`, with the channel lower-cased.
fn chat(value: &str) -> Result<String, KeyError> {
    const SHAPE: &str =
        "chat: expected chat:<channel>/<thread> or chat:<channel>/<thread>/<message>";
    let (channel, rest) = value.split_once('/').ok_or(KeyError::BadValue(SHAPE))?;
    let parts: Vec<&str> = rest.split('/').collect();
    if channel.is_empty() || parts.len() > 2 || parts.iter().any(|part| part.is_empty()) {
        return Err(KeyError::BadValue(SHAPE));
    }
    Ok(format!("{}/{rest}", channel.to_lowercase()))
}

/// `<project>#<n>` or `<project>#<n>/<comment>`, with the project lower-cased and `n` without
/// leading zeros. The project ends at the first `#`, and the number at the first `/` after it, so a
/// project may contain `/` (`org/repo#12`).
fn issue(value: &str) -> Result<String, KeyError> {
    const SHAPE: &str =
        "issue: expected issue:<project>#<number> or issue:<project>#<number>/<comment>";
    let (project, rest) = value.split_once('#').ok_or(KeyError::BadValue(SHAPE))?;
    let (number, comment) = match rest.split_once('/') {
        Some((number, comment)) => (number, Some(comment)),
        None => (rest, None),
    };
    if project.is_empty() || !is_digits(number) || comment.is_some_and(str::is_empty) {
        return Err(KeyError::BadValue(SHAPE));
    }
    let number = match number.trim_start_matches('0') {
        "" => "0",
        stripped => stripped,
    };
    let mut key = format!("{}#{number}", project.to_lowercase());
    if let Some(comment) = comment {
        key.push('/');
        key.push_str(comment);
    }
    Ok(key)
}

/// A message id: one pair of angle brackets stripped, the domain part lower-cased.
fn mail(value: &str) -> Result<String, KeyError> {
    let id = strip_angle_pair(value)?;
    Ok(match id.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => id.to_owned(),
    })
}

/// `<n>` or `e<n>`, rendered `e<n>` with `n` a `u64` without leading zeros, as the core renders a
/// user entry's key (`KeyStr::of_user_entry`, D.2.4).
fn user(value: &str) -> Result<String, KeyError> {
    let digits = value.strip_prefix('e').unwrap_or(value);
    if !is_digits(digits) {
        return Err(KeyError::BadValue(
            "user: expected user:e<entry> or user:<entry>",
        ));
    }
    let entry: u64 = digits
        .parse()
        .map_err(|_| KeyError::BadValue("user: the entry number does not fit in 64 bits"))?;
    Ok(format!("e{entry}"))
}

/// A principal key (D-3). A value that is not shaped like one at all is refused with a hint about
/// `person:` rather than the principal normaliser's `NoScheme`, which would read as if the key
/// itself had no scheme.
fn person(value: &str) -> Result<String, KeyError> {
    principal::normalise(value).map_err(|e| match e {
        KeyError::Empty | KeyError::NoScheme => KeyError::BadValue(PERSON_SHAPE),
        other => other,
    })
}

/// A URL key: WHATWG parsing (scheme and host lower-cased, default port removed), no fragment, no
/// user name or password, and a query with every `&amp;` read as `&` and without empty pairs and
/// credential parameters; an empty query loses its `?`. `key` is the whole cleaned key, scheme
/// included.
fn url_key(key: &str) -> Result<String, KeyError> {
    let Ok(mut url) = Url::parse(key) else {
        // The parser refuses a host that holds a control character, U+2028 or U+2029, a
        // default-ignorable code point that it does not drop, or an unassigned code point: the
        // refusal names that character rather than the URL.
        check_chars(key)?;
        return Err(KeyError::BadValue("http and https keys must be valid URLs"));
    };
    url.set_fragment(None);
    // Credentials in the user information are not part of the object's identity either (CS-35).
    // An http or https URL always has a host, so neither call can fail.
    let _ = url.set_password(None);
    let _ = url.set_username("");
    if let Some(query) = url.query() {
        let rebuilt = rebuild_query(query);
        // A query that the rules leave as it is is not set again; one that is empty, or left
        // empty, loses its `?`.
        if rebuilt != query || rebuilt.is_empty() {
            url.set_query((!rebuilt.is_empty()).then_some(rebuilt.as_str()));
        }
    }
    Ok(url.into())
}

/// `query` with every `&amp;` read as `&`, and without its empty pairs and its credentials.
///
/// First, every HTML-escaped `&amp;`, which a URL copied from a page holds where the URL itself
/// holds `&`, is read as `&`, however often it was escaped: a `&` and every `amp;` that follows it
/// in a row are one `&`, so `&amp;amp;` is `&` as well. That is the fixed point of reading `&amp;`
/// as `&` once, reached in one pass and in linear time, so nesting never costs a round of the
/// normaliser, and a normal form's query holds no `&amp;` (its path keeps one, which is not read:
/// `https://x.example/a&amp;b` and `https://x.example/a&b` are two keys). Only the entity `&amp;`
/// is read: `&AMP;`, `&amp` without its `;`, and `%26` are not, and nor is any other HTML entity.
/// `&quot;` is data, and `&#038;` (as WordPress's `esc_url` writes `&`), `&#38;`, and `&#x26;` end
/// the query at their `#`, which starts a fragment, and [`url_key`] strips the fragment, so
/// `?p=1&#038;page=2` and `?p=1&#038;page=3` are both `?p=1`.
///
/// Then the query splits into pairs at every `&`, as every server splits it, and an empty pair is
/// dropped. A pair splits into pieces at every `;`, which some servers read as `&` too. `%26` and
/// `%3B` are data, not separators.
///
/// A pair is cut in front of its first credential piece ([`CREDENTIAL_PARAMS`]): that piece and
/// every piece after it in the pair are dropped, with the `;` in front of it, and a pair whose
/// first piece is a credential is dropped whole. Nothing of a credential's value is therefore kept
/// however a server splits the query: `password=a;b` is one parameter to a server that splits at
/// `&` alone, and `;b` the rest of its value. A pair that the cut leaves empty is dropped too.
///
/// Everything else is kept as it is: the pairs kept are joined by `&` again, the pieces kept keep
/// their `;`, and an empty piece is kept. A query that holds no `&amp;`, no empty pair, and no
/// credential is therefore left as it is.
fn rebuild_query(query: &str) -> String {
    let kept: Vec<&str> = query
        .split('&')
        .enumerate()
        .map(|(index, pair)| {
            // Splitting before decoding gives the same pairs, because reading `&amp;` as `&` keeps
            // every `&` and drops only the `amp;`s right after one: each pair after the first
            // loses the `amp;`s at its start, and the first pair, which no `&` precedes, keeps
            // them.
            if index > 0 {
                strip_entities(pair)
            } else {
                pair
            }
        })
        .filter_map(cut_at_credential)
        .filter(|pair| !pair.is_empty())
        .collect();
    kept.join("&")
}

/// `pair` without the `amp;` in front of it, as often as it is there: the rest of one `&amp;`, or
/// of several nested ones (`&amp;amp;`), after the `&` that precedes `pair`.
fn strip_entities(pair: &str) -> &str {
    let mut rest = pair;
    while let Some(after) = rest.strip_prefix(ENTITY_REST) {
        rest = after;
    }
    rest
}

/// `pair` cut in front of its first credential piece: the pieces before it, without the `;` that
/// follows them, or `None` when the first piece is a credential. A pair without a credential is
/// returned as it is.
fn cut_at_credential(pair: &str) -> Option<&str> {
    // `end` is where `pair` ends if the current piece is a credential: at the `;` in front of it,
    // or nowhere for the first piece.
    let mut end = None;
    let mut start = 0_usize;
    for piece in pair.split_inclusive(';') {
        let text = piece.strip_suffix(';').unwrap_or(piece);
        if is_credential(text) {
            return end.and_then(|end| pair.get(..end));
        }
        end = Some(start.saturating_add(text.len()));
        start = start.saturating_add(piece.len());
    }
    Some(pair)
}

/// Whether one `name=value` piece of a query is a credential: its name, read as PHP reads it
/// ([`php_name`]), is one of [`CREDENTIAL_PARAMS`] or starts with one of [`CREDENTIAL_PREFIXES`],
/// in any ASCII case.
fn is_credential(piece: &str) -> bool {
    form_urlencoded::parse(piece.as_bytes())
        .next()
        .is_some_and(|(name, _)| {
            let name = php_name(&name);
            CREDENTIAL_PREFIXES
                .iter()
                .any(|prefix| strip_prefix_ignore_case(&name, prefix).is_some())
                || CREDENTIAL_PARAMS
                    .iter()
                    .any(|param| name.eq_ignore_ascii_case(param))
        })
}

/// A decoded parameter name as PHP files it, as far as the credential names can tell: the spaces in
/// front of it stripped, the name ended at its first NUL, and every space, `.`, and `[` in what is
/// left read as `_`. Only U+0020 counts as a space, as in PHP; `+` and `%20` decode to it.
fn php_name(decoded: &str) -> String {
    let name = decoded.trim_start_matches(' ');
    let name = name.split_once('\0').map_or(name, |(head, _)| head);
    name.chars()
        .map(|c| if matches!(c, ' ' | '.' | '[') { '_' } else { c })
        .collect()
}

/// An arXiv identifier: no whitespace after the scheme, no leading `arXiv:`, no version suffix, the
/// old-style archive lower-cased (`hep-th/9901001`, `math.gt/0309136`).
fn arxiv(value: &str) -> Result<String, KeyError> {
    let mut id = value.trim();
    while let Some(rest) = strip_prefix_ignore_case(id, "arxiv:") {
        id = rest.trim();
    }
    let id = strip_versions(id);
    if id.is_empty() {
        return Err(KeyError::BadValue("arxiv: the identifier is empty"));
    }
    Ok(match id.split_once('/') {
        Some((archive, number)) => format!("{}/{number}", archive.to_lowercase()),
        None => id.to_owned(),
    })
}

/// `id` without its trailing version suffixes: each a `v` or `V` and digits, right after a digit.
fn strip_versions(id: &str) -> &str {
    let mut id = id;
    loop {
        let without_digits = id.trim_end_matches(|c: char| c.is_ascii_digit());
        if without_digits.len() == id.len() {
            return id;
        }
        match without_digits.strip_suffix(['v', 'V']) {
            Some(base) if base.ends_with(|c: char| c.is_ascii_digit()) => id = base,
            Some(_) | None => return id,
        }
    }
}

/// A DOI: lower-cased, without whitespace after the scheme, and without leading `doi:` labels and
/// resolver prefixes (`https://doi.org/`, `http://dx.doi.org/`, `doi.org/`, …).
fn doi(value: &str) -> Result<String, KeyError> {
    let lower = value.to_lowercase();
    let mut doi = lower.trim();
    while let Some(rest) = strip_doi_prefix(doi) {
        doi = rest.trim();
    }
    if doi.is_empty() {
        return Err(KeyError::BadValue("doi: the DOI is empty"));
    }
    Ok(doi.to_owned())
}

/// `doi` (lower-cased) without one leading `doi:` label or one resolver prefix: `doi.org/` after an
/// optional `http://` or `https://` and an optional `dx.`.
fn strip_doi_prefix(doi: &str) -> Option<&str> {
    if let Some(rest) = doi.strip_prefix("doi:") {
        return Some(rest);
    }
    let host = doi
        .strip_prefix("https://")
        .or_else(|| doi.strip_prefix("http://"))
        .unwrap_or(doi);
    host.strip_prefix("dx.")
        .unwrap_or(host)
        .strip_prefix("doi.org/")
}

/// A hash computed by the agent's own toolchain (`lean:`, `run:`): hexadecimal, lower-cased.
fn hex_hash(value: &str) -> Result<String, KeyError> {
    if value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(KeyError::BadValue("lean: and run: take a hexadecimal hash"))
    }
}
