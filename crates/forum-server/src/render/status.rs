//! The status line (§20.1, design D.11.7): the first line of every response to a session.
//!
//! `[swarm] running|PAUSED(by <class>/s-<n> until <hh:mm>: <reason>) · now <time> · session s-<n>
//! (<class>) · holds <k> (<path>, <mm:ss> left, checkpoint by <time>)… · inbox <m>[ ·
//! user_decisions <p>][ · health <level>[: <reason>]]` (1.7).
//!
//! `now` is the call's `now` (T1), and each hold's `checkpoint by` is `query::checkpoint_by`, the
//! time rule 3 of the next calls suggests the checkpoint at (1.4, R8): both from the core's
//! `StatusLine` (D.8.25).
//!
//! 1.7 (the lab's second round): `session s-<n> (<class>)` names the session as the argument takes
//! it, where `you=<class>/s-<n>` did not read as one; health is left out while green; and the first
//! reason is shown only when the session has not been shown it at that level ([`HealthShown`],
//! the shell's memory of §20.1), since a standing red reason was 18 % of a run's status lines.

use forum_core::health::{Health, HealthReason, Level, MoverGap, UnreachableBy};
use forum_core::query::{PauseView, status_line};
use forum_core::{Cand, Dur, Label, NodeId, SessionId, State, Time};

use super::format::{cut, dur, dur_arg, flag, hhmm, mmss, node_ref, time};
use super::json::Json;
use super::seen::HealthShown;
use super::source::Source;

/// A pause's reason is cut to this many bytes on the status line (D.11.7).
pub const PAUSE_REASON_MAX: usize = 60;

/// A pause's reason is cut to this many bytes in the `Paused` reason of a status line's health
/// (1.12, §19, §20.1).
pub const PAUSED_ITEM_REASON_MAX: usize = 80;

/// The line that follows the status line when a silent session calls again (§3, §20.1).
pub const STALE_SESSION: &str = "STALE SESSION — re-read before writing";

/// A health level as §17 names it.
#[must_use]
pub fn level(l: Level) -> &'static str {
    match l {
        Level::Green => "green",
        Level::Yellow => "yellow",
        Level::Red => "red",
    }
}

/// `by <class>/s-<n> until <hh:mm>: <reason>`, the reason cut to 60 bytes.
fn pause_text(v: &PauseView, src: &dyn Source) -> String {
    let reason = src.pause_reason(v.scope).map(|t| t.0).unwrap_or_default();
    format!(
        "by {}/{} until {}: {}",
        v.class,
        v.by.render(),
        hhmm(v.until),
        cut(&reason, PAUSE_REASON_MAX)
    )
}

/// A node's needs, as an unreachable item names them, cut to this many bytes.
pub const NEEDS_SHOWN_MAX: usize = 60;

/// `, and <k> more` after a reason's first item, when it counts `count` (1.6).
fn and_more(count: u32) -> String {
    match count.saturating_sub(1) {
        0 => String::new(),
        k => format!(", and {k} more"),
    }
}

/// A duration as the decoder reads it, `7d`, or as `dur` writes it when no unit divides it.
fn span(d: Dur) -> String {
    match dur_arg(d) {
        Json::S(x) => x,
        _ => dur(d),
    }
}

/// The needs of node `n`, in label order; none for a node the state no longer has.
#[must_use]
pub fn needs_of(s: &State, n: NodeId) -> Vec<&str> {
    s.nodes
        .get(&n)
        .map(|x| x.needs.iter().map(Label::as_str).collect())
        .unwrap_or_default()
}

