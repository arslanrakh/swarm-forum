//! The formats of every rendered value (design D.11.7): times as RFC 3339 UTC to the minute with
//! the seconds written, durations as `1h05m`, ids as `#12`, `e345`, `s-7`, tokens as `12.3`,
//! lists' `shown/total` or `≥k`, texts cut at a UTF-8 boundary with "…", and the names §20 and
//! §21 give enums.

use forum_core::query::path_of;
use forum_core::{
    BoardState, Dur, EntryType, Flag, FlagSet, LinkRel, NodeId, Outcome, PrincipalId,
    ReleaseReason, SaidKind, State, Time, ToUserKind, Total,
};

use super::json::Json;
use crate::decode::spec;

/// The ellipsis that marks a cut text.
pub const ELLIPSIS: &str = "…";

/// A path longer than this is shown as its id (§20.9, D.11.7).
pub const PATH_SHOWN_MAX: usize = 64;

const MS_PER_MINUTE: u64 = 60_000;
const MS_PER_DAY: u64 = 86_400_000;

/// The civil date and the minute of the day of `t`, in UTC.
fn civil(t: Time) -> (i64, u8, u8, u64) {
    let days = t.0 / MS_PER_DAY;
    let minute = (t.0 % MS_PER_DAY) / MS_PER_MINUTE;
    let (y, m, d) = forum_cron::civil_from_days(i64::try_from(days).unwrap_or(i64::MAX));
    (y, m, d, minute)
}

/// A time as RFC 3339 UTC to the minute, with the seconds written (`2026-10-01T07:00:00Z`): an
/// agent copies a shown time into `due`, `until`, or `recheck_after`, whose decoder takes that
/// form (1.4, D.11.7).
#[must_use]
pub fn time(t: Time) -> String {
    let (y, m, d, minute) = civil(t);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:00Z",
        minute / 60,
        minute % 60
    )
}

