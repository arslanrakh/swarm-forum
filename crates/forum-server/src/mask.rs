//! Secret masking (design D.11.13, CS-35, review FS-15).
//!
//! Applied to every free-text argument of a call (never to keys, paths, or ids) before the command
//! is built and before the request digest (D.11.5), so the history never holds the secret (§24).
//! Each secret is replaced by `[masked:<kind>]`. Masking cannot be undone, so the generic rule for
//! assignments masks only values that look like secrets.
//!
//! Version 1, in the order applied:
//! 1. PEM private key blocks, from `-----BEGIN … PRIVATE KEY-----` to its `-----END … PRIVATE
//!    KEY-----`, or to the end of the text when the block was cut short;
//! 2. the password of a URL with user information (`scheme://user:password@`), up to the last `@`
//!    of the authority, so that a password with a raw `@` is masked whole;
//! 3. the token of an `Authorization: Bearer <token>` header, also written as JSON or a dict
//!    (`"Authorization": "Bearer <token>"`): the token's own characters, never a quote after it;
//! 4. the value after `aws_secret_access_key`;
//! 5. AWS access key ids (`AKIA…`, `ASIA…`);
//! 6. GitHub tokens (`gh[pousr]_…`, `github_pat_…`);
//! 7. Slack tokens (`xox[abprs]-…`);
//! 8. Stripe live keys (`sk_live_…`, `rk_live_…`);
//! 9. `sk-…` keys, `sk-ant-…` among them;
//! 10. Google API keys (`AIza…`);
//! 11. JWTs (`eyJ….eyJ….…`);
//! 12. the forum's own bearer tokens (`fsk_` and 43 base64url characters, D.11.3);
//! 13. assignments to `password`, `passwd`, `secret`, `token`, or `api_key` (any case; also as a
//!     segment of a compound name joined by `_` or `-`, such as `DB_PASSWORD`, `client_secret`, or
//!     `X-Api-Key`; optionally quoted, then `=`, `:`, or `:=`), but only when the value looks like
//!     a secret. The value is the longest run of `[A-Za-z0-9+/=_.-]` after an optional opening
//!     quote, so the delimiter after it (a quote, `,`, `;`, `}`, `)`, `&`, …) is never part of
//!     it. It looks like a secret when it has at least 16 characters, is not a call (followed by
//!     `(`), has at least 3.5 bits of entropy per character, is not a path (starting with `/`,
//!     `./`, or `../`), and is not a variable reference: a lower-case dotted identifier,
//!     `[a-z_][a-z0-9_.]*`, that holds a `.` or a `_`, or no digit (so a lower-case hex or base36
//!     key is not one).
//!
//! The filter of rule 13 keeps the review's eight ordinary code lines (`let token =
//! lexer.next_token();`, `secret = os.environ['APP_SECRET']`, …) and still masks its four secrets
//! (the tests below). Masking is idempotent, and deterministic: the entropy is computed in integer
//! fixed point, so the same text masks the same way on every machine, which the request digest
//! needs (D.11.5). Every released version stays callable ([`mask_v`]), because a retry is digested
//! with the masking version its `op_id` record stores.

use std::sync::LazyLock;

use regex::{Captures, Regex};

/// The current version of the pattern set, stored in `meta` and in each `st_ops` row.
pub const MASKING_VERSION: u32 = 1;

/// The replacement of a secret of `kind`.
fn marker(kind: &str) -> String {
    format!("[masked:{kind}]")
}

/// Masks `text` with the current version.
#[must_use]
pub fn mask(text: &str) -> String {
    v1(text)
}

/// Masks `text` with version `version` of the pattern set; None for a version that does not exist.
#[must_use]
pub fn mask_v(version: u32, text: &str) -> Option<String> {
    match version {
        1 => Some(v1(text)),
        _ => None,
    }
}

/// Whether `version` is a released version.
#[must_use]
pub fn is_version(version: u32) -> bool {
    version == 1
}

/// A rule that replaces each whole match by a marker, or the given group by it.
struct Rule {
    re: Regex,
    kind: &'static str,
    /// The group that holds the secret: 0 for the whole match.
    group: usize,
}

fn rule(pattern: &str, kind: &'static str, group: usize) -> Rule {
    match Regex::new(pattern) {
        Ok(re) => Rule { re, kind, group },
        // The patterns are constants, which the tests compile.
        Err(e) => panic!("masking pattern {pattern:?}: {e}"),
    }
}

