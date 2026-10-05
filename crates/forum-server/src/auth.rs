//! Classes and authentication (design D.11.3, CS-27, FQ-5).
//!
//! Bearer tokens are configuration, never forum content: one variable per token,
//! `FORUM_TOKEN_<NAME>`, from the process environment and from a `.env` file (the environment wins,
//! name by name). The value holds the SHA-256 of the token, never the token, then `;`-separated
//! fields: `class=<class>` (required) and `caps=<label>,<label>…` (optional, the default caps of a
//! session that sends none, CS-26). Any other field, `principal=` among them, is malformed: a token
//! binds no principal (FQ-5). A malformed value stops the server at startup, and keeps the old map
//! at a reload.
//!
//! The HTTP layer ([`bearer`]) hashes the `Authorization: Bearer` value, looks it up, and puts the
//! [`Caller`] (class and default caps) into the request's extensions, where the MCP handler finds
//! it; an unknown or missing token gets HTTP 401 before MCP sees the request. A class and caps are
//! never used for permissions (§3).
//!
//! The module reads neither the environment nor a file: the configuration passes the variables
//! and the `.env` text in (D.11.2), and on SIGHUP the engine swaps the map at a job boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, RwLock};

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use forum_core::{ClassName, Label};
use sha2::{Digest, Sha256};

use crate::dotenv;

/// The prefix of a token variable.
pub const VAR_PREFIX: &str = "FORUM_TOKEN_";

/// The prefix of every token the server prints (`forum-server token new`), which the masker
/// recognises (D.11.13).
pub const TOKEN_PREFIX: &str = "fsk_";

/// What a token binds a call to: `Ctx.class` and `Ctx.caps` (D.5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// The token's class.
    pub class: ClassName,
    /// The token's default caps, if it has any.
    pub caps: Option<BTreeSet<Label>>,
}

/// Where a token's line came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// The process environment, read at startup; a reload never changes it.
    Environment,
    /// The `.env` file, re-read at a reload.
    Dotenv,
}

/// One configured token.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    source: Source,
    caller: Caller,
}

/// A configuration problem: it stops the server at startup and keeps the old map at a reload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The `.env` file has a malformed line.
    Dotenv(dotenv::DotenvError),
    /// A token variable's value is malformed.
    Value {
        /// The variable.
        name: String,
        /// What is wrong.
        why: String,
    },
    /// Two variables hold the same token.
    Duplicate {
        /// The first variable.
        first: String,
        /// The second.
        second: String,
    },
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::Dotenv(e) => e.fmt(f),
            AuthError::Value { name, why } => write!(f, "{name}: {why}"),
            AuthError::Duplicate { first, second } => {
                write!(f, "{first} and {second} hold the same token")
            }
        }
    }
}

impl std::error::Error for AuthError {}

/// The tokens the server accepts, by the SHA-256 of the token.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenMap {
    by_hash: BTreeMap<[u8; 32], Entry>,
}

impl TokenMap {
    /// Builds the map from the process environment's variables and the `.env` text, if there is a
    /// file. Only `FORUM_TOKEN_*` variables are read; a variable set in both places takes the
    /// environment's value. Every `.env` line must parse, token or not.
    pub fn load<I>(environment: I, dotenv_text: Option<&str>) -> Result<TokenMap, AuthError>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let env: BTreeMap<String, String> = environment
            .into_iter()
            .filter(|(k, _)| is_token_var(k))
            .collect();
        let mut lines: BTreeMap<String, (Source, String)> = BTreeMap::new();
        if let Some(text) = dotenv_text {
            for l in dotenv::parse(text).map_err(AuthError::Dotenv)? {
                if is_token_var(&l.key) {
                    lines.insert(l.key, (Source::Dotenv, l.value));
                }
            }
        }
        for (k, v) in env {
            lines.insert(k, (Source::Environment, v));
        }
        let mut map = TokenMap::default();
        for (name, (source, value)) in lines {
            let (hash, caller) = parse_value(&value).map_err(|why| AuthError::Value {
                name: name.clone(),
                why,
            })?;
            if let Some(other) = map.by_hash.get(&hash) {
                return Err(AuthError::Duplicate {
                    first: other.name.clone(),
                    second: name,
                });
            }
            map.by_hash.insert(
                hash,
                Entry {
                    name,
                    source,
                    caller,
                },
            );
        }
        Ok(map)
    }

    /// The map after a reload (SIGHUP): the environment's tokens stay, and win, and the `.env`
    /// tokens are those of the new text. On an error the caller keeps the old map.
    pub fn reload(&self, dotenv_text: Option<&str>) -> Result<TokenMap, AuthError> {
        let env = self
            .by_hash
            .iter()
            .filter(|(_, e)| e.source == Source::Environment)
            .map(|(h, e)| (e.name.clone(), unparse(h, &e.caller)));
        TokenMap::load(env, dotenv_text)
    }

    /// The caller a bearer token binds, if the token is configured.
    #[must_use]
    pub fn lookup(&self, token: &str) -> Option<Caller> {
        self.by_hash
            .get(&sha256(token.as_bytes()))
            .map(|e| e.caller.clone())
    }

    /// The configured variables, by name, with where each came from (for the server's own log).
    #[must_use]
    pub fn names(&self) -> BTreeMap<String, Source> {
        self.by_hash
            .values()
            .map(|e| (e.name.clone(), e.source))
            .collect()
    }

    /// How many tokens are configured.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    /// Whether no token is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

