//! The value forms of D.11.5's table: each parser reads one argument's text and says what is wrong
//! with it, for the hint.

use forum_core::{
    AddresseeArg, AgentAddressee, BasisArg, Dur, Eid, Key, KeyStr, Label, Name, NodeId, NodeRef,
    NormVer, Outcome, Path, PrincipalKey, SessionId, Text, Time, Version, When,
};
use forum_cron::CronError;

/// What is wrong with a node argument.
pub const NODE_FORM: &str = "a node is a path such as /a/b or a/b, or an id such as #12, not a key \
     or an entry id: a conversation key goes in where";

/// A node: `#12`, or a path whose steps are names (`/a/b` or `a/b`; `/` is the root).
pub fn node(raw: &str) -> Result<NodeRef, String> {
    if raw.starts_with('#') {
        return NodeId::parse(raw).map(NodeRef::Id).ok_or_else(|| {
            format!("{raw:?} is not a node id: an id is # and decimal digits, such as #12")
        });
    }
    path(raw).map(NodeRef::Path)
}

/// A path: `/` is the root; otherwise one leading `/` is optional, and every step is a name.
pub fn path(raw: &str) -> Result<Path, String> {
    if raw == "/" {
        return Ok(Path(Vec::new()));
    }
    if raw.is_empty() {
        return Err(format!("the text is empty: {NODE_FORM}"));
    }
    let rest = raw.strip_prefix('/').unwrap_or(raw);
    let mut steps = Vec::new();
    for step in rest.split('/') {
        match Name::new(step) {
            Some(n) => steps.push(n),
            None => {
                return Err(format!(
                    "{raw:?} is not a path: its step {step:?} is not a name (1 to 128 bytes, no \
                     control characters, not . or .., not starting with #, no surrounding \
                     whitespace); {NODE_FORM}"
                ));
            }
        }
    }
    Ok(Path(steps))
}

/// A place: a path only (`create {path}`, `move {to}`).
pub fn place(raw: &str) -> Result<Path, String> {
    if raw.starts_with('#') {
        return Err(format!(
            "{raw:?} is an id: this argument names a place, and takes a path only"
        ));
    }
    path(raw)
}

/// Splits `<node>@<version>` at the last `@` whose suffix is a canonical decimal.
fn split_version(raw: &str) -> Option<(&str, Version)> {
    let (node, v) = raw.rsplit_once('@')?;
    canonical_decimal(v).map(|v| (node, Version(v)))
}

/// A node at a version: `#12@4` or `/a/b@4`.
pub fn node_at(raw: &str) -> Result<(NodeRef, Version), String> {
    let (n, v) = split_version(raw).ok_or_else(|| {
        format!("{raw:?} names no version: write the node and @<version>, such as #12@4 or /a/b@4")
    })?;
    Ok((node(n)?, v))
}

/// Decimal digits without a sign or a leading zero, within `u64`.
pub fn canonical_decimal(s: &str) -> Option<u64> {
    let ok = !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_digit())
        && !(s.len() > 1 && s.starts_with('0'));
    if ok { s.parse().ok() } else { None }
}

/// A key, normalised with the current version of `forum_keys` (§7).
pub fn key(raw: &str) -> Result<Key, String> {
    let n = forum_keys::normalise(raw).map_err(|e| format!("{raw:?} is not a key: {e}"))?;
    let key = KeyStr::new(&n.key)
        .ok_or_else(|| format!("{raw:?} normalises to {:?}, which is not a key", n.key))?;
    Ok(Key {
        key,
        nv: NormVer(n.version),
    })
}

/// A principal key (CS-1, D.11.14).
pub fn principal(raw: &str) -> Result<PrincipalKey, String> {
    let n = forum_keys::normalise_principal(raw)
        .map_err(|e| format!("{raw:?} is not a principal key: {e}"))?;
    PrincipalKey::new(&n).ok_or_else(|| {
        format!("{raw:?} is not a principal key: user, or <scheme>:<value> other than user:")
    })
}