static RULES_V1: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
            "private_key",
            0,
        ),
        // Greedy, so that the password runs to the last `@` before the path, the query, the
        // fragment, or a space, as WHATWG takes the last `@` of the authority.
        rule(
            r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/?#@:]*:([^\s/?#]+)@",
            "url_password",
            1,
        ),
        // The token is RFC 6750's b64token, so that a closing quote after it stays.
        rule(
            r#"(?i)\bauthorization["']?\s*[:=]\s*["']?bearer\s+([A-Za-z0-9._~+/=-]+)"#,
            "bearer_token",
            1,
        ),
        rule(
            r#"(?i)\baws_secret_access_key["']?\s*[:=]?\s*["']?([A-Za-z0-9/+=]{16,})"#,
            "aws_secret_access_key",
            1,
        ),
        rule(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b", "aws_access_key_id", 0),
        rule(
            r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})",
            "github_token",
            0,
        ),
        rule(r"\bxox[abprs]-[A-Za-z0-9-]{10,}", "slack_token", 0),
        rule(r"\b[sr]k_live_[A-Za-z0-9]{16,}", "stripe_key", 0),
        rule(r"\bsk-[A-Za-z0-9_-]{20,}", "secret_key", 0),
        rule(r"\bAIza[0-9A-Za-z_-]{35}", "google_api_key", 0),
        rule(
            r"\beyJ[A-Za-z0-9_-]{2,}\.eyJ[A-Za-z0-9_-]{2,}\.[A-Za-z0-9_-]*",
            "jwt",
            0,
        ),
        rule(r"\bfsk_[A-Za-z0-9_-]{43}", "forum_token", 0),
    ]
});

/// Rule 13: group 1 is the name's keyword, group 2 the value, and group 3 a `(` right after the
/// value, which makes it a call.
static ASSIGNMENT_V1: LazyLock<Regex> = LazyLock::new(|| {
    match Regex::new(concat!(
        r"(?i)\b(?:[a-z0-9]+[_-])*(password|passwd|secret|token|api[_-]?key)(?:[_-][a-z0-9]+)*",
        r#"["']?\s*[:=]=?\s*["']?([A-Za-z0-9+/=_.-]+)(\(?)"#,
    )) {
        Ok(re) => re,
        Err(e) => panic!("masking pattern: {e}"),
    }
});

/// The most passes of a version's rules: masking repeats them until the text no longer changes,
/// because a replacement can open a match for an earlier rule (a `fsk_` token that ran into
/// `https://…` leaves a URL behind its marker), and a fixed point is what makes masking
/// idempotent. Every pass replaces unmasked text by markers, which no rule changes, so a few
/// passes reach it.
const MAX_PASSES: usize = 16;

fn v1(text: &str) -> String {
    let mut out = v1_pass(text);
    for _ in 1..MAX_PASSES {
        let next = v1_pass(&out);
        if next == out {
            break;
        }
        out = next;
    }
    out
}