/// An unreachable candidate and the value that keeps every session from it (1.6):
/// `/support/tag-cleanup needs caps no session has (support)`, the needs cut to
/// `NEEDS_SHOWN_MAX` bytes; `/a: no session seen in 7d`, for a node with no needs, which only
/// the absence of every session keeps from work; or `/a est 60 min exceeds every session's
/// horizon`; 1.12: `/a est 60 min exceeds your horizon (45) and every other session's`, to a
/// session whose caps meet the needs.
fn unreachable_item(s: &State, n: NodeId, by: UnreachableBy, window: &str) -> String {
    match by {
        UnreachableBy::Needs => match needs_of(s, n).as_slice() {
            [] => format!("{}: no session seen in {window}", node_ref(s, n)),
            needs => format!(
                "{} needs caps no session has ({})",
                node_ref(s, n),
                cut(&needs.join(", "), NEEDS_SHOWN_MAX)
            ),
        },
        UnreachableBy::Horizon => match s.nodes.get(&n).and_then(|x| x.est) {
            Some(est) => format!(
                "{} est {est} min exceeds every session's horizon",
                node_ref(s, n)
            ),
            None => format!("{} exceeds every session's horizon", node_ref(s, n)),
        },
        // 1.12 (§17): computed for the session shown it, whose caps meet the needs.
        UnreachableBy::Yours { horizon } => match s.nodes.get(&n).and_then(|x| x.est) {
            Some(est) => format!(
                "{} est {est} min exceeds your horizon ({horizon}) and every other session's",
                node_ref(s, n)
            ),
            None => format!(
                "{} exceeds your horizon ({horizon}) and every other session's",
                node_ref(s, n)
            ),
        },
    }
}

/// A `NoMover` item and what holds it (1.6): `/a waits for /b, which nothing moves`.
fn gap_item(s: &State, g: &MoverGap) -> String {
    match g {
        MoverGap::NodeWait { node, target } => format!(
            "{} waits for {}, which nothing moves",
            node_ref(s, *node),
            node_ref(s, *target)
        ),
        MoverGap::Dependency { node, target } => format!(
            "{} depends on {}, which nothing moves",
            node_ref(s, *node),
            node_ref(s, *target)
        ),
        MoverGap::FlaggedUnreachable { node, flags } => {
            let first = flags.iter().next().map_or("flagged", flag);
            format!(
                "{} is flagged {first}, and nothing can take it",
                node_ref(s, *node)
            )
        }
    }
}

/// A candidate for `next` as an item names it: its node, or a user entry still to route.
#[must_use]
pub fn cand_words(s: &State, c: Cand) -> String {
    match c {
        Cand::Node(n) | Cand::Status(n) => node_ref(s, n),
        Cand::Route(e) => format!("user entry {} to route", e.render()),
    }
}

/// The first reason of a health that is not green, in words that name its first item and the
/// value that holds it there, and how many more it counts (CS-9; 1.6, the lab's first round,
/// where `unreachable 9 (horizon)` and `overloaded` sent agents to `control health` for nothing):
/// `paused: / by coding-agent/s-12 until 14:00, excludes 4`, `no mover: /a waits for /b, which
/// nothing moves, and 2 more`, `unreachable: /support/tag-cleanup needs caps no session has
/// (support)`, `flags piling up: 95 flagged nodes not waiting, up from 40 in 7d`, `overloaded: 900
/// min of work in 7d, 300 min done; oldest waiting /tidepool/comments-provider`, `write surge:
/// coding-agent made 5000 writes in the last hour, against a mean of 10`, `underused: 60 % of next
/// calls found nothing, and no work waits`.
#[must_use]
pub fn reason_words(s: &State, r: &HealthReason) -> String {
    let window = span(s.params.capacity_window);
    match r {
        HealthReason::Paused { count, top, .. } => paused_words(s, *count, top, None),
        HealthReason::NoMover { count, top, .. } => match top.first() {
            Some(g) => format!("no mover: {}{}", gap_item(s, g), and_more(*count)),
            None => format!("no mover: {count}"),
        },
        HealthReason::Unreachable { count, top, .. } => match top.first() {
            Some((n, by)) => format!(
                "unreachable: {}{}",
                unreachable_item(s, *n, *by, &window),
                and_more(*count)
            ),
            None => format!("unreachable: {count}"),
        },
        HealthReason::FlagsPilingUp { start, now } => {
            format!("flags piling up: {now} flagged nodes not waiting, up from {start} in {window}")
        }
        HealthReason::Overloaded {
            demand_min,
            supply_min,
            oldest,
            ..
        } => {
            let oldest = oldest
                .map(|c| format!("; oldest waiting {}", cand_words(s, c)))
                .unwrap_or_default();
            format!(
                "overloaded: {demand_min} min of work in {window}, {supply_min} min done{oldest}"
            )
        }
        HealthReason::WriteSurge {
            class,
            last_hour,
            baseline,
        } => format!(
            "write surge: {class} made {last_hour} writes in the last hour, against a mean of \
             {baseline}"
        ),
        HealthReason::Underused { idle_permille } => format!(
            "underused: {} % of next calls found nothing, and no work waits",
            idle_permille / 10
        ),
    }
}

