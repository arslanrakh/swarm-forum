//! Configuration (design D.11.9, D.11.3): the server settings and the parameters from a TOML file,
//! and the bearer tokens from the process environment and a `.env` file. This module holds the
//! server's only reads of the environment and of configuration files (D.11.2); the rest of the
//! shell receives what it reads.
//!
//! ```toml
//! listen          = "127.0.0.1:8640"
//! database        = "forum.db"
//! search_database = "forum-search.db"            # the search index (D.11.12); by default
//!                                                # `<stem>-search.db` beside the database
//! allowed_hosts   = ["localhost", "127.0.0.1"]   # rmcp Host validation; ["*"]: none
//! instructions    = "default"                    # or a custom text ≤ instructions.max
//! [params]                                       # any field of Params; the rest default (§22)
//! lease_first     = "15m"                        # a duration: "15m", "2h", "50s", or milliseconds
//! hold_per_session = 2
//! conversation_schemes = ["chat", "issue", "mail"]
//! ```
//!
//! Parameters change only at startup (D.11.8 step 6); a configuration that breaks a rule of §22
//! stops the server. Bearer tokens are never in this file: `FORUM_TOKEN_<NAME>` variables, from the
//! environment and from `.env` in the working directory, the environment winning name by name
//! (D.11.3). On SIGHUP the shell re-reads the `.env` lines only, and a malformed file keeps the old
//! map.
//!
//! The process environment can give the settings too, for a container that is configured by its
//! variables (D.11.9): `FORUM_CONFIG` (the file), `FORUM_LISTEN`, `FORUM_DATABASE`,
//! `FORUM_SEARCH_DATABASE` (empty: no search), and `FORUM_ALLOWED_HOSTS` (host names separated by
//! commas or whitespace). A flag wins over its variable, a variable over the file, and the file
//! over the default ([`resolve`]). Every `allowed_hosts` entry, from the file or the environment,
//! is reduced to the host name that rmcp compares ([`allowed_host`]), and `*` turns rmcp's `Host`
//! validation off.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Path, PathBuf};

use axum::http::uri::Authority;
use forum_core::Params;
use serde_json::Value;

use crate::auth::{AuthError, TokenMap};
use crate::decode::values::dur_text;

/// The `.env` file the tokens are read from, in the working directory (D.11.3).
pub const DOTENV: &str = ".env";

/// The server settings (D.11.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Where the HTTP server listens.
    pub listen: String,
    /// The forum's database file.
    pub database: PathBuf,
    /// The search file (D.11.12); None: no search.
    pub search_database: Option<PathBuf>,
    /// rmcp's `Host` validation: host names as [`allowed_host`] gives them, never empty;
    /// [`ANY_HOST`] alone turns it off.
    pub allowed_hosts: Vec<String>,
    /// `default` (§22's text) or a custom text of at most `instructions.max` characters.
    pub instructions: String,
    /// The parameters (§22): the defaults with the file's `[params]` over them.
    pub params: Params,
    /// The usability test's arm (feature `usability`, WP-U only; design D.13.2 WP-E3).
    #[cfg(feature = "usability")]
    pub usability_arm: crate::render::usability::Arm,
    /// The usability lab's lagged clock (feature `usability`, test only): how far behind the
    /// system's clock the forum's starts, and the file a harness writes a catch-up to
    /// (`clock::LaggedClock`).
    #[cfg(feature = "usability")]
    pub usability_clock: Option<(forum_core::Dur, PathBuf)>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            listen: "127.0.0.1:8640".into(),
            database: PathBuf::from("forum.db"),
            search_database: Some(PathBuf::from("forum-search.db")),
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            instructions: "default".into(),
            params: Params::default(),
            #[cfg(feature = "usability")]
            usability_arm: crate::render::usability::Arm::default(),
            #[cfg(feature = "usability")]
            usability_clock: None,
        }
    }
}

/// The search file that belongs beside the database at `db`: `<stem>-search.db` in its directory
/// (`forum.db` → `forum-search.db`), so that two forums never share one (review E4-spec-6).
#[must_use]
pub fn search_path_for(db: &Path) -> PathBuf {
    let stem = db
        .file_stem()
        .map_or_else(|| "forum".into(), |s| s.to_string_lossy().into_owned());
    db.with_file_name(format!("{stem}-search.db"))
}

impl Config {
    /// Sets the database; the search file follows it unless it was named apart from it (it is
    /// still the one beside the old database).
    pub fn set_database(&mut self, db: PathBuf) {
        if self.search_database.as_deref() == Some(search_path_for(&self.database).as_path()) {
            self.search_database = Some(search_path_for(&db));
        }
        self.database = db;
    }
}