fn v1_pass(text: &str) -> String {
    let mut out = text.to_string();
    for r in RULES_V1.iter() {
        if !r.re.is_match(&out) {
            continue;
        }
        out =
            r.re.replace_all(&out, |c: &Captures<'_>| replace_group(c, r.group, r.kind))
                .into_owned();
    }
    if ASSIGNMENT_V1.is_match(&out) {
        out = ASSIGNMENT_V1
            .replace_all(&out, |c: &Captures<'_>| assignment(c))
            .into_owned();
    }
    out
}

/// The match with group `group` replaced by the marker of `kind`.
fn replace_group(c: &Captures<'_>, group: usize, kind: &str) -> String {
    let (Some(whole), Some(secret)) = (c.get(0), c.get(group)) else {
        return c
            .get(0)
            .map_or_else(String::new, |m| m.as_str().to_string());
    };
    let start = secret.start() - whole.start();
    let end = secret.end() - whole.start();
    let w = whole.as_str();
    format!("{}{}{}", &w[..start], marker(kind), &w[end..])
}

/// Rule 13: the match, with its value masked if the value looks like a secret.
fn assignment(c: &Captures<'_>) -> String {
    let (Some(whole), Some(name), Some(value)) = (c.get(0), c.get(1), c.get(2)) else {
        return c
            .get(0)
            .map_or_else(String::new, |m| m.as_str().to_string());
    };
    let w = whole.as_str();
    let call = c.get(3).is_some_and(|m| !m.as_str().is_empty());
    if call || !looks_like_secret(value.as_str()) {
        return w.to_string();
    }
    let start = value.start() - whole.start();
    let end = value.end() - whole.start();
    let keyword = name.as_str().to_ascii_lowercase();
    // `apikey` and `api-key` name what `api_key` does.
    let kind = if keyword.starts_with("api") {
        "api_key"
    } else {
        keyword.as_str()
    };
    format!("{}{}{}", &w[..start], marker(kind), &w[end..])
}

/// Bits of entropy per character required of an assigned value, in units of 2^-31 (3.5 × 2^31).
const ENTROPY_MIN_HALF_BITS: u128 = 7;

/// Rule 13's filter.
fn looks_like_secret(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() >= 16
        && b.iter().all(|&c| {
            c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'_' | b'.' | b'-')
        })
        && !(v.starts_with('/') || v.starts_with("./") || v.starts_with("../"))
        && !is_variable_reference(b)
        && entropy_at_least_3_5(b)
}

/// `[a-z_][a-z0-9_.]*` that looks like an identifier: it holds a `.` or a `_`, or no digit. A
/// lower-case hex or base36 secret that starts with a letter has digits and neither separator, so
/// it is not one.
fn is_variable_reference(b: &[u8]) -> bool {
    let shape = b
        .first()
        .is_some_and(|&c| c.is_ascii_lowercase() || c == b'_')
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'.');
    shape && (b.iter().any(|&c| c == b'.' || c == b'_') || !b.iter().any(u8::is_ascii_digit))
}

/// Whether the Shannon entropy of the byte distribution of `b` is at least 3.5 bits per byte:
/// `n·log2 n − Σ cᵢ·log2 cᵢ ≥ 3.5·n`, in fixed point with 32 fractional bits. Each floored term is
/// off by less than one unit, so the sum is off by less than 2n units, under 2^-31 bits per
/// character, and the comparison is exact integer arithmetic.
fn entropy_at_least_3_5(b: &[u8]) -> bool {
    if b.is_empty() {
        return false;
    }
    let mut counts = [0_u64; 256];
    for &c in b {
        counts[usize::from(c)] += 1;
    }
    let n = u64::try_from(b.len()).unwrap_or(u64::MAX);
    let total = u128::from(n) * log2_fixed(n);
    let parts: u128 = counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| u128::from(c) * log2_fixed(c))
        .sum();
    let lhs = total.saturating_sub(parts);
    lhs >= (ENTROPY_MIN_HALF_BITS * u128::from(n)) << 31
}