/// The health that session `sess` is shown (1.11, §17): the writer's cache `h` with the session's
/// caps and horizon counted (`health::counting`), which the cache may predate; `h` itself with no
/// such session. In the lab's fourth to sixth rounds, `hello` told the only sessions with a11y,
/// and a Lean prover, that no session had their caps.
#[must_use]
pub fn health_of(s: &State, sess: Option<SessionId>, h: &Health) -> Health {
    match sess.and_then(|x| s.sessions.get(&x)) {
        Some(x) => forum_core::health::counting(h, s, &x.caps, x.horizon),
        None => h.clone(),
    }
}

/// `health <level>[: <reason>]`, from the writer's cache (D.11.2): the whole segment, with the
/// first reason of a level that is not green, as `control health` and the first status line of a
/// session show it.
#[must_use]
pub fn health_segment(s: &State, h: &Health) -> String {
    match (h.level, h.reasons.first()) {
        (Level::Green, _) | (_, None) => format!("health {}", level(h.level)),
        (l, Some(r)) => format!("health {}: {}", level(l), reason_words(s, r)),
    }
}

/// The identity of a reason (1.7, §20.1): its kind and its first item, without the counts that
/// move while it stands (`and 2 more`, the minutes of `Overloaded`), so that the status line shows
/// the reason again when it changes, not when a count does.
#[must_use]
pub fn reason_key(s: &State, r: &HealthReason) -> String {
    let window = span(s.params.capacity_window);
    match r {
        HealthReason::Paused { top, .. } => format!(
            "paused {}",
            top.first()
                .map(|p| node_ref(s, p.pause.scope))
                .unwrap_or_default()
        ),
        HealthReason::NoMover { top, .. } => format!(
            "no mover {}",
            top.first().map(|g| gap_item(s, g)).unwrap_or_default()
        ),
        HealthReason::Unreachable { top, .. } => format!(
            "unreachable {}",
            top.first()
                .map(|(n, by)| unreachable_item(s, *n, *by, &window))
                .unwrap_or_default()
        ),
        HealthReason::FlagsPilingUp { .. } => "flags piling up".into(),
        HealthReason::Overloaded { oldest, .. } => format!(
            "overloaded {}",
            oldest.map(|c| cand_words(s, c)).unwrap_or_default()
        ),
        HealthReason::WriteSurge { class, .. } => format!("write surge {class}"),
        HealthReason::Underused { .. } => "underused".into(),
    }
}

/// The words of a `Paused` reason: its first pause, with `reason` quoted after its end when given
/// (1.12), what it excludes, and how many more pauses it counts.
fn paused_words(
    s: &State,
    count: u32,
    top: &[forum_core::health::PauseExclusion],
    reason: Option<&str>,
) -> String {
    match top.first() {
        Some(p) => format!(
            "paused: {} by {}/{} until {}{}, excludes {}{}",
            node_ref(s, p.pause.scope),
            p.pause.class,
            p.pause.by.render(),
            hhmm(p.pause.until),
            reason.map_or_else(String::new, |t| format!(
                " (\"{}\")",
                cut(t, PAUSED_ITEM_REASON_MAX)
            )),
            p.excluded,
            and_more(count)
        ),
        None => format!("paused: {count} pauses"),
    }
}