/// The map the HTTP layer reads, swapped whole at a reload.
#[derive(Clone, Debug, Default)]
pub struct SharedTokens(Arc<RwLock<Arc<TokenMap>>>);

impl SharedTokens {
    /// Shares `map`.
    #[must_use]
    pub fn new(map: TokenMap) -> SharedTokens {
        SharedTokens(Arc::new(RwLock::new(Arc::new(map))))
    }

    /// The map in force.
    #[must_use]
    pub fn current(&self) -> Arc<TokenMap> {
        match self.0.read() {
            Ok(g) => Arc::clone(&g),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    /// Replaces the map (the engine calls it at a job boundary, D.11.2).
    pub fn swap(&self, map: TokenMap) {
        let map = Arc::new(map);
        match self.0.write() {
            Ok(mut g) => *g = map,
            Err(poisoned) => *poisoned.into_inner() = map,
        }
    }
}

/// The axum middleware of the `/mcp` route: `Authorization: Bearer <token>` → the [`Caller`] in
/// the request's extensions, or HTTP 401.
pub async fn bearer(State(tokens): State<SharedTokens>, mut req: Request, next: Next) -> Response {
    let caller = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer_token)
        .and_then(|t| tokens.current().lookup(t));
    match caller {
        Some(c) => {
            req.extensions_mut().insert(c);
            next.run(req).await
        }
        None => {
            let mut resp = (
                StatusCode::UNAUTHORIZED,
                "unknown or missing bearer token\n",
            )
                .into_response();
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            resp
        }
    }
}

/// The token of an `Authorization` value `Bearer <token>` (the scheme in any case).
fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

fn is_token_var(name: &str) -> bool {
    name.len() > VAR_PREFIX.len() && name.starts_with(VAR_PREFIX)
}

/// Parses `<sha256 hex>;class=<class>[;caps=<label>,…]`.
fn parse_value(value: &str) -> Result<([u8; 32], Caller), String> {
    let mut parts = value.split(';').map(str::trim);
    let hash = parts.next().unwrap_or_default();
    let hash = parse_hex32(hash).ok_or_else(|| {
        "expected the SHA-256 of the token as 64 hexadecimal digits, then ;class=<class>"
            .to_string()
    })?;
    let mut class: Option<ClassName> = None;
    let mut caps: Option<BTreeSet<Label>> = None;
    for part in parts {
        let Some((k, v)) = part.split_once('=') else {
            return Err(format!("expected <field>=<value>, found {part:?}"));
        };
        match k.trim() {
            "class" => {
                if class.is_some() {
                    return Err("class is given twice".to_string());
                }
                class = Some(
                    ClassName::new(v.trim())
                        .ok_or_else(|| format!("class {v:?} is not [a-z0-9][a-z0-9-]{{0,31}}"))?,
                );
            }
            "caps" => {
                if caps.is_some() {
                    return Err("caps is given twice".to_string());
                }
                let mut set = BTreeSet::new();
                for c in v.split(',') {
                    set.insert(Label::new(c.trim()).ok_or_else(|| {
                        format!("cap {c:?} is not a label [a-z0-9][a-z0-9_.:-]{{0,63}}")
                    })?);
                }
                caps = Some(set);
            }
            "principal" => {
                return Err(
                    "a token binds a class and caps, never a principal: a session names its \
                     principal itself (hello {principal})"
                        .to_string(),
                );
            }
            other => return Err(format!("unknown field {other:?}: only class and caps")),
        }
    }
    let class = class.ok_or_else(|| "class=<class> is required".to_string())?;
    Ok((hash, Caller { class, caps }))
}

/// The value that `parse_value` reads back as `(hash, caller)`.
fn unparse(hash: &[u8; 32], caller: &Caller) -> String {
    let mut s = hex(hash);
    s.push_str(";class=");
    s.push_str(caller.class.as_str());
    if let Some(caps) = &caller.caps {
        s.push_str(";caps=");
        s.push_str(&caps.iter().map(Label::as_str).collect::<Vec<_>>().join(","));
    }
    s
}

/// SHA-256.
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Lower-case hexadecimal.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, pair) in b.chunks_exact(2).enumerate() {
        let hi = char::from(pair[0]).to_digit(16)?;
        let lo = char::from(pair[1]).to_digit(16)?;
        out[i] = u8::try_from(hi * 16 + lo).ok()?;
    }
    Some(out)
}