/// A configuration that stops the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A file that cannot be read.
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        why: String,
    },
    /// TOML that does not parse.
    Toml(String),
    /// A setting that is unknown or of the wrong type.
    Setting {
        /// The setting, such as `listen` or `params.lease_first`.
        key: String,
        /// What is wrong.
        why: String,
    },
    /// Parameters that break a rule of §22 (`Params::valid`).
    Params,
    /// The tokens.
    Tokens(AuthError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, why } => write!(f, "{}: {why}", path.display()),
            ConfigError::Toml(e) => write!(f, "the configuration is not TOML: {e}"),
            ConfigError::Setting { key, why } => write!(f, "{key}: {why}"),
            ConfigError::Params => write!(f, "the parameters break a rule of §22"),
            ConfigError::Tokens(e) => write!(f, "tokens: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

fn setting(key: &str, why: impl Into<String>) -> ConfigError {
    ConfigError::Setting {
        key: key.into(),
        why: why.into(),
    }
}

fn string(key: &str, v: &toml::Value) -> Result<String, ConfigError> {
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| setting(key, "expected a string"))
}

/// Parses a configuration file's text.
///
/// # Errors
///
/// TOML that does not parse, an unknown setting or parameter, a value of the wrong type, or
/// parameters that break a rule of §22.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| ConfigError::Toml(e.to_string()))?;
    let mut c = Config::default();
    // The database first, so that a search file named in the file wins over the one beside it.
    if let Some(v) = table.get("database") {
        c.set_database(PathBuf::from(string("database", v)?));
    }
    for (k, v) in &table {
        match k.as_str() {
            "listen" => c.listen = string(k, v)?,
            "database" => {}
            "search_database" => {
                let s = string(k, v)?;
                c.search_database = (!s.is_empty()).then(|| PathBuf::from(s));
            }
            "allowed_hosts" => {
                let entries = v
                    .as_array()
                    .ok_or_else(|| setting(k, "expected a list of host names"))?
                    .iter()
                    .map(|h| string(k, h))
                    .collect::<Result<Vec<_>, _>>()?;
                c.allowed_hosts = allowed_hosts(entries.iter().map(String::as_str))
                    .map_err(|why| setting(k, why))?
                    .ok_or_else(|| setting(k, EMPTY_HOSTS))?;
            }
            "instructions" => c.instructions = string(k, v)?,
            "params" => {
                c.params = params(
                    v.as_table()
                        .ok_or_else(|| setting(k, "expected a table of parameters"))?,
                )?;
            }
            #[cfg(feature = "usability")]
            "usability_arm" => {
                use crate::render::usability::Arm;
                c.usability_arm = match string(k, v)?.as_str() {
                    "with_guidance" => Arm::WithGuidance,
                    "without_guidance" => Arm::WithoutGuidance,
                    _ => return Err(setting(k, "with_guidance or without_guidance")),
                };
            }
            #[cfg(feature = "usability")]
            "usability_clock_lag" => {
                let lag = dur_text(&string(k, v)?)
                    .map_err(|e| setting(k, format!("not a duration: {e}")))?;
                let file = c
                    .usability_clock
                    .take()
                    .map_or_else(|| PathBuf::from("clock.jump"), |(_, f)| f);
                c.usability_clock = Some((lag, file));
            }
            #[cfg(feature = "usability")]
            "usability_clock_jump_file" => {
                let file = PathBuf::from(string(k, v)?);
                let lag = c
                    .usability_clock
                    .take()
                    .map_or(forum_core::Dur(0), |(l, _)| l);
                c.usability_clock = Some((lag, file));
            }
            other => return Err(setting(other, "unknown setting")),
        }
    }
    Ok(c)
}

/// The parameters of a `[params]` table: the defaults of §22 with the table's values over them.
/// A duration is a string such as `"15m"`, `"2h"`, or `"50s"`, or an integer of milliseconds.
///
/// # Errors
///
/// An unknown parameter, a value of the wrong type, or parameters that break a rule of §22.
pub fn params(t: &toml::Table) -> Result<Params, ConfigError> {
    let Ok(Value::Object(mut map)) = serde_json::to_value(Params::default()) else {
        return Err(ConfigError::Params);
    };
    for (k, v) in t {
        let key = format!("params.{k}");
        let Some(default) = map.get(k) else {
            return Err(setting(&key, "unknown parameter (§22)"));
        };
        let value = match (default, v) {
            (Value::Number(_), toml::Value::Integer(n)) => {
                Value::from(u64::try_from(*n).map_err(|_| setting(&key, "out of range"))?)
            }
            (Value::Number(_), toml::Value::String(s)) => Value::from(
                dur_text(s)
                    .map_err(|e| setting(&key, format!("not a duration: {e}")))?
                    .0,
            ),
            (Value::Array(_), toml::Value::Array(items)) => Value::Array(
                items
                    .iter()
                    .map(|x| string(&key, x).map(Value::from))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(setting(&key, "a value of the wrong type")),
        };
        map.insert(k.clone(), value);
    }
    let p: Params =
        serde_json::from_value(Value::Object(map)).map_err(|e| setting("params", e.to_string()))?;
    if p.valid() {
        Ok(p)
    } else {
        Err(ConfigError::Params)
    }
}

/// Reads and parses the configuration file at `path`, or the defaults without one.
///
/// # Errors
///
/// As [`parse`], and a file that cannot be read.
pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
    match path {
        None => Ok(Config::default()),
        Some(p) => parse(&read(p)?.ok_or_else(|| ConfigError::Read {
            path: p.to_path_buf(),
            why: "no such file".into(),
        })?),
    }
}

