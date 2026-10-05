//! The `.env` parser (design D.11.3): the shell's own, with no crate.
//!
//! The format is `KEY=VALUE` lines, `#` comments, and optional quotes, nothing else:
//! - a line that is empty or holds only whitespace is skipped;
//! - a line whose first non-blank character is `#` is a comment;
//! - otherwise the line is `KEY=VALUE`: the key is `[A-Za-z_][A-Za-z0-9_]*`, whitespace around the
//!   key and around the value is trimmed, and a value wrapped in one pair of matching `"` or `'`
//!   loses them (there are no escapes, and nothing may follow the closing quote);
//! - anything else (no `=`, a malformed key, `export KEY=…`, an unclosed quote) is an error that
//!   names the line, and stops the server at startup (D.12.1).
//!
//! The parser never reads a file: the caller passes the text (the configuration reads it once,
//! D.11.2).

use std::fmt;

/// One `KEY=VALUE` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The key.
    pub key: String,
    /// The value, without its quotes.
    pub value: String,
    /// The line's number, from 1.
    pub line: usize,
}

/// A line that is not `KEY=VALUE`, a comment, or blank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DotenvError {
    /// The line's number, from 1.
    pub line: usize,
    /// What is wrong with it.
    pub why: &'static str,
}

impl fmt::Display for DotenvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ".env line {}: {}", self.line, self.why)
    }
}

impl std::error::Error for DotenvError {}

/// Parses the text of a `.env` file into its `KEY=VALUE` lines, in order.
pub fn parse(text: &str) -> Result<Vec<Line>, DotenvError> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(DotenvError {
                line,
                why: "expected KEY=VALUE",
            });
        };
        let key = key.trim();
        if !is_key(key) {
            return Err(DotenvError {
                line,
                why: "the key must match [A-Za-z_][A-Za-z0-9_]*",
            });
        }
        let value = unquote(value.trim()).ok_or(DotenvError {
            line,
            why: "a quoted value must end with its opening quote, and nothing may follow it",
        })?;
        out.push(Line {
            key: key.to_string(),
            value: value.to_string(),
            line,
        });
    }
    Ok(out)
}

fn is_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Strips one pair of matching quotes; None for a value that opens a quote it does not close.
fn unquote(value: &str) -> Option<&str> {
    let Some(q) = value.chars().next().filter(|c| *c == '"' || *c == '\'') else {
        return Some(value);
    };
    let inner = &value[1..];
    let close = inner.find(q)?;
    if close + 1 != inner.len() {
        return None;
    }
    Some(&inner[..close])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(lines: &[Line]) -> Vec<(&str, &str)> {
        lines
            .iter()
            .map(|l| (l.key.as_str(), l.value.as_str()))
            .collect()
    }

    #[test]
    fn keys_values_comments_and_quotes() {
        let text = "# .env — <NAME> is only a label\n\
                    FORUM_TOKEN_CI=9f2c;class=coding-agent\n\
                    \n\
                    \t  # an indented comment\n\
                    FORUM_TOKEN_IDE = \"41ab;class=ide-agent\"\n\
                    OTHER='a # not a comment'\n\
                    EMPTY=\n\
                    HASH=a#b\n";
        let lines = parse(text).unwrap();
        assert_eq!(
            kv(&lines),
            [
                ("FORUM_TOKEN_CI", "9f2c;class=coding-agent"),
                ("FORUM_TOKEN_IDE", "41ab;class=ide-agent"),
                ("OTHER", "a # not a comment"),
                ("EMPTY", ""),
                ("HASH", "a#b"),
            ]
        );
        assert_eq!(lines[0].line, 2);
        assert_eq!(lines[1].line, 5);
    }

    #[test]
    fn crlf_lines_parse_like_lf_lines() {
        let lines = parse("A=1\r\nB=\"2\"\r\n").unwrap();
        assert_eq!(kv(&lines), [("A", "1"), ("B", "2")]);
    }

    #[test]
    fn anything_else_is_an_error_naming_its_line() {
        for (text, line) in [
            ("A=1\nnot a pair\n", 2),
            ("export A=1\n", 1),
            ("1A=1\n", 1),
            ("=1\n", 1),
            ("A-B=1\n", 1),
            ("A=\"open\n", 1),
            ("A=\"a\" b\n", 1),
            ("A='a\"\n", 1),
        ] {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, line, "{text:?}");
            assert!(e.to_string().starts_with(&format!(".env line {line}: ")));
        }
    }

    #[test]
    fn an_empty_file_has_no_lines() {
        assert_eq!(parse("").unwrap(), []);
        assert_eq!(parse("\n\n# only comments\n").unwrap(), []);
    }
}