/// The `principal` of a human the caller names (`hello`, its `from_user`, `talk from_user`, and
/// `talk to_user`): a principal key, refused with a hint of its own (1.5, R3, D.14.7): `user:` is
/// reserved, and a principal is the key the client's configuration names the human by, or `user`
/// when it names none (§20.3, 1.9), since an agent that guesses one invents a person (round R3:
/// one agent turned a refused `user:<name>` into `ide:<name>`).
pub fn principal_arg(raw: &str) -> Result<PrincipalKey, String> {
    principal(raw).map_err(|_| {
        let reserved = raw
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.trim().eq_ignore_ascii_case("user"));
        let why = if reserved {
            "user: is reserved, since user:e12 names a log entry, not a person".to_string()
        } else {
            match forum_keys::normalise_principal(raw) {
                Err(e) => e.to_string(),
                Ok(_) => "it is not a principal key".to_string(),
            }
        };
        format!(
            "{raw:?} is not a principal: {why}; pass the key your client's configuration names \
             the human by, as <scheme>:<value> such as mail:alice@example.org, or user if it \
             names none"
        )
    })
}

/// A duration as a string: decimal digits and one unit of `s`, `m`, `h`, or `d`.
pub fn dur_text(raw: &str) -> Result<Dur, String> {
    let bad = || {
        format!(
            "{raw:?} is not a duration: write digits and one unit, such as 50s, 15m, 2h, or 7d, \
             or an integer of milliseconds"
        )
    };
    // The unit is the last character, which may be any character: split at its boundary, never at
    // a byte inside it (review E2-bugs-1).
    let unit = raw.chars().next_back().ok_or_else(bad)?;
    let digits = &raw[..raw.len() - unit.len_utf8()];
    let per: u64 = match unit {
        's' => 1_000,
        'm' => 60_000,
        'h' => 3_600_000,
        'd' => 86_400_000,
        _ => return Err(bad()),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    n.checked_mul(per).map(Dur).ok_or_else(bad)
}

/// A time: RFC 3339 (`2026-10-01T12:00:00Z`), `in <duration>` (A-79), or, since 1.10, a bare
/// duration, read as from now (§20.2): `2h` is `in 2h`. In the lab's fifth round (it4 l6, l7), 2
/// of 13 `recheck_after` were bare durations, which the descriptions list beside the times, and
/// both were refused.
pub fn when(raw: &str) -> Result<When, String> {
    if let Ok(d) = dur_text(raw) {
        return Ok(When::In(d));
    }
    at_or_in(raw)
}

/// A time as 1.9 read it: RFC 3339, or `in <duration>`. `checked_at` reads no bare duration, which
/// there could mean as long ago.
fn at_or_in(raw: &str) -> Result<When, String> {
    if let Some(d) = raw.strip_prefix("in ") {
        return dur_text(d.trim()).map(When::In);
    }
    let ts: jiff::Timestamp = raw.parse().map_err(|_| {
        format!(
            "{raw:?} is not a time: write an RFC 3339 time such as 2026-10-01T12:00:00Z, or a \
             duration from now such as 10m or in 10m"
        )
    })?;
    let ms = ts.as_millisecond();
    u64::try_from(ms)
        .map(|ms| When::At(Time(ms)))
        .map_err(|_| format!("{raw:?} is before 1970-01-01T00:00:00Z"))
}

/// The usual words for `done` that a close's outcome reads as `done`, with no warning (1.9, §5):
/// in the lab's fourth round, closes of finished work carried `completed` and `implemented`, which
/// rollups count as closed otherwise and an upheld objection does not correct.
pub const DONE_SYNONYMS: [&str; 5] = ["completed", "implemented", "fixed", "resolved", "merged"];

/// A close's outcome: one label (§5). The usual words for `done` ([`DONE_SYNONYMS`]) are `done`
/// (1.9); a text that is no label, such as a sentence, is refused with the usual outcomes, and
/// the sentence belongs in `summary` (1.9: the label's form was learned by refusal).
pub fn outcome(raw: &str) -> Result<Outcome, String> {
    Ok(match raw {
        "done" => Outcome::Done,
        "failed" => Outcome::Failed,
        "dropped" => Outcome::Dropped,
        "superseded" => Outcome::Superseded,
        "missed" => Outcome::Missed,
        s if DONE_SYNONYMS.contains(&s) => Outcome::Done,
        other => Outcome::Other(Label::new(other).ok_or_else(|| {
            format!(
                "{other:?} is not an outcome: one label, usually done, failed, dropped, \
                 superseded, or refuted ([a-z0-9][a-z0-9_.:-]{{0,63}}); write the sentence in \
                 summary"
            )
        })?),
    })
}

/// The addressees of a later release (§13, A-83).
fn later_release(raw: &str) -> Option<String> {
    let later = raw.starts_with("class:") || raw.starts_with("cap:") || raw == "everyone";
    later.then(|| {
        format!(
            "{raw:?}: class:, cap:, and everyone addressees are a later release; address \
             session:<id>, holder:<node>, holders:<node>/**, principal:<key>, or user"
        )
    })
}

/// `session:s-4`.
fn session_addressee(rest: &str) -> Result<SessionId, String> {
    SessionId::parse(rest).ok_or_else(|| format!("{rest:?} is not a session id such as s-4"))
}

/// The node of `holders:<node>/**`.
fn holders_node(rest: &str) -> Result<NodeRef, String> {
    let inner = rest
        .strip_suffix("/**")
        .ok_or_else(|| format!("holders:{rest} must end with /**, as in holders:/a/b/**"))?;
    node(if inner.is_empty() { "/" } else { inner })
}

/// The `to` of `say` (§13).
pub fn addressee(raw: &str) -> Result<AddresseeArg, String> {
    if let Some(why) = later_release(raw) {
        return Err(why);
    }
    if raw == "user" {
        return Ok(AddresseeArg::User);
    }
    if let Some(rest) = raw.strip_prefix("principal:") {
        return principal(rest).map(AddresseeArg::Principal);
    }
    agent(raw).map(|a| match a {
        AgentAddressee::Session(s) => AddresseeArg::Session(s),
        AgentAddressee::Holder(n) => AddresseeArg::Holder(n),
        AgentAddressee::Holders(n) => AddresseeArg::Holders(n),
    })
}

/// The `to` of a question for agents (§13). A bare `s-4` is `session:s-4` (1.7: agents wrote
/// it so, l1 and l2 of the lab's second round).
pub fn agent(raw: &str) -> Result<AgentAddressee, String> {
    if let Some(why) = later_release(raw) {
        return Err(why);
    }
    if let Some(rest) = raw.strip_prefix("session:") {
        return session_addressee(rest).map(AgentAddressee::Session);
    }
    if let Some(s) = SessionId::parse(raw) {
        return Ok(AgentAddressee::Session(s));
    }
    if let Some(rest) = raw.strip_prefix("holders:") {
        return holders_node(rest).map(AgentAddressee::Holders);
    }
    if let Some(rest) = raw.strip_prefix("holder:") {
        return node(rest).map(AgentAddressee::Holder);
    }
    Err(format!(
        "{raw:?} is not an addressee: session:<id>, holder:<node>, holders:<node>/**, \
         principal:<key>, or user"
    ))
}

/// The `to` of a question for a human: `user` (None) or `principal:<key>`.
pub fn ask_human(raw: &str) -> Result<Option<PrincipalKey>, String> {
    if raw == "user" {
        return Ok(None);
    }
    match raw.strip_prefix("principal:") {
        Some(rest) => principal(rest).map(Some),
        None => Err(format!("{raw:?}: a human is user or principal:<key>")),
    }
}

/// Whether a basis string is written as a node or entry reference (1.6, §15): it starts with `#`
/// or `/`, is `e57` or starts with `entry:`, or is a relative path, names joined by `/` without
/// whitespace whose first holds no `:`. Any other string, prose, a URL, or a key such as
/// `arxiv:2101.00001`, is something outside the forum: agents wrote all of these (l1, l2), and a
/// line of prose was read as a path and not found.
#[must_use]
pub fn written_as_reference(raw: &str) -> bool {
    let t = raw.trim();
    t.starts_with('#')
        || t.starts_with('/')
        || t.starts_with("entry:")
        || Eid::parse(t).is_some()
        || (t.contains('/')
            && !t.contains(char::is_whitespace)
            && t.split('/')
                .next()
                .is_some_and(|first| !first.contains(':')))
}

/// One basis reference written as a string (§15, CS-22): an entry (`e57`, `entry:e57`), a node at
/// a version (`#12@4`, a path with `@4`), or a node (`#12`, or a bare path, 1.5, R3: an agent
/// that wrote one was refused, though `#12` and a path with `@4` were taken); and any string not
/// written as one of those ([`written_as_reference`]), something outside the forum checked now
/// (1.6), as `{external: <the string>}` with `checked_at` now, which the core resolves.
pub fn basis_text(raw: &str) -> Result<BasisArg, String> {
    let bad = || {
        format!(
            "{raw:?} is not a basis reference: #12@4 or a path with @4 (a node at a version), #12 \
             or a path (a node), e57 or entry:e57 (an entry); any other text is something \
             outside the forum, as is {{external, checked_at?, recheck?}}"
        )
    };
    let t = raw.trim();
    if t.is_empty() {
        return Err(bad());
    }
    if !written_as_reference(t) {
        return Ok(BasisArg::External {
            what: Text(t.to_string()),
            checked_at: When::In(Dur(0)),
            recheck: None,
        });
    }
    if let Some(e) = t.strip_prefix("entry:") {
        return Eid::parse(e).map(BasisArg::Entry).ok_or_else(bad);
    }
    if let Some(e) = Eid::parse(t) {
        return Ok(BasisArg::Entry(e));
    }
    if let Some((n, v)) = split_version(t) {
        return node(n)
            .map(|at| BasisArg::Node {
                at,
                version: Some(v),
            })
            .map_err(|_| bad());
    }
    node(t)
        .map(|at| BasisArg::Node { at, version: None })
        .map_err(|_| bad())
}

/// When something outside the forum was checked (`checked_at`, 1.6, §15): `now`; a date
/// `2026-10-04`, read as 00:00 UTC of that day; or a time as [`when`] reads it, but a bare
/// duration (1.10), which here could mean as long ago. A time after now is not refused here: the
/// core reads it as now. Agents wrote `now` and a date (l2, l6).
pub fn checked_at(raw: &str) -> Result<When, String> {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("now") {
        return Ok(When::In(Dur(0)));
    }
    let date_shaped = t.len() == 10
        && t.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    if date_shaped {
        let midnight = t
            .parse::<jiff::civil::Date>()
            .ok()
            .and_then(|d| d.to_zoned(jiff::tz::TimeZone::UTC).ok())
            .map(|z| z.timestamp().as_millisecond())
            .and_then(|ms| u64::try_from(ms).ok());
        return midnight
            .map(|ms| When::At(Time(ms)))
            .ok_or_else(|| format!("{raw:?} is not a date from 1970-01-01 on"));
    }
    at_or_in(t).map_err(|_| {
        format!(
            "{raw:?} is not a time: write now, a date such as 2026-10-04, or an RFC 3339 time \
             such as 2026-10-04T09:30:00Z"
        )
    })
}

/// The hint of a cron expression `forum_cron` refuses.
pub fn cron_error(e: &CronError) -> String {
    const FIELDS: [&str; 5] = ["minute", "hour", "day of month", "month", "day of week"];
    let field = |i: &u8| FIELDS.get(usize::from(*i)).copied().unwrap_or("a field");
    match e {
        CronError::FieldCount => {
            "a cron expression has five fields: minute, hour, day of month, month, day of week"
                .to_string()
        }
        CronError::Field { index } => format!(
            "the {} field is not a list of *, values, ranges, and steps (?, L, W, and # are not \
             supported)",
            field(index)
        ),
        CronError::Range { index } => format!(
            "a value of the {} field is out of range, or a range runs backwards",
            field(index)
        ),
        CronError::Step { index } => format!(
            "a step of the {} field is not a positive number after * or a range",
            field(index)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(steps: &[&str]) -> NodeRef {
        NodeRef::Path(Path(steps.iter().map(|s| Name::new(s).unwrap()).collect()))
    }

    #[test]
    fn nodes_and_paths() {
        assert_eq!(node("#12"), Ok(NodeRef::Id(NodeId(12))));
        assert_eq!(node("/a/b"), Ok(p(&["a", "b"])));
        assert_eq!(node("a/b"), Ok(p(&["a", "b"])));
        assert_eq!(node("/"), Ok(p(&[])));
        assert_eq!(node("@user/decisions"), Ok(p(&["@user", "decisions"])));
        // A key whose steps are names decodes as a relative path (D.6.3): resolution fails later.
        assert_eq!(node("chat:C1/t2/m3"), Ok(p(&["chat:C1", "t2", "m3"])));
        assert_eq!(node("e400"), Ok(p(&["e400"])));
        for bad in [
            "",
            "#012",
            "#12/x",
            "a//b",
            "a/b/",
            "https://x.example/a",
            "/a/./b",
            "#",
        ] {
            assert!(node(bad).is_err(), "{bad:?}");
        }
        assert!(place("#12").unwrap_err().contains("a path only"));
        assert_eq!(place("/a"), Ok(Path(vec![Name::new("a").unwrap()])));
    }

    #[test]
    fn versions() {
        assert_eq!(node_at("#12@4"), Ok((NodeRef::Id(NodeId(12)), Version(4))));
        assert_eq!(node_at("/a/b@4"), Ok((p(&["a", "b"]), Version(4))));
        assert_eq!(node_at("/@user@3"), Ok((p(&["@user"]), Version(3))));
        for bad in ["#12", "/@user", "#12@", "#12@04", "#12@x", "@4"] {
            assert!(node_at(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(dur_text("50s"), Ok(Dur(50_000)));
        assert_eq!(dur_text("15m"), Ok(Dur(900_000)));
        assert_eq!(dur_text("2h"), Ok(Dur(7_200_000)));
        assert_eq!(dur_text("7d"), Ok(Dur(604_800_000)));
        assert_eq!(dur_text("0s"), Ok(Dur(0)));
        for bad in [
            "",
            "m",
            "15",
            "1h05m",
            "-1s",
            "1.5h",
            "15 m",
            "15M",
            "99999999999999999999d",
            // A multi-byte last character panicked at a byte inside it (review E2-bugs-1).
            "é",
            "1é",
            "Ω",
            "１２",
            "１m",
            "5ｍ",
        ] {
            assert!(dur_text(bad).is_err(), "{bad:?}");
        }
        for bad in ["in 1é", "in é", "in Ω"] {
            assert!(when(bad).is_err(), "{bad:?}");
        }
        assert_eq!(when("in 10m"), Ok(When::In(Dur(600_000))));
        assert_eq!(
            when("2026-10-01T07:00:00Z"),
            Ok(When::At(Time(1_790_838_000_000)))
        );
        assert_eq!(
            when("2026-10-01T09:00:00+02:00"),
            Ok(When::At(Time(1_790_838_000_000)))
        );
        assert_eq!(
            when("2026-10-01T07:00:00.250Z"),
            Ok(When::At(Time(1_790_838_000_250)))
        );
        for bad in [
            "",
            "tomorrow",
            "in",
            "in ten",
            "1969-12-31T23:59:59Z",
            "2026-10-01T07:00:00",
        ] {
            assert!(when(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn addressees() {
        assert_eq!(addressee("user"), Ok(AddresseeArg::User));
        assert_eq!(
            addressee("principal:mail:Bob@X.example"),
            Ok(AddresseeArg::Principal(
                PrincipalKey::new("mail:bob@x.example").unwrap()
            ))
        );
        assert_eq!(
            addressee("session:s-4"),
            Ok(AddresseeArg::Session(SessionId(4)))
        );
        // 1.7: a bare session id is unambiguous.
        assert_eq!(addressee("s-4"), Ok(AddresseeArg::Session(SessionId(4))));
        assert_eq!(agent("s-11"), Ok(AgentAddressee::Session(SessionId(11))));
        assert_eq!(
            addressee("holder:/a/b"),
            Ok(AddresseeArg::Holder(p(&["a", "b"])))
        );
        assert_eq!(
            addressee("holders:/a/b/**"),
            Ok(AddresseeArg::Holders(p(&["a", "b"])))
        );
        assert_eq!(addressee("holders:/**"), Ok(AddresseeArg::Holders(p(&[]))));
        assert_eq!(
            addressee("holders:#12/**"),
            Ok(AddresseeArg::Holders(NodeRef::Id(NodeId(12))))
        );
        for later in ["class:coding-agent", "cap:rust", "everyone"] {
            assert!(addressee(later).unwrap_err().contains("later release"));
            assert!(agent(later).unwrap_err().contains("later release"));
        }
        for bad in ["holders:/a/b", "session:4", "principal:user:e1", "bob", ""] {
            assert!(addressee(bad).is_err(), "{bad:?}");
        }
        assert_eq!(ask_human("user"), Ok(None));
        assert!(ask_human("principal:slack:t01/u02").unwrap().is_some());
        assert!(ask_human("session:s-1").is_err());
    }

    #[test]
    fn basis_references() {
        assert_eq!(
            basis_text("#12@4"),
            Ok(BasisArg::Node {
                at: NodeRef::Id(NodeId(12)),
                version: Some(Version(4))
            })
        );
        assert_eq!(
            basis_text("/a/b@4"),
            Ok(BasisArg::Node {
                at: p(&["a", "b"]),
                version: Some(Version(4))
            })
        );
        assert_eq!(
            basis_text("#12"),
            Ok(BasisArg::Node {
                at: NodeRef::Id(NodeId(12)),
                version: None
            })
        );
        assert_eq!(basis_text("e57"), Ok(BasisArg::Entry(Eid(57))));
        assert_eq!(basis_text("entry:e57"), Ok(BasisArg::Entry(Eid(57))));
        // 1.5 (§15, R3): a bare path is a node, absolute or relative, as in every node argument.
        for path in ["/a/b", "a/b"] {
            assert_eq!(
                basis_text(path),
                Ok(BasisArg::Node {
                    at: p(&["a", "b"]),
                    version: None
                }),
                "{path:?}"
            );
        }
        assert_eq!(
            basis_text("/"),
            Ok(BasisArg::Node {
                at: p(&[]),
                version: None
            })
        );
        // `e57` is an entry; a node named e57 is written `/e57`.
        assert_eq!(
            basis_text("/e57"),
            Ok(BasisArg::Node {
                at: p(&["e57"]),
                version: None
            })
        );
        // Written as a reference, but not one: refused, with the forms.
        for bad in ["entry:57", "#12@", "#x", "", "  ", "a//b", "/a/./b", "a/b/"] {
            let e = basis_text(bad).unwrap_err();
            assert!(e.contains("#12 or a path (a node)"), "{bad:?}: {e}");
        }
        // 1.6 (§15, l1, l2): any other string is something outside the forum, checked now.
        for outside in [
            "Platform reported diff restricted to tests/test_checkout.py, 100 focused repetitions",
            "Platform git show a1b2c3d and persistent runner cache configuration",
            "https://remark42.com/docs/",
            "arxiv:2101.00001",
            "lean:Mathlib/Order/Basic",
            "build 812 is green",
            "kb",
            "e57x",
        ] {
            assert_eq!(
                basis_text(outside),
                Ok(BasisArg::External {
                    what: Text(outside.to_string()),
                    checked_at: When::In(Dur(0)),
                    recheck: None
                }),
                "{outside:?}"
            );
        }
        assert_eq!(basis_text(" e57 "), Ok(BasisArg::Entry(Eid(57))), "trimmed");
    }

    /// 1.6 (§15, l2, l6): `checked_at` takes `now`, a date (00:00 UTC), and a time.
    #[test]
    fn checked_at_takes_now_and_a_date() {
        assert_eq!(checked_at("now"), Ok(When::In(Dur(0))));
        assert_eq!(checked_at(" NOW "), Ok(When::In(Dur(0))));
        assert_eq!(
            checked_at("2026-10-01"),
            Ok(When::At(Time(1_790_812_800_000)))
        );
        assert_eq!(
            checked_at("2026-10-01T07:00:00Z"),
            Ok(When::At(Time(1_790_838_000_000)))
        );
        assert_eq!(checked_at("in 10m"), Ok(When::In(Dur(600_000))));
        for bad in ["yesterday", "2026-13-01", "2026-1-01", "1969-12-31", ""] {
            let e = checked_at(bad).unwrap_err();
            assert!(e.contains(&format!("{bad:?}")), "{bad:?}: {e}");
        }
        assert!(checked_at("soon").unwrap_err().ends_with(
            "write now, a date such as 2026-10-04, or an RFC 3339 time such as \
                            2026-10-04T09:30:00Z"
        ));
    }

    /// 1.5 (R3, D.14.7): a refused principal gets its own hint, `user:` reserved among them.
    #[test]
    fn a_refused_principal_says_when_to_pass_one() {
        for raw in ["user:alice", "USER:x", " user : y"] {
            let e = principal_arg(raw).unwrap_err();
            assert!(e.contains("user: is reserved"), "{raw:?}: {e}");
            assert!(
                e.ends_with(
                    "pass the key your client's configuration names the human by, as \
                     <scheme>:<value> such as mail:alice@example.org, or user if it names none"
                ),
                "{e}"
            );
        }
        let e = principal_arg("alice").unwrap_err();
        assert!(
            e.starts_with("\"alice\" is not a principal: the key has no scheme"),
            "{e}"
        );
        assert!(e.contains("or user if it names none"), "{e}");
        assert_eq!(
            principal_arg("mail:Alice@Example.org").unwrap().as_str(),
            "mail:alice@example.org"
        );
        assert_eq!(principal_arg("user").unwrap().as_str(), "user");
    }

    #[test]
    fn keys_and_principals_are_normalised() {
        let k = key("CHAT:General/T1").unwrap();
        assert_eq!(k.key.as_str(), "chat:general/T1");
        assert_eq!(k.nv, NormVer(forum_keys::VERSION));
        assert!(key("repeat:12:20261002T0500Z").is_err());
        assert!(key("no scheme").is_err());
        assert_eq!(
            principal(" mail:<Bob@X.Example> ").unwrap().as_str(),
            "mail:bob@x.example"
        );
        assert!(principal("user:e12").is_err());
        assert!(principal("mail:Bob <bob@x>").is_err());
    }

    proptest::proptest! {
        // A failure is kept as a unit test above, not in a regressions file outside `src/`.
        #![proptest_config(proptest::prelude::ProptestConfig {
            cases: 2_000,
            failure_persistence: None,
            ..proptest::prelude::ProptestConfig::default()
        })]

        /// Every parser of a value is total: it answers any text, multi-byte characters
        /// included, with a value or a refusal, and never panics (review E2-bugs-1).
        #[test]
        fn the_value_parsers_never_panic(
            raw in "\\PC{0,12}",
            pre in proptest::sample::select(vec![
                "", "in ", "#", "/", "e", "entry:", "holder:", "holders:", "session:",
                "principal:", "@", "1", "12@",
            ]),
        ) {
            let s = format!("{pre}{raw}");
            let _ = node(&s);
            let _ = path(&s);
            let _ = place(&s);
            let _ = node_at(&s);
            let _ = canonical_decimal(&s);
            let _ = key(&s);
            let _ = principal(&s);
            let _ = dur_text(&s);
            let _ = when(&s);
            let _ = outcome(&s);
            let _ = addressee(&s);
            let _ = agent(&s);
            let _ = ask_human(&s);
            let _ = basis_text(&s);
            let _ = checked_at(&s);
        }
    }

    #[test]
    fn outcomes() {
        assert_eq!(outcome("done"), Ok(Outcome::Done));
        assert_eq!(outcome("missed"), Ok(Outcome::Missed));
        assert_eq!(
            outcome("wont-fix"),
            Ok(Outcome::Other(Label::new("wont-fix").unwrap()))
        );
        assert!(outcome("Won't fix").is_err());
        // 1.9 (§5): the usual words for done are done; refuted stays a label of its own.
        for s in DONE_SYNONYMS {
            assert_eq!(outcome(s), Ok(Outcome::Done), "{s}");
        }
        assert_eq!(
            outcome("refuted"),
            Ok(Outcome::Other(Label::new("refuted").unwrap()))
        );
        assert_eq!(
            outcome("Implemented and PR opened"),
            Err(
                "\"Implemented and PR opened\" is not an outcome: one label, usually done, \
                 failed, dropped, superseded, or refuted ([a-z0-9][a-z0-9_.:-]{0,63}); write the \
                 sentence in summary"
                    .to_string()
            )
        );
    }
}