/// The text of a file, or None if there is none.
fn read(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            why: e.to_string(),
        }),
    }
}

/// The variable that names the configuration file (D.11.9).
pub const ENV_CONFIG: &str = "FORUM_CONFIG";
/// The variable of `listen`.
pub const ENV_LISTEN: &str = "FORUM_LISTEN";
/// The variable of `database`.
pub const ENV_DATABASE: &str = "FORUM_DATABASE";
/// The variable of `search_database`; empty, no search.
pub const ENV_SEARCH_DATABASE: &str = "FORUM_SEARCH_DATABASE";
/// The variable of `allowed_hosts`: entries separated by commas or whitespace.
pub const ENV_ALLOWED_HOSTS: &str = "FORUM_ALLOWED_HOSTS";
/// Every settings variable that `serve` reads, which a test that runs the binary removes from the
/// environment it passes on, so that a developer's own variables do not configure its server.
pub const ENV_SETTINGS: [&str; 5] = [
    ENV_CONFIG,
    ENV_LISTEN,
    ENV_DATABASE,
    ENV_SEARCH_DATABASE,
    ENV_ALLOWED_HOSTS,
];

/// The `allowed_hosts` entry that turns rmcp's `Host` validation off: every `Host` header is
/// accepted at `/mcp`, and the bearer tokens alone guard it.
pub const ANY_HOST: &str = "*";

/// Why an empty `allowed_hosts` list stops the server: rmcp would take it as no validation at all.
const EMPTY_HOSTS: &str = "an empty list would accept every host: name the hosts, or \"*\" to \
                           accept every host";

/// Settings that override the file's (D.11.9): the environment's `FORUM_*` variables
/// ([`from_environment`]), or the command line's flags. None leaves a setting as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// The configuration file.
    pub config: Option<PathBuf>,
    /// `listen`.
    pub listen: Option<String>,
    /// `database`; the search file follows it unless it was named apart ([`Config::set_database`]).
    pub database: Option<PathBuf>,
    /// `search_database`; `Some(None)`: no search.
    pub search_database: Option<Option<PathBuf>>,
    /// `allowed_hosts`, as [`allowed_hosts`] gives them.
    pub allowed_hosts: Option<Vec<String>>,
}

impl Overrides {
    /// Sets in `c` what these settings name, the database before the search file.
    fn apply(&self, c: &mut Config) {
        if let Some(l) = &self.listen {
            c.listen.clone_from(l);
        }
        if let Some(db) = &self.database {
            c.set_database(db.clone());
        }
        if let Some(s) = &self.search_database {
            c.search_database.clone_from(s);
        }
        if let Some(h) = &self.allowed_hosts {
            c.allowed_hosts.clone_from(h);
        }
    }
}

/// The process environment's `FORUM_*` variables: the settings' (D.11.9) and the tokens' (D.11.3).
/// Only the tokens are also read from `.env`.
#[must_use]
pub fn environment() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(k, _)| k.starts_with("FORUM_"))
        .collect()
}