/// `floor(log2(x) · 2^32)` for `x ≥ 1`, by repeated squaring in Q62 fixed point (0 for 0).
fn log2_fixed(x: u64) -> u128 {
    if x == 0 {
        return 0;
    }
    let k = 63 - x.leading_zeros();
    // y = x / 2^k ∈ [1, 2) in Q62.
    let mut y: u128 = (u128::from(x) << 62) >> k;
    let two: u128 = 2 << 62;
    let mut frac: u128 = 0;
    for _ in 0..32 {
        y = (y * y) >> 62;
        frac <<= 1;
        if y >= two {
            y >>= 1;
            frac |= 1;
        }
    }
    (u128::from(k) << 32) | frac
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The review's eight ordinary code and prose lines (FS-15): the naive assignment rule masked
    /// all eight; the filter must leave every one unchanged.
    const REVIEW_CODE_LINES: [&str; 8] = [
        "let token = lexer.next_token();",
        "token: Option<Token>,",
        "if secret == None: raise ValueError('missing')",
        "password: the field is hashed with argon2 before storage",
        "the api_key = config.get('api_key') line reads it from the environment",
        "secret = os.environ['APP_SECRET']",
        "We store token = hash(session_id) in the cookie.",
        "token=abc (example from the docs)",
    ];

    /// The review's four secret assignments (FS-15), with fixed random values of the same lengths
    /// (20, 32, 40, and 24 alphanumeric characters): each must be masked.
    const REVIEW_SECRETS: [(&str, &str); 4] = [
        (
            "password = h4Kq9ZtWm2LxB7cRv0Ps",
            "password = [masked:password]",
        ),
        (
            "api_key: Q8vN2kLm5XrT9bWc3HyJ6gFd1SaZ4uPe",
            "api_key: [masked:api_key]",
        ),
        (
            "token=Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3Jd5Gf0YaUeIoTyNb",
            "token=[masked:token]",
        ),
        (
            "secret: 'mW3nQ8xL1vR6tB9kZ4cJ7hY2'",
            "secret: '[masked:secret]'",
        ),
    ];

    #[test]
    fn the_reviews_code_lines_pass_unchanged() {
        for line in REVIEW_CODE_LINES {
            assert_eq!(mask(line), line);
        }
    }

    #[test]
    fn the_reviews_secrets_are_masked() {
        for (secret, masked) in REVIEW_SECRETS {
            assert_eq!(mask(secret), masked);
        }
    }

    #[test]
    fn every_pattern() {
        let cases: [(&str, &str); 22] = [
            (
                "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAK\n-----END RSA PRIVATE KEY-----\nafter",
                "key:\n[masked:private_key]\nafter",
            ),
            (
                "-----BEGIN PRIVATE KEY-----\nMIIEvQ cut short",
                "[masked:private_key]",
            ),
            (
                "-----BEGIN OPENSSH PRIVATE KEY-----\nb3Blb\n-----END OPENSSH PRIVATE KEY-----",
                "[masked:private_key]",
            ),
            (
                "clone https://bob:hunter2@git.example/r.git now",
                "clone https://bob:[masked:url_password]@git.example/r.git now",
            ),
            (
                "postgres://:pw@db:5432/x",
                "postgres://:[masked:url_password]@db:5432/x",
            ),
            (
                "Authorization: Bearer abc.def-ghi",
                "Authorization: Bearer [masked:bearer_token]",
            ),
            (
                "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
                "aws_secret_access_key = [masked:aws_secret_access_key]",
            ),
            (
                "id AKIAIOSFODNN7EXAMPLE and ASIAY34FZKBOKMUTVV7A",
                "id [masked:aws_access_key_id] and [masked:aws_access_key_id]",
            ),
            (
                "ghp_0123456789abcdefghijABCDEFGHIJ012345",
                "[masked:github_token]",
            ),
            (
                "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz",
                "[masked:github_token]",
            ),
            (
                "slack xoxb-1234567890-abcdefABCDEF",
                "slack [masked:slack_token]",
            ),
            (
                "sk_live_4eC39HqLyjWDarjtT1zdp7dc and rk_live_51H8abcdefghijklmn",
                "[masked:stripe_key] and [masked:stripe_key]",
            ),
            (
                "OPENAI=sk-proj-abcdefghijklmnopqrstuvwx",
                "OPENAI=[masked:secret_key]",
            ),
            (
                "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz",
                "[masked:secret_key]",
            ),
            (
                "AIzaSyA-1234567890abcdefghijklmnopqrstu",
                "[masked:google_api_key]",
            ),
            (
                "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U end",
                "jwt [masked:jwt] end",
            ),
            (
                "fsk_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "[masked:forum_token]",
            ),
            (
                r#"{"token": "Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3", "user": "bob"}"#,
                r#"{"token": "[masked:token]", "user": "bob"}"#,
            ),
            ("PASSWD=Q8vN2kLm5XrT9bWc3HyJ;", "PASSWD=[masked:passwd];"),
            (
                "API_KEY='Q8vN2kLm5XrT9bWc3HyJ',",
                "API_KEY='[masked:api_key]',",
            ),
            ("Secret:h4Kq9ZtWm2LxB7cRv0Ps", "Secret:[masked:secret]"),
            (
                "two: token=Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3 password=h4Kq9ZtWm2LxB7cRv0Ps",
                "two: token=[masked:token] password=[masked:password]",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(mask(input), want, "{input:?}");
        }
    }

    #[test]
    fn negative_cases() {
        for text in [
            "",
            "plain prose with no secret at all",
            "https://git.example/r.git and mailto:bob@example.org",
            "ssh://git@host/repo",
            "Authorization: Basic",
            "AKIA too short AKIA123",
            "akiaiosfodnn7example",
            "gh_ token ghp_short",
            "risk-free-strategies-for-the-whole-quarter",
            "task-management",
            "eyJhbGciOiJIUzI1NiJ9 alone",
            "fsk_tooshort",
            // Rule 13's filter: short, a call, a path, a variable reference, or low entropy.
            "token = short",
            "token = config.secrets.github_token_value",
            "password = /etc/app/secrets/password.txt",
            "secret = ./relative/path/to/file.key",
            "token = ../relative/path/to/file.key",
            "token = aaaaaaaaaaaaaaaaaaaaaaaa",
            "token = abababababababababababab",
            "token = ABABABABABABABABABABABAB",
            "token = get_token_from_environment()",
            "tokens = Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3",
            "-----BEGIN PUBLIC KEY-----\nMIIB\n-----END PUBLIC KEY-----",
            // A compound name needs a `_` or `-` between its segments (review E2-spec-2), and a
            // call stays a call.
            "tokenizer: PreTrainedTokenizerFast",
            "max_tokens: 4096",
            "access_token = get_access_token_for_user()",
            "secret_ref = settings.vault.secret_path",
            // A port and an `@` in the query are not user information (review E2-bugs-11).
            "https://host:8080/path?email=a@b.com",
            "https://example.com:443 or bob@corp.example",
        ] {
            assert_eq!(mask(text), text, "{text:?}");
        }
    }

    /// The review's leaks (E2-spec-1, E2-spec-2, E2-spec-11, E2-bugs-3, E2-bugs-7, E2-bugs-11):
    /// each came back from `mask` unchanged, or lost a delimiter, before rule 13 took the value as
    /// a run of its alphabet, compound names, and the variable references' narrower shape, and
    /// before the bearer and URL rules were tightened.
    #[test]
    fn the_reviews_leaks_are_masked() {
        let cases: [(&str, &str); 21] = [
            // E2-spec-1: the delimiter after the value is not part of it.
            (
                r#"{"password": "h4Kq9ZtWm2LxB7cRv0Ps"}"#,
                r#"{"password": "[masked:password]"}"#,
            ),
            (
                r#"{"password":"h4Kq9ZtWm2LxB7cRv0Ps","user":"bob"}"#,
                r#"{"password":"[masked:password]","user":"bob"}"#,
            ),
            (
                r#"{"user":"bob","password":"h4Kq9ZtWm2LxB7cRv0Ps"}"#,
                r#"{"user":"bob","password":"[masked:password]"}"#,
            ),
            (
                "https://api.example/x?token=Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3&user=bob",
                "https://api.example/x?token=[masked:token]&user=bob",
            ),
            (
                "f(token=Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3)",
                "f(token=[masked:token])",
            ),
            (
                "db.connect(host='db', password='h4Kq9ZtWm2LxB7cRv0Ps')",
                "db.connect(host='db', password='[masked:password]')",
            ),
            (
                "api_key=Q8vN2kLm5XrT9bWc3HyJ6gFd1SaZ4uPe)",
                "api_key=[masked:api_key])",
            ),
            // E2-spec-2: compound names.
            (
                "DB_PASSWORD=h4Kq9ZtWm2LxB7cRv0Ps",
                "DB_PASSWORD=[masked:password]",
            ),
            (
                "GITHUB_TOKEN=Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3",
                "GITHUB_TOKEN=[masked:token]",
            ),
            (
                "client_secret: Q8vN2kLm5XrT9bWc3HyJ6gFd1SaZ4uPe",
                "client_secret: [masked:secret]",
            ),
            (
                "SECRET_KEY = 'Q8vN2kLm5XrT9bWc3HyJ6gFd1SaZ4uPe'",
                "SECRET_KEY = '[masked:secret]'",
            ),
            (
                "access_token=Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3",
                "access_token=[masked:token]",
            ),
            (
                r#"{"access_token":"ya29.a0AfH6SMBx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3","expires_in":3599}"#,
                r#"{"access_token":"[masked:token]","expires_in":3599}"#,
            ),
            (
                "X-Api-Key: Q8vN2kLm5XrT9bWc3HyJ6gFd1SaZ4uPe",
                "X-Api-Key: [masked:api_key]",
            ),
            // E2-bugs-3: lower-case hex and base36 are not variable references.
            (
                "token=e72e16c7e42f292c6912e7710c838347ae178b4a",
                "token=[masked:token]",
            ),
            (
                "token: k3j5h7g9f2d4s6a8q1w3e5r7t9y2u4i6",
                "token: [masked:token]",
            ),
            // E2-spec-11: a closing quote after a bearer token stays.
            (
                "curl -H 'Authorization: Bearer abc.def' https://x",
                "curl -H 'Authorization: Bearer [masked:bearer_token]' https://x",
            ),
            (
                r#"curl -H "Authorization: Bearer abc.def" https://x"#,
                r#"curl -H "Authorization: Bearer [masked:bearer_token]" https://x"#,
            ),
            // E2-bugs-7: the header as JSON or as a Python dict.
            (
                r#"headers = {"Authorization": "Bearer 7f9c2ba4e88f827d616045507605853e"}"#,
                r#"headers = {"Authorization": "Bearer [masked:bearer_token]"}"#,
            ),
            (
                "headers = {'Authorization': 'Bearer 7f9c2ba4e88f827d616045507605853e'}",
                "headers = {'Authorization': 'Bearer [masked:bearer_token]'}",
            ),
            // E2-bugs-11: a raw `@` in a password; WHATWG takes the authority's last `@`.
            (
                "https://bob:p@ss@host/",
                "https://bob:[masked:url_password]@host/",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(mask(input), want, "{input:?}");
            assert_eq!(mask(want), want, "idempotent: {want:?}");
        }
    }

    #[test]
    fn the_entropy_filter_is_exact_at_its_threshold() {
        // 16 characters: 8 distinct, twice each, is exactly 3 bits; 16 distinct is exactly 4.
        assert!(!entropy_at_least_3_5(b"ABCDEFGHABCDEFGH"));
        assert!(entropy_at_least_3_5(b"ABCDEFGHIJKLMNOP"));
        // 12 distinct of 16 (4 twice, 8 once): 3.5 bits exactly, which passes.
        assert!(entropy_at_least_3_5(b"AABBCCDDEFGHIJKL"));
        // One more repeat drops it below.
        assert!(!entropy_at_least_3_5(b"AABBCCDDEEFGHIJK"));
        assert_eq!(log2_fixed(1), 0);
        assert_eq!(log2_fixed(2), 1 << 32);
        assert_eq!(log2_fixed(1024), 10 << 32);
        // log2(3) = 1.584962500721156…
        let l3 = log2_fixed(3);
        assert!((l3 >> 32) == 1 && ((l3 & 0xffff_ffff) >> 20) == 0x95c);
    }

    #[test]
    fn versions() {
        assert_eq!(
            mask_v(1, "token=Zx7Lq2Wn9Bv4Kc6Mt1Rp8Hs3"),
            Some("token=[masked:token]".to_string())
        );
        assert_eq!(mask_v(0, "x"), None);
        assert_eq!(mask_v(2, "x"), None);
        assert!(is_version(MASKING_VERSION));
    }

    #[test]
    fn masking_is_idempotent_on_the_fixed_cases() {
        for (s, _) in REVIEW_SECRETS {
            let once = mask(s);
            assert_eq!(mask(&once), once);
        }
        let all = REVIEW_CODE_LINES.join("\n");
        assert_eq!(mask(&all), all);
        // Found by the property test: a forum token that runs into a URL leaves the URL behind its
        // marker, and a second pass masks the URL's password.
        let joined = format!("fsk_{}https://u:p@h/", "A".repeat(40));
        let once = mask(&joined);
        assert_eq!(once, "[masked:forum_token]ps://u:[masked:url_password]@h/");
        assert_eq!(mask(&once), once);
    }

    fn secretish() -> impl Strategy<Value = String> {
        prop_oneof![
            "[A-Za-z0-9+/=_.-]{0,40}",
            Just("-----BEGIN RSA PRIVATE KEY-----".to_string()),
            Just("-----END RSA PRIVATE KEY-----".to_string()),
            Just("https://u:p@h/".to_string()),
            Just("Authorization: Bearer ".to_string()),
            Just("AKIAIOSFODNN7EXAMPLE".to_string()),
            Just("ghp_0123456789abcdefghijABCDEFGHIJ012345".to_string()),
            Just("xoxb-1234567890-abc".to_string()),
            Just("sk-".to_string()),
            Just("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.".to_string()),
            Just("fsk_".to_string()),
            Just("token=".to_string()),
            Just("password: '".to_string()),
            Just("[masked:token]".to_string()),
            Just(" ".to_string()),
            Just("\n".to_string()),
            "\\PC{0,8}",
        ]
    }

    proptest! {
        // A failure is kept as a unit test above, not in a regressions file outside `src/`.
        #![proptest_config(ProptestConfig {
            cases: 10_000,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        /// D.11.13: masking is idempotent.
        #[test]
        fn masking_is_idempotent(parts in prop::collection::vec(secretish(), 0..12)) {
            let text: String = parts.concat();
            let once = mask(&text);
            prop_assert_eq!(mask(&once), once);
        }
    }
}