/// A date, `2027-03-02` (UTC).
#[must_use]
pub fn date(t: Time) -> String {
    let (y, m, d, _) = civil(t);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The time of day, `07:15` (UTC): a pause's end on the status line and in an empty `next`.
#[must_use]
pub fn hhmm(t: Time) -> String {
    let (_, _, _, minute) = civil(t);
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// A duration as `1h05m`, `15m`, `15m30s`, or `45s`.
#[must_use]
pub fn dur(d: Dur) -> String {
    let s = d.0 / 1_000;
    let (h, m, sec) = (s / 3_600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 && sec > 0 {
        format!("{m}m{sec:02}s")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{sec}s")
    }
}

/// Minutes and seconds, `12:05` (the minutes may pass 59): a hold's time left.
#[must_use]
pub fn mmss(d: Dur) -> String {
    let s = d.0 / 1_000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// A duration as the decoder of D.11.5 takes it: one unit (`50s`, `15m`, `2h`, `7d`) when one
/// divides it, else an integer of milliseconds. The form of a next call's `args`.
#[must_use]
pub fn dur_arg(d: Dur) -> Json {
    for (unit, ms) in [
        ("d", MS_PER_DAY),
        ("h", 3_600_000),
        ("m", MS_PER_MINUTE),
        ("s", 1_000),
    ] {
        if d.0 > 0 && d.0.is_multiple_of(ms) {
            return Json::S(format!("{}{unit}", d.0 / ms));
        }
    }
    Json::U(d.0)
}

/// `s` cut to at most `max` bytes at a UTF-8 boundary, with "…" when it was cut (the ellipsis
/// counts against `max`).
#[must_use]
pub fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let room = max.saturating_sub(ELLIPSIS.len());
    let mut end = room.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s.get(..end).unwrap_or("").to_string();
    out.push_str(ELLIPSIS);
    out
}

/// A total: `12`, or `≥20`.
#[must_use]
pub fn total(t: Total) -> String {
    match t {
        Total::Exact(n) => n.to_string(),
        Total::AtLeast(n) => format!("≥{n}"),
    }
}

/// `shown/total` (§18).
#[must_use]
pub fn shown(shown: usize, t: Total) -> String {
    format!("{shown}/{}", total(t))
}

/// Whether a list of `shown` items of `t` was cut short (1.7, §18): its total is a lower bound,
/// or exceeds what it shows.
#[must_use]
pub fn is_cut(shown: usize, t: Total) -> bool {
    match t {
        Total::Exact(k) => u64::try_from(shown).unwrap_or(u64::MAX) < u64::from(k),
        Total::AtLeast(_) => true,
    }
}

/// `shown/total` of a list cut short, and None for a list shown whole, which carries no counts
/// (1.7, §18, §20.1).
#[must_use]
pub fn shown_if_cut(n: usize, t: Total) -> Option<Json> {
    is_cut(n, t).then(|| Json::S(shown(n, t)))
}

/// An integer with thousands separators: `1,204`.
#[must_use]
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    let len = digits.len();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A node's live path: `/` for ROOT, `/a/b` otherwise.
#[must_use]
pub fn path(s: &State, n: NodeId) -> String {
    let p = path_of(s, n);
    if p.0.is_empty() {
        return "/".to_string();
    }
    let mut out = String::new();
    for name in &p.0 {
        out.push('/');
        out.push_str(name.as_str());
    }
    out
}

/// A node as an argument or a short reference: its path, or its id when the path is longer than
/// 64 bytes (§20.9, D.11.7) or the node has none.
#[must_use]
pub fn node_ref(s: &State, n: NodeId) -> String {
    let p = path(s, n);
    if p.len() > PATH_SHOWN_MAX || (p == "/" && n != forum_core::ROOT) {
        n.render()
    } else {
        p
    }
}

/// A principal by its key (CS-1); `p<n>` for one the state does not know.
#[must_use]
pub fn principal(s: &State, p: PrincipalId) -> String {
    forum_core::query::principal_key(s, p)
        .map_or_else(|| format!("p{}", p.0), |k| k.as_str().to_string())
}

/// §5's outcome.
#[must_use]
pub fn outcome(o: &Outcome) -> String {
    match o {
        Outcome::Done => "done".into(),
        Outcome::Failed => "failed".into(),
        Outcome::Dropped => "dropped".into(),
        Outcome::Superseded => "superseded".into(),
        Outcome::Missed => "missed".into(),
        Outcome::Other(l) => l.as_str().to_string(),
    }
}

/// §14's flag.
#[must_use]
pub fn flag(f: Flag) -> &'static str {
    spec::FLAGS
        .get(usize::from(f.index()))
        .copied()
        .unwrap_or("flag")
}

/// The flags of a set, in table order.
#[must_use]
pub fn flags(f: FlagSet) -> Json {
    Json::strs(f.iter().map(flag))
}

/// §6's entry type.
#[must_use]
pub fn entry_type(t: EntryType) -> &'static str {
    crate::store::shell_tables::entry_type_name(t)
}

/// §16's relation.
#[must_use]
pub fn link_rel(r: LinkRel) -> &'static str {
    match r {
        LinkRel::Continues => "continues",
        LinkRel::DerivedFrom => "derived_from",
        LinkRel::ConflictsWith => "conflicts_with",
        LinkRel::Duplicate => "duplicate",
        LinkRel::SupersededBy => "superseded_by",
    }
}

/// §9.2's `said` kind.
#[must_use]
pub fn said_kind(k: SaidKind) -> &'static str {
    match k {
        SaidKind::Ack => "ack",
        SaidKind::Reply => "reply",
        SaidKind::Result => "result",
        SaidKind::Correction => "correction",
        SaidKind::Question => "question",
    }
}

/// §8.1's report kind.
#[must_use]
pub fn to_user_kind(k: ToUserKind) -> &'static str {
    match k {
        ToUserKind::Ack => "ack",
        ToUserKind::Question => "question",
        ToUserKind::Result => "result",
    }
}

/// §10.3's release reason.
#[must_use]
pub fn release_reason(r: ReleaseReason) -> &'static str {
    match r {
        ReleaseReason::Handoff => "handoff",
        ReleaseReason::Blocked => "blocked",
        ReleaseReason::Cannot => "cannot",
    }
}