/// The settings the environment's variables give (D.11.9). A variable that is unset or empty
/// leaves its setting to the file, except `FORUM_SEARCH_DATABASE`, where empty means no search, as
/// the flag's and the file's empty value do; a `FORUM_ALLOWED_HOSTS` with no entry is unset too, so
/// that a variable a platform left empty never turns the validation off.
///
/// # Errors
///
/// A `FORUM_ALLOWED_HOSTS` entry that [`allowed_host`] refuses.
pub fn from_environment(vars: &[(String, String)]) -> Result<Overrides, ConfigError> {
    let get = |name: &str| {
        vars.iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let set = |name: &str| get(name).filter(|v| !v.is_empty());
    let allowed_hosts = match get(ENV_ALLOWED_HOSTS) {
        None => None,
        Some(v) => allowed_hosts(
            v.split(|c: char| c == ',' || c.is_whitespace())
                .filter(|e| !e.is_empty()),
        )
        .map_err(|why| setting(ENV_ALLOWED_HOSTS, why))?,
    };
    Ok(Overrides {
        config: set(ENV_CONFIG).map(PathBuf::from),
        listen: set(ENV_LISTEN).map(str::to_string),
        database: set(ENV_DATABASE).map(PathBuf::from),
        search_database: get(ENV_SEARCH_DATABASE)
            .map(|v| (!v.is_empty()).then(|| PathBuf::from(v))),
        allowed_hosts,
    })
}

/// The configuration at startup (D.11.9): the defaults, then the file, then the environment's
/// settings, then the command line's flags, each over the ones before it. The file is the flags'
/// `--config`, else `FORUM_CONFIG`, else none.
///
/// # Errors
///
/// As [`load`].
pub fn resolve(environment: &Overrides, flags: &Overrides) -> Result<Config, ConfigError> {
    let file = flags.config.as_deref().or(environment.config.as_deref());
    let mut c = load(file)?;
    environment.apply(&mut c);
    flags.apply(&mut c);
    Ok(c)
}

/// One `allowed_hosts` entry as rmcp 3.5 compares it (D.11.9). rmcp parses an entry and a
/// request's `Host` header as `host[:port]`, compares the host names without case or IPv6
/// brackets, and compares the ports only when the entry has one: an entry without a port accepts
/// its host at any port. An entry is therefore its host name alone, in lower case, and a value
/// that a platform generates works as it is: a scheme (`https://`), a path, a query, and a
/// trailing slash are dropped (`https://forum.example.com/` gives `forum.example.com`), and so is
/// a port, which behind a proxy is the proxy's or, in a Coolify domain
/// (`https://forum.example.com:8640`), the container's, rather than the one a client's `Host`
/// header carries. [`ANY_HOST`] stays as it is.
///
/// # Errors
///
/// An entry that names no host, one that is not a host name, or a wildcard other than `*` alone,
/// which rmcp would compare as text and never match.
pub fn allowed_host(entry: &str) -> Result<String, String> {
    let e = entry.trim();
    if e == ANY_HOST {
        return Ok(ANY_HOST.into());
    }
    let rest = match e.split_once("://") {
        Some((scheme, rest)) if is_scheme(scheme) => rest,
        _ => e,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err(format!("{entry:?} names no host"));
    }
    let host = match Authority::try_from(authority) {
        Ok(a) => {
            // The parser takes a port that is not a number for no port: refuse it, not drop it.
            let s = a.as_str();
            let tail = s.rfind(a.host()).map_or("", |i| &s[i + a.host().len()..]);
            if let Some(port) = tail.strip_prefix(':')
                && !port.is_empty()
                && port.parse::<u16>().is_err()
            {
                return Err(format!("{entry:?}: the port is not a number"));
            }
            a.host().to_ascii_lowercase()
        }
        // A bare IPv6 address, which is no authority without its brackets.
        Err(_) if authority.parse::<Ipv6Addr>().is_ok() => {
            format!("[{}]", authority.to_ascii_lowercase())
        }
        Err(_) => return Err(format!("{entry:?} is not a host name")),
    };
    if host.is_empty() {
        return Err(format!("{entry:?} names no host"));
    }
    if host.contains('*') {
        return Err(format!(
            "{entry:?}: rmcp matches no wildcard in a name; \"*\" alone accepts every host"
        ));
    }
    Ok(host)
}

/// Whether `s` is a URI scheme (RFC 3986: a letter, then letters, digits, `+`, `-`, and `.`).
fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The `allowed_hosts` entries, each as [`allowed_host`] gives it, without repeats; with `*`
/// among them, `["*"]`. None for no entry.
///
/// # Errors
///
/// As [`allowed_host`], for any entry.
pub fn allowed_hosts<'a>(
    entries: impl IntoIterator<Item = &'a str>,
) -> Result<Option<Vec<String>>, String> {
    let mut hosts: Vec<String> = Vec::new();
    for e in entries {
        let h = allowed_host(e)?;
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    if hosts.iter().any(|h| h == ANY_HOST) {
        return Ok(Some(vec![ANY_HOST.into()]));
    }
    Ok((!hosts.is_empty()).then_some(hosts))
}

/// A warning the startup logs about the allowed hosts of a server bound to an address (D.11.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostWarning {
    /// `*`: rmcp's `Host` validation is off.
    AnyHost,
    /// The server listens beyond loopback, but only loopback names are allowed: a request that
    /// comes through a domain gets HTTP 403 from rmcp.
    LoopbackOnly,
}

impl fmt::Display for HostWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HostWarning::AnyHost => {
                "the allowed hosts are \"*\": every Host header is accepted at /mcp, and the \
                 bearer tokens alone guard it"
            }
            HostWarning::LoopbackOnly => {
                "listening beyond loopback with loopback names alone as allowed hosts: requests \
                 through a domain will be refused (HTTP 403) until FORUM_ALLOWED_HOSTS names it"
            }
        })
    }
}