/// The words of a reason as a status line shows them (1.12, §19, §20.1): [`reason_words`], and,
/// for `Paused`, its first pause's reason, quoted and cut to [`PAUSED_ITEM_REASON_MAX`] bytes,
/// after its end: `paused: /payments by coding-agent/s-25 until 07:36 ("Freeze for INC-311: …"),
/// excludes 2`. In the lab's seventh round (it6 l6), an incident lead never saw why /payments was
/// paused, and asked its silent author.
#[must_use]
pub fn reason_words_shown(s: &State, r: &HealthReason, src: &dyn Source) -> String {
    match r {
        HealthReason::Paused { count, top, .. } => {
            let reason = top
                .first()
                .and_then(|p| src.pause_reason(p.pause.scope))
                .map(|t| t.0)
                .filter(|t| !t.is_empty());
            paused_words(s, *count, top, reason.as_deref())
        }
        _ => reason_words(s, r),
    }
}

/// The health segment of a session's status line, given what its last status line showed (1.7,
/// §20.1): none while green; the level and its first reason when the session has not been shown
/// that reason at that level (`last` is None, the level differs, or the reason's kind or first item
/// changed); the level alone otherwise. With what the line now shows. 1.12: the first reason in
/// [`reason_words_shown`]'s words, a pause's with its reason.
#[must_use]
pub fn health_for(
    s: &State,
    h: &Health,
    last: Option<&HealthShown>,
    src: &dyn Source,
) -> (Option<String>, HealthShown) {
    let first = h.reasons.first().filter(|_| h.level != Level::Green);
    let now = HealthShown {
        level: h.level,
        reason: first.map(|r| reason_key(s, r)),
    };
    let segment = match first {
        None if h.level == Level::Green => None,
        None => Some(format!("health {}", level(h.level))),
        Some(r) if last != Some(&now) => Some(format!(
            "health {}: {}",
            level(h.level),
            reason_words_shown(s, r, src)
        )),
        Some(_) => Some(format!("health {}", level(h.level))),
    };
    (segment, now)
}

/// The status line of session `sess` at `now`, rendered from the state as it stands, given the
/// health its last status line showed; with the health this one shows (1.7).
#[must_use]
pub fn line(
    s: &State,
    sess: SessionId,
    now: Time,
    health: &Health,
    src: &dyn Source,
    last: Option<&HealthShown>,
) -> (String, HealthShown) {
    let st = status_line(s, sess, now);
    let mut out = String::from("[swarm] ");
    match &st.root_pause {
        Some(p) => {
            out.push_str("PAUSED(");
            out.push_str(&pause_text(p, src));
            out.push(')');
        }
        None => out.push_str("running"),
    }
    out.push_str(" · now ");
    out.push_str(&time(st.now));
    out.push_str(&format!(" · session {} ({})", sess.render(), st.class));
    // The session's live holds in token order, each with its checkpoint's due time (1.4, R8).
    out.push_str(&format!(" · holds {}", st.holds.len()));
    for h in &st.holds {
        out.push_str(&format!(
            " ({}, {} left, checkpoint by {})",
            node_ref(s, h.node),
            mmss(h.lease_until.since(now)),
            time(h.checkpoint_by)
        ));
    }
    for p in &st.hold_pauses {
        out.push_str(&format!(
            " (paused: {} {})",
            node_ref(s, p.scope),
            pause_text(p, src)
        ));
    }
    out.push_str(&format!(" · inbox {}", st.inbox));
    if let Some(p) = st.user_decisions {
        out.push_str(&format!(" · user_decisions {p}"));
    }
    let (segment, shown) = health_for(s, health, last, src);
    if let Some(h) = segment {
        out.push_str(" · ");
        out.push_str(&h);
    }
    (out, shown)
}
