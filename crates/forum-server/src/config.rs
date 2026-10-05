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
//! allowed_hosts   = ["localhost", "127.0.0.1"]   # rmcp Host validation
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

use std::fmt;
use std::path::{Path, PathBuf};

use forum_core::Params;
use serde_json::Value;

use crate::auth::{AuthError, TokenMap, VAR_PREFIX};
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
    /// rmcp's `Host` validation.
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
                c.allowed_hosts = v
                    .as_array()
                    .ok_or_else(|| setting(k, "expected a list of host names"))?
                    .iter()
                    .map(|h| string(k, h))
                    .collect::<Result<_, _>>()?;
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

/// The process environment's `FORUM_TOKEN_*` variables (D.11.3).
#[must_use]
pub fn token_environment() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(k, _)| k.starts_with(VAR_PREFIX))
        .collect()
}

/// The tokens at startup: the environment's and the `.env` file's at `dotenv`, if there is one.
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
}