/// The warning, if any, about `allowed_hosts` for a server bound to `listen` (D.11.9).
#[must_use]
pub fn host_warning(listen: IpAddr, allowed_hosts: &[String]) -> Option<HostWarning> {
    if allowed_hosts.is_empty() || allowed_hosts.iter().any(|h| h == ANY_HOST) {
        Some(HostWarning::AnyHost)
    } else if !listen.is_loopback() && allowed_hosts.iter().all(|h| is_loopback_name(h)) {
        Some(HostWarning::LoopbackOnly)
    } else {
        None
    }
}

/// Whether a host name always means this machine: `localhost`, a name under it, or a loopback
/// address.
fn is_loopback_name(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h == "localhost"
        || h.ends_with(".localhost")
        || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The tokens at startup: the `FORUM_TOKEN_*` variables among `environment` ([`environment`]),
/// and the `.env` file's at `dotenv`, if there is one.
///
/// # Errors
///
/// A `.env` file that cannot be read or parsed, or a malformed token line: the server stops.
pub fn load_tokens(
    environment: Vec<(String, String)>,
    dotenv: &Path,
) -> Result<TokenMap, ConfigError> {
    let text = read(dotenv)?;
    TokenMap::load(environment, text.as_deref()).map_err(ConfigError::Tokens)
}

/// The tokens after a SIGHUP: the environment's as at startup, and the `.env` file's as it is now.
///
/// # Errors
///
/// As [`load_tokens`]; the caller keeps the old map.
pub fn reload_tokens(current: &TokenMap, dotenv: &Path) -> Result<TokenMap, ConfigError> {
    let text = read(dotenv)?;
    current.reload(text.as_deref()).map_err(ConfigError::Tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{hex, sha256};
    use forum_core::{Dur, MINUTE, SECOND};

    #[test]
    fn the_example_of_d_11_9_parses() {
        let c = parse(
            r#"
listen          = "127.0.0.1:8640"
database        = "forum.db"
search_database = "forum-search.db"
allowed_hosts   = ["localhost", "127.0.0.1"]
instructions    = "default"
[params]
lease_first     = "15m"
hold_per_session = 2
refusal_quorum  = 3
queue_max_wait  = "10s"
text_max        = 65536
presence_resolution = "3m"
session_retain  = "24h"
op_retain       = "24h"
conversation_schemes = ["chat", "issue", "mail"]
"#,
        )
        .unwrap();
        assert_eq!(c.listen, "127.0.0.1:8640");
        assert_eq!(c.search_database, Some(PathBuf::from("forum-search.db")));
        assert_eq!(c.params.lease_first, Dur(15 * MINUTE.0));
        assert_eq!(c.params.queue_max_wait, Dur(10 * SECOND.0));
        assert_eq!(c.params.hold_per_session, 2);
        assert_eq!(c.params.text_max, 65_536);
        assert_eq!(
            c.params,
            Params {
                lease_first: Dur(15 * MINUTE.0),
                hold_per_session: 2,
                refusal_quorum: 3,
                ..c.params.clone()
            }
        );
        // Without a file, the defaults of §22.
        assert_eq!(load(None).unwrap(), Config::default());
        // Milliseconds as an integer.
        assert_eq!(
            parse("[params]\nqueue_max_wait = 12000")
                .unwrap()
                .params
                .queue_max_wait,
            Dur(12_000)
        );
    }

    /// The search file follows the database unless it is named apart from it (review E4-spec-6).
    #[test]
    fn the_search_file_sits_beside_its_database() {
        assert_eq!(
            search_path_for(Path::new("forum.db")),
            PathBuf::from("forum-search.db")
        );
        assert_eq!(
            search_path_for(Path::new("/var/lib/a/other.sqlite")),
            PathBuf::from("/var/lib/a/other-search.db")
        );
        // The default, `--db`, and the file's `database` move it along.
        let mut c = Config::default();
        c.set_database(PathBuf::from("/srv/b.db"));
        assert_eq!(c.search_database, Some(PathBuf::from("/srv/b-search.db")));
        let c = parse("database = \"/srv/c.db\"").unwrap();
        assert_eq!(c.search_database, Some(PathBuf::from("/srv/c-search.db")));
        // A search file named apart, or none, stays.
        let mut c = parse("database = \"/srv/c.db\"\nsearch_database = \"/idx/s.db\"").unwrap();
        assert_eq!(c.search_database, Some(PathBuf::from("/idx/s.db")));
        c.set_database(PathBuf::from("/srv/d.db"));
        assert_eq!(c.search_database, Some(PathBuf::from("/idx/s.db")));
        let mut c = parse("search_database = \"\"").unwrap();
        c.set_database(PathBuf::from("/srv/d.db"));
        assert_eq!(c.search_database, None);
    }

    #[test]
    fn a_bad_setting_stops_the_server() {
        assert!(matches!(
            parse("listen = 8640"),
            Err(ConfigError::Setting { key, .. }) if key == "listen"
        ));
        assert!(matches!(
            parse("tokens = []"),
            Err(ConfigError::Setting { key, .. }) if key == "tokens"
        ));
        assert!(matches!(
            parse("[params]\nlease_firts = \"15m\""),
            Err(ConfigError::Setting { key, .. }) if key == "params.lease_firts"
        ));
        assert!(matches!(
            parse("[params]\nlease_first = \"soon\""),
            Err(ConfigError::Setting { key, .. }) if key == "params.lease_first"
        ));
        // text.max below digest.max breaks a rule of §22 (Params::valid).
        assert_eq!(parse("[params]\ntext_max = 10"), Err(ConfigError::Params));
        assert!(matches!(parse("listen = "), Err(ConfigError::Toml(_))));
    }

    #[test]
    fn tokens_come_from_the_environment_and_dotenv_and_reload_from_dotenv() {
        let dir = std::env::temp_dir().join(format!("forum-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dotenv = dir.join(".env");
        let line = |t: &str, rest: &str| format!("{};{rest}", hex(&sha256(t.as_bytes())));
        std::fs::write(
            &dotenv,
            format!(
                "# tokens\nFORUM_TOKEN_A={}\nFORUM_TOKEN_B={}\n",
                line("fsk_a", "class=coding-agent"),
                line("fsk_b", "class=ide-agent")
            ),
        )
        .unwrap();
        let env = vec![(
            "FORUM_TOKEN_B".to_string(),
            line("fsk_b2", "class=ci-agent;caps=rust"),
        )];
        let m = load_tokens(env, &dotenv).unwrap();
        assert_eq!(m.lookup("fsk_a").unwrap().class.as_str(), "coding-agent");
        assert!(m.lookup("fsk_b").is_none(), "the environment wins");
        assert_eq!(m.lookup("fsk_b2").unwrap().class.as_str(), "ci-agent");
        // A reload re-reads .env: A is revoked, C added; the environment's B stays.
        std::fs::write(
            &dotenv,
            format!("FORUM_TOKEN_C={}\n", line("fsk_c", "class=coding-agent")),
        )
        .unwrap();
        let r = reload_tokens(&m, &dotenv).unwrap();
        assert!(r.lookup("fsk_a").is_none());
        assert!(r.lookup("fsk_c").is_some());
        assert!(r.lookup("fsk_b2").is_some());
        // A malformed file is refused: the caller keeps the old map.
        std::fs::write(&dotenv, "FORUM_TOKEN_D=nonsense\n").unwrap();
        assert!(reload_tokens(&r, &dotenv).is_err());
        // No .env: the environment alone.
        std::fs::remove_file(&dotenv).unwrap();
        assert_eq!(load_tokens(Vec::new(), &dotenv).unwrap().len(), 0);
        let _ = std::fs::remove_dir(&dir);
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| (*h).to_string()).collect()
    }

    /// D.11.9: a flag over its variable, a variable over the file, the file over the default.
    #[test]
    fn the_environment_wins_over_the_file_and_the_flags_over_the_environment() {
        let dir = std::env::temp_dir().join(format!("forum-config-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("forum.toml");
        std::fs::write(
            &file,
            "listen = \"127.0.0.1:7000\"\ndatabase = \"/srv/file.db\"\n\
             allowed_hosts = [\"file.example.com\"]\n[params]\nhold_per_session = 2\n",
        )
        .unwrap();
        let path = file.to_string_lossy().into_owned();
        let none = Overrides::default();
        // The file alone, named by FORUM_CONFIG.
        let env = from_environment(&vars(&[(ENV_CONFIG, &path)])).unwrap();
        let c = resolve(&env, &none).unwrap();
        assert_eq!(c.listen, "127.0.0.1:7000");
        assert_eq!(c.database, PathBuf::from("/srv/file.db"));
        assert_eq!(
            c.search_database,
            Some(PathBuf::from("/srv/file-search.db"))
        );
        assert_eq!(c.allowed_hosts, hosts(&["file.example.com"]));
        assert_eq!(c.params.hold_per_session, 2);
        // The environment over the file; the search file follows the database it names.
        let env = from_environment(&vars(&[
            (ENV_CONFIG, &path),
            (ENV_LISTEN, "0.0.0.0:8640"),
            (ENV_DATABASE, "/data/forum.db"),
            (ENV_ALLOWED_HOSTS, "https://forum.example.com"),
            ("FORUM_TOKEN_CI", "not a setting"),
        ]))
        .unwrap();
        let c = resolve(&env, &none).unwrap();
        assert_eq!(c.listen, "0.0.0.0:8640");
        assert_eq!(c.database, PathBuf::from("/data/forum.db"));
        assert_eq!(
            c.search_database,
            Some(PathBuf::from("/data/forum-search.db"))
        );
        assert_eq!(c.allowed_hosts, hosts(&["forum.example.com"]));
        assert_eq!(
            c.params.hold_per_session, 2,
            "the file still gives the rest"
        );
        // The flags over the environment.
        let flags = Overrides {
            listen: Some("127.0.0.1:0".into()),
            database: Some(PathBuf::from("/tmp/flag.db")),
            search_database: Some(None),
            ..Overrides::default()
        };
        let c = resolve(&env, &flags).unwrap();
        assert_eq!(c.listen, "127.0.0.1:0");
        assert_eq!(c.database, PathBuf::from("/tmp/flag.db"));
        assert_eq!(c.search_database, None);
        assert_eq!(c.allowed_hosts, hosts(&["forum.example.com"]));
        // `--config` over FORUM_CONFIG: a missing file named by the variable is not read.
        let env = from_environment(&vars(&[(ENV_CONFIG, "/nowhere/forum.toml")])).unwrap();
        assert!(matches!(
            resolve(&env, &none),
            Err(ConfigError::Read { .. })
        ));
        let flags = Overrides {
            config: Some(file.clone()),
            ..Overrides::default()
        };
        assert_eq!(resolve(&env, &flags).unwrap().listen, "127.0.0.1:7000");
        // FORUM_SEARCH_DATABASE: empty is no search, and a file named apart stays when a flag
        // moves the database; the flag's `--search-db` wins over it.
        let env = from_environment(&vars(&[(ENV_SEARCH_DATABASE, "")])).unwrap();
        assert_eq!(resolve(&env, &none).unwrap().search_database, None);
        let env = from_environment(&vars(&[(ENV_SEARCH_DATABASE, "/idx/s.db")])).unwrap();
        let flags = Overrides {
            database: Some(PathBuf::from("/srv/d.db")),
            ..Overrides::default()
        };
        assert_eq!(
            resolve(&env, &flags).unwrap().search_database,
            Some(PathBuf::from("/idx/s.db"))
        );
        let flags = Overrides {
            search_database: Some(Some(PathBuf::from("/flag/s.db"))),
            ..Overrides::default()
        };
        assert_eq!(
            resolve(&env, &flags).unwrap().search_database,
            Some(PathBuf::from("/flag/s.db"))
        );
        // An empty variable is unset (a platform's variable left empty): the file's, or the
        // default, stays; a FORUM_ALLOWED_HOSTS with no entry never turns the validation off.
        let env = from_environment(&vars(&[
            (ENV_CONFIG, &path),
            (ENV_LISTEN, ""),
            (ENV_DATABASE, ""),
            (ENV_ALLOWED_HOSTS, " , "),
        ]))
        .unwrap();
        assert_eq!(env.allowed_hosts, None);
        let c = resolve(&env, &none).unwrap();
        assert_eq!(c.listen, "127.0.0.1:7000");
        assert_eq!(c.database, PathBuf::from("/srv/file.db"));
        assert_eq!(c.allowed_hosts, hosts(&["file.example.com"]));
        let env = from_environment(&vars(&[(ENV_CONFIG, ""), (ENV_ALLOWED_HOSTS, "")])).unwrap();
        assert_eq!(resolve(&env, &none).unwrap(), Config::default());
        // No variable, no flag: the defaults.
        assert_eq!(resolve(&none, &none).unwrap(), Config::default());
        // A bad entry stops the server, and names the variable.
        assert!(matches!(
            from_environment(&vars(&[(ENV_ALLOWED_HOSTS, "https://")])),
            Err(ConfigError::Setting { key, .. }) if key == ENV_ALLOWED_HOSTS
        ));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }

    /// D.11.9: an entry is the host name that rmcp compares, so that a value a platform generates
    /// (Coolify's `SERVICE_FQDN_*` or `SERVICE_URL_*`) works as it is.
    #[test]
    fn allowed_hosts_are_reduced_to_their_host_names() {
        let one = |e: &str| allowed_host(e);
        // A bare host, in lower case.
        assert_eq!(one("forum.example.com"), Ok("forum.example.com".into()));
        assert_eq!(one(" Forum.Example.COM "), Ok("forum.example.com".into()));
        // A URL: the scheme, the path, the query, and the trailing slash go.
        assert_eq!(
            one("https://forum.example.com"),
            Ok("forum.example.com".into())
        );
        assert_eq!(
            one("http://forum.example.com/"),
            Ok("forum.example.com".into())
        );
        assert_eq!(
            one("HTTPS://forum.example.com/mcp/?x=1#top"),
            Ok("forum.example.com".into())
        );
        assert_eq!(one("forum.example.com/mcp"), Ok("forum.example.com".into()));
        // A port goes: rmcp accepts an entry without one at any port, and Coolify's domain
        // carries the container's port (8640) while clients come through 443.
        assert_eq!(
            one("forum.example.com:8640"),
            Ok("forum.example.com".into())
        );
        assert_eq!(
            one("https://forum.example.com:8640/"),
            Ok("forum.example.com".into())
        );
        assert_eq!(one("127.0.0.1:8640"), Ok("127.0.0.1".into()));
        // IPv6, with or without brackets and a port.
        assert_eq!(one("::1"), Ok("[::1]".into()));
        assert_eq!(one("[::1]"), Ok("[::1]".into()));
        assert_eq!(one("http://[::1]:8640/"), Ok("[::1]".into()));
        // `*` alone turns the validation off; a wildcard in a name rmcp would never match.
        assert_eq!(one("*"), Ok(ANY_HOST.into()));
        assert!(one("*.example.com").is_err());
        // Nothing to name, or not a host.
        assert!(one("").is_err());
        assert!(one("https://").is_err());
        assert!(one("https:///mcp").is_err());
        assert!(one(":8640").is_err());
        assert!(one("forum.example.com:port").is_err());
        assert!(one("a b").is_err());

        // A list: commas or whitespace, repeats dropped, the order kept.
        let env = |v: &str| {
            from_environment(&vars(&[(ENV_ALLOWED_HOSTS, v)]))
                .unwrap()
                .allowed_hosts
        };
        assert_eq!(
            env("forum.example.com"),
            Some(hosts(&["forum.example.com"]))
        );
        assert_eq!(
            env("https://forum.example.com,localhost , 127.0.0.1:8640\n\tFORUM.example.com:443"),
            Some(hosts(&["forum.example.com", "localhost", "127.0.0.1"]))
        );
        assert_eq!(env("localhost *"), Some(hosts(&[ANY_HOST])));
        assert_eq!(env(""), None);
        // The file's list is reduced the same way; an empty one would accept every host.
        assert_eq!(
            parse("allowed_hosts = [\"https://Forum.example.com:8640/\", \"[::1]\"]")
                .unwrap()
                .allowed_hosts,
            hosts(&["forum.example.com", "[::1]"])
        );
        assert!(matches!(
            parse("allowed_hosts = []"),
            Err(ConfigError::Setting { key, .. }) if key == "allowed_hosts"
        ));
        assert!(matches!(
            parse("allowed_hosts = [\"https://\"]"),
            Err(ConfigError::Setting { key, .. }) if key == "allowed_hosts"
        ));
        assert_eq!(
            parse("allowed_hosts = [\"*\"]").unwrap().allowed_hosts,
            hosts(&[ANY_HOST])
        );
    }

    /// D.11.9: the startup warns when a server that listens beyond loopback allows loopback names
    /// alone (requests through a domain get 403), and when `*` turns the validation off.
    #[test]
    fn the_startup_warns_of_allowed_hosts_that_refuse_a_domain() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let loopback = Config::default().allowed_hosts;
        // The default: listening on loopback, loopback names.
        assert_eq!(host_warning(ip("127.0.0.1"), &loopback), None);
        assert_eq!(host_warning(ip("::1"), &loopback), None);
        // A container listens on every interface: a domain would be refused.
        assert_eq!(
            host_warning(ip("0.0.0.0"), &loopback),
            Some(HostWarning::LoopbackOnly)
        );
        assert_eq!(
            host_warning(ip("::"), &loopback),
            Some(HostWarning::LoopbackOnly)
        );
        assert_eq!(
            host_warning(
                ip("10.0.0.5"),
                &hosts(&["app.localhost", "[::1]", "127.0.0.2"])
            ),
            Some(HostWarning::LoopbackOnly)
        );
        assert!(
            HostWarning::LoopbackOnly
                .to_string()
                .contains("until FORUM_ALLOWED_HOSTS names it")
        );
        // A name beside them, and nothing to warn of.
        assert_eq!(
            host_warning(ip("0.0.0.0"), &hosts(&["forum.example.com"])),
            None
        );
        assert_eq!(
            host_warning(ip("0.0.0.0"), &hosts(&["localhost", "forum.example.com"])),
            None
        );
        assert_eq!(host_warning(ip("0.0.0.0"), &hosts(&["10.0.0.5"])), None);
        // `*`, wherever it listens.
        assert_eq!(
            host_warning(ip("127.0.0.1"), &hosts(&[ANY_HOST])),
            Some(HostWarning::AnyHost)
        );
        assert_eq!(
            host_warning(ip("0.0.0.0"), &hosts(&[ANY_HOST])),
            Some(HostWarning::AnyHost)
        );
    }
}