/// §8.3's board state.
#[must_use]
pub fn board_state(b: BoardState) -> &'static str {
    match b {
        BoardState::Unrouted => "unrouted",
        BoardState::Answered => "answered",
        BoardState::ClosedUnanswered => "closed_unanswered",
        BoardState::Decided => "decided",
        BoardState::Waiting => "waiting",
        BoardState::InProgress => "in_progress",
    }
}

/// A total as JSON: a number, or the string `≥k`.
#[must_use]
pub fn total_json(t: Total) -> Json {
    match t {
        Total::Exact(n) => Json::from(n),
        Total::AtLeast(n) => Json::S(format!("≥{n}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_render_to_the_minute_with_seconds() {
        // 2026-10-01T00:00:00Z.
        let t = Time(1_790_812_800_000);
        assert_eq!(time(t), "2026-10-01T00:00:00Z");
        assert_eq!(
            time(Time(t.0 + 7 * 3_600_000 + 15 * 60_000 + 59_999)),
            "2026-10-01T07:15:00Z"
        );
        assert_eq!(date(t), "2026-10-01");
        assert_eq!(hhmm(Time(t.0 + 23 * 3_600_000 + 59 * 60_000)), "23:59");
        assert_eq!(time(Time(0)), "1970-01-01T00:00:00Z");
        // The decoder reads the rendered form back to the same minute.
        let back = crate::decode::values::when(&time(t)).unwrap();
        assert_eq!(back, forum_core::When::At(t));
    }

    #[test]
    fn durations() {
        assert_eq!(dur(Dur(3_900_000)), "1h05m");
        assert_eq!(dur(Dur(900_000)), "15m");
        assert_eq!(dur(Dur(930_000)), "15m30s");
        assert_eq!(dur(Dur(45_000)), "45s");
        assert_eq!(dur(Dur(0)), "0s");
        assert_eq!(mmss(Dur(725_000)), "12:05");
        assert_eq!(mmss(Dur(2 * 3_600_000)), "120:00");
        assert_eq!(dur_arg(Dur(50_000)), Json::s("50s"));
        assert_eq!(dur_arg(Dur(1_000)), Json::s("1s"));
        assert_eq!(dur_arg(Dur(7_200_000)), Json::s("2h"));
        assert_eq!(dur_arg(Dur(1_500)), Json::U(1_500));
        assert_eq!(dur_arg(Dur(0)), Json::U(0));
        for d in [50_000, 1_000, 7_200_000, 86_400_000, 1_500] {
            let arg = dur_arg(Dur(d));
            let back = match &arg {
                Json::S(s) => crate::decode::values::dur_text(s).unwrap(),
                Json::U(n) => Dur(*n),
                other => panic!("{other:?}"),
            };
            assert_eq!(back, Dur(d));
        }
    }

    #[test]
    fn cuts_fall_on_utf8_boundaries() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcdef", 5), "ab…");
        // "é" is two bytes: a cut never splits it.
        let s = "ééééé";
        for max in 0..12 {
            let c = cut(s, max);
            assert!(c.len() <= max.max(3), "{max}: {c}");
            assert!(c.is_char_boundary(c.len()));
        }
        assert_eq!(thousands(1_204), "1,204");
        assert_eq!(thousands(12), "12");
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(total(Total::AtLeast(20)), "≥20");
        assert_eq!(shown(3, Total::Exact(7)), "3/7");
    }

    #[test]
    fn names_match_the_decoders_tables() {
        for (i, f) in Flag::ALL.iter().enumerate() {
            assert_eq!(flag(*f), spec::FLAGS[i]);
        }
        for (i, r) in [
            LinkRel::Continues,
            LinkRel::DerivedFrom,
            LinkRel::ConflictsWith,
            LinkRel::Duplicate,
            LinkRel::SupersededBy,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(link_rel(r), spec::LINK_RELS[i]);
        }
        for o in [
            "done",
            "failed",
            "dropped",
            "superseded",
            "missed",
            "partly",
        ] {
            assert_eq!(outcome(&crate::decode::values::outcome(o).unwrap()), o);
        }
    }
}