/// A fresh token from 32 random bytes: `fsk_` and 43 base64url characters (D.11.3). The caller
/// supplies the randomness.
#[must_use]
pub fn new_token(random: &[u8; 32]) -> String {
    let mut s = String::from(TOKEN_PREFIX);
    s.push_str(&base64url(random));
    s
}

/// The `.env` line that configures `token` for `class` and `caps`, under `FORUM_TOKEN_<name>`.
#[must_use]
pub fn token_line(
    name: &str,
    token: &str,
    class: &ClassName,
    caps: Option<&BTreeSet<Label>>,
) -> String {
    let caller = Caller {
        class: class.clone(),
        caps: caps.cloned(),
    };
    format!(
        "{VAR_PREFIX}{name}={}",
        unparse(&sha256(token.as_bytes()), &caller)
    )
}

/// Base64url without padding.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            let idx = (n >> (18 - 6 * i)) & 0x3f;
            s.push(char::from(ALPHABET[idx as usize]));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::server::test_http;
    use axum::Router;
    use axum::routing::get;

    fn caps(list: &[&str]) -> BTreeSet<Label> {
        list.iter().map(|c| Label::new(c).unwrap()).collect()
    }

    fn line(token: &str, rest: &str) -> String {
        format!("{};{rest}", hex(&sha256(token.as_bytes())))
    }

    fn env(pairs: &[(&str, String)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn tokens_from_the_environment_and_dotenv_the_environment_winning() {
        let dot = format!(
            "# tokens\nFORUM_TOKEN_CI={}\nFORUM_TOKEN_IDE={}\nOTHER=x\n",
            line("ci-from-dotenv", "class=coding-agent"),
            line("ide", "class=ide-agent"),
        );
        let map = TokenMap::load(
            env(&[
                (
                    "FORUM_TOKEN_CI",
                    line("ci-from-env", "class=coding-agent;caps=rust"),
                ),
                ("PATH", "/bin".to_string()),
                ("FORUM_TOKEN_", line("nameless", "class=x")),
            ]),
            Some(&dot),
        )
        .unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.lookup("ci-from-env"),
            Some(Caller {
                class: ClassName::new("coding-agent").unwrap(),
                caps: Some(caps(&["rust"])),
            })
        );
        assert_eq!(map.lookup("ci-from-dotenv"), None, "the environment wins");
        assert_eq!(
            map.lookup("ide").map(|c| c.class),
            ClassName::new("ide-agent")
        );
        assert_eq!(map.lookup("nameless"), None);
        assert_eq!(map.lookup(""), None);
        assert_eq!(
            map.names(),
            BTreeMap::from([
                ("FORUM_TOKEN_CI".to_string(), Source::Environment),
                ("FORUM_TOKEN_IDE".to_string(), Source::Dotenv),
            ])
        );
    }

    #[test]
    fn a_tokens_caps_are_labels_and_reach_the_caller() {
        let map = TokenMap::load(
            env(&[(
                "FORUM_TOKEN_PAYMENTS",
                line("p", "class=coding-agent;caps=team:payments,rust"),
            )]),
            None,
        )
        .unwrap();
        assert_eq!(
            map.lookup("p").and_then(|c| c.caps),
            Some(caps(&["team:payments", "rust"]))
        );
    }

    #[test]
    fn malformed_values_stop_the_server() {
        for (value, needle) in [
            ("9f2c;class=coding-agent".to_string(), "64 hexadecimal"),
            (line("t", ""), "expected <field>=<value>"),
            (line("t", "caps=rust"), "class=<class> is required"),
            (line("t", "class=Coding"), "is not [a-z0-9]"),
            (line("t", "class=a;class=b"), "twice"),
            (line("t", "class=a;caps="), "is not a label"),
            (line("t", "class=a;caps=ok,Not Ok"), "is not a label"),
            (
                line("t", "class=a;principal=mail:bob@x"),
                "never a principal",
            ),
            (line("t", "class=a;role=admin"), "unknown field"),
        ] {
            let e = TokenMap::load(env(&[("FORUM_TOKEN_X", value.clone())]), None).unwrap_err();
            assert!(e.to_string().contains(needle), "{value:?}: {e}");
            assert!(e.to_string().starts_with("FORUM_TOKEN_X: "), "{e}");
        }
        let e = TokenMap::load(Vec::new(), Some("FORUM_TOKEN_X=1\nbroken\n")).unwrap_err();
        assert_eq!(e.to_string(), ".env line 2: expected KEY=VALUE");
        let e = TokenMap::load(
            env(&[
                ("FORUM_TOKEN_A", line("same", "class=a")),
                ("FORUM_TOKEN_B", line("same", "class=b")),
            ]),
            None,
        )
        .unwrap_err();
        assert_eq!(
            e,
            AuthError::Duplicate {
                first: "FORUM_TOKEN_A".to_string(),
                second: "FORUM_TOKEN_B".to_string()
            }
        );
    }

    #[test]
    fn upper_case_hashes_are_accepted() {
        let v = line("t", "class=a")
            .to_uppercase()
            .replace("CLASS=A", "class=a");
        let map = TokenMap::load(env(&[("FORUM_TOKEN_X", v)]), None).unwrap();
        assert!(map.lookup("t").is_some());
    }

    #[test]
    fn a_reload_revokes_a_dotenv_token_and_keeps_the_environments() {
        let map = TokenMap::load(
            env(&[("FORUM_TOKEN_ENV", line("from-env", "class=a;caps=x,y"))]),
            Some(&format!(
                "FORUM_TOKEN_OLD={}\nFORUM_TOKEN_ENV={}\n",
                line("old", "class=b"),
                line("shadowed", "class=c")
            )),
        )
        .unwrap();
        assert!(map.lookup("old").is_some());
        let reloaded = map
            .reload(Some(&format!(
                "FORUM_TOKEN_NEW={}\nFORUM_TOKEN_ENV={}\n",
                line("new", "class=d"),
                line("shadowed", "class=c")
            )))
            .unwrap();
        assert_eq!(reloaded.lookup("old"), None, "revoked");
        assert!(reloaded.lookup("new").is_some());
        assert_eq!(reloaded.lookup("from-env"), map.lookup("from-env"));
        assert_eq!(reloaded.lookup("shadowed"), None);
        // A broken file keeps the old map: the caller swaps only on success.
        assert!(map.reload(Some("broken\n")).is_err());
        let shared = SharedTokens::new(map.clone());
        shared.swap(reloaded.clone());
        assert_eq!(*shared.current(), reloaded);
    }

    #[test]
    fn new_tokens_have_the_maskers_shape_and_their_line_parses() {
        let t = new_token(&[0xfb; 32]);
        assert_eq!(t.len(), 4 + 43);
        assert!(t.starts_with("fsk_"));
        assert!(
            t[4..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(base64url(b"\xfb\xff"), "-_8");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url(b"fooba"), "Zm9vYmE");
        let class = ClassName::new("coding-agent").unwrap();
        let l = token_line("CI", &t, &class, Some(&caps(&["rust"])));
        let (k, v) = l.split_once('=').unwrap();
        assert_eq!(k, "FORUM_TOKEN_CI");
        let map = TokenMap::load(env(&[(k, v.to_string())]), None).unwrap();
        assert_eq!(
            map.lookup(&t),
            Some(Caller {
                class,
                caps: Some(caps(&["rust"]))
            })
        );
    }

    #[test]
    fn bearer_values() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  abc "), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer"), None);
        assert_eq!(bearer_token("Bearer "), None);
    }

    async fn status_and_class(app: &Router, auth: Option<&str>) -> (u16, String) {
        let addr = test_http::serve(app.clone()).await;
        let headers: Vec<(&str, &str)> = auth.map(|a| ("authorization", a)).into_iter().collect();
        let r = test_http::request(addr, "GET", "/", &headers, b"").await;
        (r.status, r.body)
    }

    #[tokio::test]
    async fn the_layer_puts_the_caller_in_the_extensions_or_answers_401() {
        let map = TokenMap::load(
            env(&[(
                "FORUM_TOKEN_CI",
                line("good", "class=coding-agent;caps=rust"),
            )]),
            None,
        )
        .unwrap();
        let tokens = SharedTokens::new(map);
        let app = Router::new()
            .route(
                "/",
                get(|axum::Extension(c): axum::Extension<Caller>| async move {
                    format!("{} {:?}", c.class, c.caps.map(|s| s.len()))
                }),
            )
            .layer(axum::middleware::from_fn_with_state(tokens.clone(), bearer));
        assert_eq!(
            status_and_class(&app, Some("Bearer good")).await,
            (200, "coding-agent Some(1)".to_string())
        );
        assert_eq!(status_and_class(&app, Some("Bearer bad")).await.0, 401);
        assert_eq!(status_and_class(&app, None).await.0, 401);
        // A reload that revokes the token takes effect at the next request.
        tokens.swap(TokenMap::default());
        assert_eq!(status_and_class(&app, Some("Bearer good")).await.0, 401);
    }
}
