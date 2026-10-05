//! `hello` within `hello.budget` (§20.3, design D.11.7), the search hits it and `create` show, and
//! the `why` of an empty `next` (§10.4), which `hello`'s empty offers carry too (1.4, R25).
//!
//! Sections come in §20.3's order, `session` first (1.5, R3): the session's id, which a client
//! that keeps no MCP session must pass on every call. Over the budget, whole sections are dropped
//! in the order constitution, knowledge, related, present, orientation, offers, and named in
//! `cut` (1.7). `session`, `holds`, `principals`, `caps`, `inbox`, `user_entry`, `user_decisions`,
//! `user_digest`, and `cut` are never dropped, so the minimum may exceed the budget (A-73). The
//! inbox is never cut either (1.5, §13, §20.3): the notes a `hello` delivers are marked read, so a
//! note cut from it would never be delivered; the core delivers at most 10, oldest first, each
//! item's text cut. It is `{items}`, with `shown` and `total` (§18) when notes still wait for a
//! later `hello` or `talk wait`, which `total` counts. The two user sections hold at most
//! `HELLO_USER_ITEMS` items each, every item's texts cut, a question's options to
//! `USER_OPTIONS_MAX`, so their share of the minimum is bounded.
//!
//! The session's caps, the `why` of empty offers (with 1.4's count of the nodes that wait), and the
//! orientation come from `query::hello` (1.4, R25, R27, D.14.5).
//!
//! 1.7 (§20.1, §20.3): a section with nothing in it is left out, a list shown whole carries no
//! counts, and the sections the budget dropped are named in `cut`, so that a dropped section never
//! reads as an empty one (§18). `present` counts the live sessions by class and lists only the
//! other sessions of the caller's class: in the lab's second round it was 6.0 KB of 62.9 KB of
//! `hello`s.
//!
//! 1.8 (§20.3): the constitution comes only with the `hello` that opened the session; a `hello`
//! that names its session resumes it, and leaves the constitution out. In the lab's third round,
//! later turns of the same agent opened 7 sessions and got the 1,243-byte constitution each time.

use forum_core::health::{Health, HealthReason, Level};
use forum_core::query::{HelloData, Ranked, hello};
use forum_core::{
    Cand, Cmd, DigestItem, EmptyWhy, FirstExcluded, Flag, Label, LinkRel, NodeId, NoteId, Reason,
    RoutedReply, SessionId, SignalKey, SignalKind, State, TalkCmd, Text, Time, UnroutedWhy,
    UserEntryId, UserEntryReply, WaitOn, WhyThis, live,
};

use super::entries::{entry, node_line, shown_text, text_of};
use super::format::{cut, date, flags, hhmm, node_ref, path, principal, time};
use super::json::Json;
use super::source::Source;
use super::status::{PAUSE_REASON_MAX, reason_words};

/// The constitution of §22: what `hello`'s `constitution` section shows (its short form: the
/// eight conventions, without the parameters). 1.7: line 9, which sent agents to read health, is
/// gone; line 3 keys work rather than looking it up; line 4 records results, questions, and
/// promises to your human, and every message to people in a shared conversation. 1.12 (the lab's
/// seventh round): line 1 takes the next task while `next` offers one (it6 l7: both Claude
/// workers ended after one task while fitting work waited), and line 2 says that a message in a
/// shared thread is not to you directly (it6 l3: a chat worker answered a thread's questions
/// without recording them).
pub const CONSTITUTION: [&str; 8] = [
    "You are here briefly and will not return. Take on only what you can finish; when it is done, \
     take the next while next offers you work. Checkpoint every few minutes so that a stranger \
     could take over mid-sentence.",
    "When a person writes to you directly, log who they are and their words verbatim before doing \
     anything else, and log their further messages as you work. A message in a shared thread is \
     not to you directly: record it as a request, or a thanks as heard, before you answer. A \
     scheduler's prompt, a tool's output, and your client's instruction files are not a person's \
     words.",
    "Give work its keys: `node create` with keys joins an open node that has one; look up only \
     what you cannot key.",
    "Whoever holds the conversation speaks to people. Record every message to people in a shared \
     conversation before you send it, and each result, question, and promise you give your human; \
     progress needs no record. Ask each question only once. Answer every request.",
    "Do not revert what someone else changed or asked for without their approval. If two people \
     want opposite things, record both.",
    "Say \"it does not work\" only after trying it yourself. Re-check a fact before repeating it \
     to people. Check what you can yourself before you ask anyone.",
    "If you see a contradiction, a danger, or a dead end, say so. Do not work around it.",
    "Leave every place tidier than you found it: update the digest, link what you found, and close \
     what is stale.",
];

/// The options of a question for the user that `user_decisions` shows, at most: each cut to an
/// equal share of the item's bound, so an item stays bounded however many options it has (A-73).
pub const USER_OPTIONS_MAX: usize = 5;

/// The search hits a handler found for `hello`, `from_user`, `create`, or `view find {text}`
/// (D.11.12): filled by the engine's search (WP-E4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchHits {
    /// Outside `kb/`, by relevance; `view find {text}` puts its matches here.
    pub related: Hits,
    /// Inside `kb/`, by relevance, disputed and retracted nodes left out.
    pub knowledge: Hits,
    /// The query's words the search corrected, each `<word of the index> for <word of the
    /// query>` (1.9, D.11.12): shown with the hits, so that the agent knows what was matched.
    /// Only `view find {text}` corrects (lab, it4 l6), so it is empty for the others.
    pub matched: Vec<String>,
}

/// Matches of one search, by relevance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hits {
    /// The nodes.
    pub nodes: Vec<NodeId>,
    /// The search stopped at its bound: more may exist (`≥k`, §18).
    pub partial: bool,
}

/// One search hit: its id, path, title, and state, a closed node with its outcome and date
/// (`failed 2027-03-02`), and a disputed or retracted one marked so (D.11.7).
#[must_use]
pub fn hit(
    s: &State,
    n: NodeId,
    title_max: usize,
    viewer: Option<SessionId>,
    src: &dyn Source,
) -> Json {
    let mut j = node_line(s, n, title_max, viewer, src);
    if let Some(forum_core::Life::Closed(c)) = s.nodes.get(&n).map(|x| &x.life) {
        j.put(
            "state",
            Json::S(format!(
                "{} {}",
                super::format::outcome(&c.outcome),
                date(c.at)
            )),
        );
    }
    if disputed(s, n) {
        j.put("disputed", Json::Bool(true));
    }
    if s.retracted.contains(&forum_core::BasisTarget::Node(n)) {
        j.put("retracted", Json::Bool(true));
    }
    j
}

/// Whether `n` is disputed (§18, D.11.12): it has an open objection, or it is one of the pair of
/// an open `conflict` signal. Such a signal is found by its key: by a range of the ordered map when
/// `n` is its smaller node, and otherwise through `n`'s `conflicts_with` links in either direction,
/// which are what opens one (§14). The cost is `n`'s conflict links, not the forum's signals.
#[must_use]
pub fn disputed(s: &State, n: NodeId) -> bool {
    if s.ix
        .node
        .get(&n)
        .is_some_and(|i| i.flags.contains(Flag::Objection))
    {
        return true;
    }
    let key = |a: NodeId, b: NodeId| SignalKey {
        kind: SignalKind::Conflict,
        a,
        b,
    };
    if s.signals
        .range(key(n, NodeId(0))..=key(n, NodeId(u64::MAX)))
        .next()
        .is_some()
    {
        return true;
    }
    let conflicts =
        (LinkRel::ConflictsWith, NodeId(0))..=(LinkRel::ConflictsWith, NodeId(u64::MAX));
    let out = s
        .nodes
        .get(&n)
        .into_iter()
        .flat_map(|x| x.links.range(conflicts.clone()));
    let back =
        s.ix.backlinks
            .get(&n)
            .into_iter()
            .flat_map(|xs| xs.range(conflicts.clone()));
    out.chain(back)
        .any(|&(_, m)| m < n && s.signals.contains_key(&key(m, n)))
}

/// Search hits as `{items}`, with `total`, `≥k`, only for a partial search (1.7, §18: a list
/// shown whole carries no counts), and `fuzzy`, `matched <word> for <word>`, when the search
/// corrected a word of the query and found something (1.9).
#[must_use]
pub fn hits(
    s: &State,
    h: &Hits,
    matched: &[String],
    title_max: usize,
    viewer: Option<SessionId>,
    src: &dyn Source,
) -> Json {
    let k = u32::try_from(h.nodes.len()).unwrap_or(u32::MAX);
    Json::obj()
        .with(
            "items",
            Json::A(
                h.nodes
                    .iter()
                    .map(|n| hit(s, *n, title_max, viewer, src))
                    .collect(),
            ),
        )
        .with_opt(
            "total",
            h.partial
                .then(|| super::format::total_json(forum_core::Total::AtLeast(k))),
        )
        .with_opt(
            "fuzzy",
            (!matched.is_empty() && !h.nodes.is_empty())
                .then(|| Json::S(format!("matched {}", matched.join(", ")))),
        )
}

/// `hits`, or None when the search found nothing and was not cut short: `hello`'s `related` and
/// `knowledge`, and the `related` of a create or a `from_user`, are left out then (1.7, §20.1).
#[must_use]
pub fn some_hits(
    s: &State,
    h: &Hits,
    matched: &[String],
    title_max: usize,
    viewer: Option<SessionId>,
    src: &dyn Source,
) -> Option<Json> {
    (!h.nodes.is_empty() || h.partial).then(|| hits(s, h, matched, title_max, viewer, src))
}

/// 1.12 (§10.4): the `why_this` of work longer than every session's horizon, offered after every
/// fitting candidate.
pub const LONGER: &str =
    "longer than any session's horizon: do a part, checkpoint, release handoff";

/// `why_this = {tier, flags, aged, reason, est?}` (§10.4); 1.10: `est: "no estimate"` for a node to
/// take without `est`, offered under a horizon, which ranks it after the estimated ones. 1.12:
/// `horizon`, `LONGER`, for work longer than every session's horizon.
#[must_use]
pub fn why_this(w: &WhyThis) -> Json {
    let reason = match w.reason {
        Reason::Flagged => "flagged",
        Reason::Offered => "offered",
        Reason::Occurrence => "occurrence",
        Reason::Signal => "signal",
        Reason::Routing => "routing",
        Reason::Status => "status",
        Reason::ClosedFlagged => "closed_flagged",
        Reason::Review => "review",
    };
    Json::obj()
        .with("tier", Json::from(u32::from(w.tier)))
        .with("flags", flags(w.flags))
        .with("aged", Json::I(w.aged))
        .with("reason", Json::s(reason))
        .with_opt("est", w.no_estimate.then(|| Json::s("no estimate")))
        .with_opt("horizon", w.longer.then(|| Json::s(LONGER)))
}

/// `why_this` of a candidate of node `n`: 1.11 (§10.4), a review item's names its node's needs,
/// the caps its work asked for, when it has any. 1.12 (§10.4): `beyond_horizon`, `{count, first:
/// "/claims/lemma-01 est 60 > your horizon 45"}`, the work whose needs the caller's caps meet that
/// its horizon excludes, on the first candidate.
#[must_use]
pub fn why_this_at(s: &State, n: Option<NodeId>, w: &WhyThis) -> Json {
    let needs = n
        .filter(|_| w.reason == Reason::Review)
        .and_then(|n| s.nodes.get(&n))
        .map(|x| &x.needs)
        .filter(|x| !x.is_empty());
    why_this(w)
        .with_opt(
            "needs",
            needs.map(|x| Json::strs(x.iter().map(|l| l.as_str().to_string()))),
        )
        .with_opt(
            "beyond_horizon",
            w.beyond_horizon.map(|b| {
                // 1.12's review (§18): a count whose scan stopped at SCAN_MAX is `≥k`.
                let count = if b.capped {
                    Json::S(format!("≥{}", b.count))
                } else {
                    Json::from(b.count)
                };
                Json::obj().with("count", count).with(
                    "first",
                    Json::S(format!(
                        "{} est {} > your horizon {}",
                        node_ref(s, b.first),
                        b.est,
                        b.horizon
                    )),
                )
            }),
        )
}

/// The `why` of an empty `next` (§10.4, CS-9, CS-31): the candidates in the index, always, and the
/// count each exclusion removed when it removed any (1.7, §20.1), the nodes that wait (1.4, R27:
/// they come back when their waits resolve, so an empty `next` never says that no work is left
/// while some waits, D.8.5), each pause that excluded candidates with its source, end, and reason,
/// and the first health reason that is not green.
#[must_use]
pub fn empty_why(s: &State, w: &EmptyWhy, health: &Health, src: &dyn Source) -> Json {
    let query = if w.query_capped {
        Some(Json::S(format!("≥{}", w.query)))
    } else {
        (w.query > 0).then(|| Json::from(w.query))
    };
    let count = |k: u32| (k > 0).then(|| Json::from(k));
    let mut j = Json::obj()
        .with("candidates", Json::from(w.candidates))
        .with_opt("depends", count(w.depends))
        .with_opt("refused", count(w.refused))
        .with_opt("needs", count(w.needs))
        .with_opt("horizon", count(w.horizon))
        .with_opt("paused", count(w.paused))
        .with_opt("own", count(w.own))
        .with_opt("query", query)
        .with_opt("waiting", count(w.waiting))
        // 1.11 (§10.4, §12): the series whose next run is not due yet.
        .with_opt("before_slot", count(w.before_slot));
    if w.waiting > 0 {
        j.put("waits", Json::S(waits_sentence(w.waiting)));
    }
    let first = first_excluded(s, &w.first);
    if !first.is_empty() {
        j.put("first", Json::O(first));
    }
    if w.paused > 0 && !w.pause_scopes.is_empty() {
        let excluded = |scope: NodeId| {
            health.reasons.iter().find_map(|r| match r {
                HealthReason::Paused { top, .. } => top
                    .iter()
                    .find(|p| p.pause.scope == scope)
                    .map(|p| p.excluded),
                _ => None,
            })
        };
        let pauses: Vec<String> = w
            .pause_scopes
            .iter()
            .map(|scope| {
                let reason = src.pause_reason(*scope).map(|t| t.0).unwrap_or_default();
                let by = s
                    .pauses
                    .get(scope)
                    .map(|p| format!(" by {}/{} until {}", p.class, p.by.render(), hhmm(p.until)))
                    .unwrap_or_default();
                let ex = excluded(*scope)
                    .map(|k| format!(" (excludes {k})"))
                    .unwrap_or_default();
                format!(
                    "{}{by}: {}{ex}",
                    node_ref(s, *scope),
                    cut(&reason, PAUSE_REASON_MAX)
                )
            })
            .collect();
        j.put("pauses", Json::strs(pauses));
        j.put(
            "hint",
            Json::s(
                "read the reason, and lift a pause whose reason no longer holds with control \
                 unpause",
            ),
        );
    }
    if health.level != Level::Green
        && let Some(r) = health.reasons.first()
    {
        j.put("health", Json::S(reason_words(s, r)));
    }
    j
}

/// The first node of each exclusion, with the value that excluded it (1.6, §10.4): `needs:
/// /support/tag-cleanup needs support`, `horizon: /tidepool/app-store-511 est 60 > your horizon 30`.
/// In the lab's first round, agents found by trial what their own horizon or caps excluded.
fn first_excluded(s: &State, f: &FirstExcluded) -> Vec<(String, Json)> {
    let mut out: Vec<(String, Json)> = Vec::new();
    let mut put = |k: &str, v: String| out.push((k.to_string(), Json::S(v)));
    if let Some((n, lacks)) = &f.needs {
        let words: Vec<&str> = lacks.iter().map(Label::as_str).collect();
        put(
            "needs",
            format!(
                "{} needs {}, which your caps lack",
                node_ref(s, *n),
                words.join(", ")
            ),
        );
    }
    if let Some((n, est, horizon)) = f.horizon {
        put(
            "horizon",
            format!("{} est {est} > your horizon {horizon}", node_ref(s, n)),
        );
    }
    if let Some(n) = f.refused {
        put(
            "refused",
            format!(
                "{}: your class refused it, or is backed off from it after a release with cannot",
                node_ref(s, n)
            ),
        );
    }
    if let Some((n, t)) = f.depends {
        put(
            "depends",
            format!(
                "{} depends on {}, which is open",
                node_ref(s, n),
                node_ref(s, t)
            ),
        );
    }
    if let Some(n) = f.waiting {
        let on = s.nodes.get(&n).and_then(|x| x.wait.as_ref()).map(|w| w.on);
        put(
            "waiting",
            format!("{} waits for {}", node_ref(s, n), wait_words(s, on)),
        );
    }
    out
}

/// What a wait waits for, in words (§10.3): its question, node, time, or outside thing.
#[must_use]
pub fn wait_words(s: &State, on: Option<WaitOn>) -> String {
    match on {
        Some(WaitOn::Question { q, .. }) => format!("an answer to the question {}", q.render()),
        Some(WaitOn::UserQuestion(q)) => {
            format!("an answer to the question for the user {}", q.render())
        }
        Some(WaitOn::Node(t)) => format!("{} to close", node_ref(s, t)),
        Some(WaitOn::Time(t)) => format!("the time {}", time(t)),
        Some(WaitOn::External(t)) => {
            format!("something outside, until its recheck at {}", time(t))
        }
        None => "nothing now".to_string(),
    }
}

/// D.8.5's sentence for the nodes an empty `next` counts as waiting (1.4, §10.4, R27).
#[must_use]
pub fn waits_sentence(k: u32) -> String {
    format!(
        "{k} nodes wait (for a question, a node, a time, or something outside) and come back when \
         their waits resolve"
    )
}

/// One offer: the node or item, its `est`, and `why_this`.
fn offer(
    s: &State,
    r: &Ranked,
    title_max: usize,
    viewer: Option<SessionId>,
    src: &dyn Source,
) -> Json {
    let mut j = match r.cand {
        Cand::Node(n) => node_line(s, n, title_max, viewer, src),
        // 1.9 (§10.4): on a closed node, an item with no hold is the review of its objection.
        Cand::Status(n) => match forum_core::query::review_objection(s, n) {
            Some(o) => node_line(s, n, title_max, viewer, src).with("review", Json::S(o.render())),
            None => node_line(s, n, title_max, viewer, src).with("status", Json::Bool(true)),
        },
        Cand::Route(e) => Json::obj().with("route", Json::S(e.render())),
    };
    if let Cand::Node(n) | Cand::Status(n) = r.cand
        && let Some(e) = s.nodes.get(&n).and_then(|x| x.est)
    {
        j.put("est", Json::from(e));
    }
    let n = match r.cand {
        Cand::Node(n) | Cand::Status(n) => Some(n),
        Cand::Route(_) => None,
    };
    j.with("why_this", why_this_at(s, n, &r.why))
}

/// A note, agent question, or answer in an inbox.
#[must_use]
pub fn inbox_item(s: &State, e: NoteId, src: &dyn Source, max: Option<usize>) -> Json {
    match src.entry(e) {
        Some(v) => entry(s, &v, max).with("node", Json::S(node_ref(s, v.node))),
        None => Json::obj().with("e", Json::S(e.render())),
    }
}

/// What `routed` reads from the call beside the reply's `RoutedReply` (§8.2, D.6.2): the reason of
/// a routing to none that this call made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteCall {
    /// The reason this call routed the entry to none with.
    pub reason: Option<Text>,
}

impl RouteCall {
    /// What `cmd` tells `routed`. `existing`: the entry was routed before this call, whose own
    /// reason is then not its routing's (the first routing wins, §8.2).
    #[must_use]
    pub fn of(cmd: &Cmd, existing: bool) -> RouteCall {
        // 1.7: `reason` is optional, and the stored reason ("no work needed" by default) is read
        // from the source when the call gave none.
        let reason = match cmd {
            Cmd::Talk(TalkCmd::Route { reason, .. } | TalkCmd::FromUser { reason, .. }) => {
                reason.clone().filter(|_| !existing)
            }
            _ => None,
        };
        RouteCall { reason }
    }
}

/// The live holds of `sess` at `now`.
#[must_use]
pub fn live_holds(s: &State, sess: SessionId, now: Time) -> u32 {
    let k =
        s.ix.holds_by_session
            .get(&sess)
            .into_iter()
            .flatten()
            .filter(|t| s.holds.get(t).is_some_and(|h| live(h, now)))
            .count();
    u32::try_from(k).unwrap_or(u32::MAX)
}

/// `user_entry`: `{entry, key, routed}` (§20.3).
#[must_use]
pub fn user_entry(s: &State, u: &UserEntryReply, call: &RouteCall, src: &dyn Source) -> Json {
    Json::obj()
        .with("entry", Json::S(u.entry.render()))
        .with("key", Json::S(u.key.to_string()))
        .with("routed", routed(s, &u.routed, u.entry, call, src))
}

/// How user entry `e` was routed (1.4, §8.2, R3, D.11.7), from the reply's `RoutedReply`:
/// `{request: e…, node: <path>, follows?: e…}`, `{answers: e…}`, `{clarifies: e…}`, `{none:
/// <reason>}`, the reason this call gave or, for an earlier routing, its event's (null when that is
/// out of reach), or `{unrouted: <why>}`: the question it answers is no longer pending, or the call
/// named no target and the session holds no node or several.
#[must_use]
pub fn routed(
    s: &State,
    r: &RoutedReply,
    e: UserEntryId,
    call: &RouteCall,
    src: &dyn Source,
) -> Json {
    match r {
        RoutedReply::Request {
            request,
            node,
            follows,
        } => Json::obj()
            .with("request", Json::S(request.render()))
            .with("node", Json::S(node_ref(s, *node)))
            .with_opt("follows", follows.map(|f| Json::S(f.render()))),
        RoutedReply::Answers { question } => {
            Json::obj().with("answers", Json::S(question.render()))
        }
        RoutedReply::Clarifies { request } => {
            Json::obj().with("clarifies", Json::S(request.render()))
        }
        RoutedReply::NoAction => {
            let reason = call.reason.clone().or_else(|| src.route_reason(e));
            Json::obj().with("none", reason.map_or(Json::Null, |t| Json::S(t.0)))
        }
        RoutedReply::Unrouted(why) => {
            let why = match why {
                UnroutedWhy::NotPending { question } => {
                    format!("the question {} is no longer pending", question.render())
                }
                UnroutedWhy::NoTarget { holds } => format!(
                    "no target, and you hold {holds} node{}",
                    if *holds == 1 { "" } else { "s" }
                ),
            };
            Json::obj().with("unrouted", Json::S(why))
        }
    }
}

/// `user_decisions`: `{count, items}`, each item a question with its text cut to `max` and at most
/// `USER_OPTIONS_MAX` of its options, each cut to an equal share of `max`, with `options_total`
/// when there are more; a hidden question shows none of its texts (§16).
#[must_use]
pub fn user_decisions(s: &State, h: &HelloData, src: &dyn Source, max: usize) -> Json {
    let option_max = (max / USER_OPTIONS_MAX).max(16);
    let items = h
        .user_decisions
        .items
        .iter()
        .map(|q| {
            let mut j = Json::obj().with("question", Json::S(q.render()));
            if let Some(u) = s.uquestions.get(q) {
                if let Some(a) = u.about {
                    j.put("about", Json::S(node_ref(s, a)));
                }
                j.put("due", Json::S(time(u.due)));
            }
            match src.entry(*q) {
                Some(v) => {
                    let all = v.extras(forum_core::TextField::Option).count();
                    let options: Vec<String> = v
                        .extras(forum_core::TextField::Option)
                        .take(USER_OPTIONS_MAX)
                        .map(|t| shown_text(&v, t, Some(option_max)))
                        .collect();
                    let more = (all > options.len())
                        .then(|| Json::U(u64::try_from(all).unwrap_or(u64::MAX)));
                    j.with("text", Json::S(text_of(&v, Some(max))))
                        .with("options", Json::strs(options))
                        .with_opt("options_total", more)
                }
                None => j,
            }
        })
        .collect();
    Json::obj()
        .with("count", Json::from(h.user_decisions.count))
        .with_set("items", Json::A(items))
}

/// `user_digest`: `{count, older, items}`, oldest first.
#[must_use]
pub fn user_digest(s: &State, h: &HelloData, src: &dyn Source, max: usize) -> Json {
    let items = h
        .user_digest
        .items
        .iter()
        .map(|d| {
            let mut j = Json::obj().with("entry", Json::S(d.render()));
            if let Some(x) = s.digest.get(d) {
                let (what, q, node) = match x.item {
                    DigestItem::Assumption { question } => ("assumption", question, None),
                    DigestItem::DefaultApplied { question } => ("default_applied", question, None),
                    DigestItem::RefusalDrop { node, question } => {
                        ("refusal_drop", question, Some(node))
                    }
                    DigestItem::Lapsed { question } => ("lapsed", question, None),
                };
                j.put("what", Json::s(what));
                j.put("question", Json::S(q.render()));
                if let Some(n) = node {
                    j.put("node", Json::S(node_ref(s, n)));
                }
                if let Some(v) = src.entry(q) {
                    j.put("text", Json::S(text_of(&v, Some(max))));
                }
            }
            j
        })
        .collect();
    let older = h.user_digest.older.unwrap_or(0);
    Json::obj()
        .with("count", Json::from(h.user_digest.count))
        .with_opt("older", (older > 0).then_some(Json::U(older)))
        .with_set("items", Json::A(items))
}

/// What a `hello` shows beside `query::hello`.
pub struct HelloInput<'a> {
    /// For `session hello`: the notes the call delivered (`Reply::Hello.inbox`); None for a
    /// `hello` prepended to another first call, which shows the session's inbox.
    pub delivered: Option<&'a [NoteId]>,
    /// The entry `hello {from_user}` logged.
    pub user_entry: Option<&'a UserEntryReply>,
    /// The search hits for the intent and the human's message.
    pub hits: &'a SearchHits,
    /// What the call tells `user_entry`'s `routed`.
    pub route: &'a RouteCall,
    /// The `hello` that opened the session: the only one with the constitution (1.8, §20.3). In
    /// the lab's third round, later turns of the same agent got the 1,243-byte constitution again.
    pub first: bool,
}

/// The sections that the budget may drop, in the order it drops them (§20.3).
const DROP_ORDER: [&str; 6] = [
    "constitution",
    "knowledge",
    "related",
    "present",
    "orientation",
    "offers",
];

/// `hello` of `sess` at `now`, within `hello.budget`: the JSON object, and whether it is within
/// the budget (it may exceed it only with every droppable section dropped and the inbox empty).
#[must_use]
pub fn render_hello(
    s: &State,
    sess: SessionId,
    now: Time,
    health: &Health,
    input: &HelloInput<'_>,
    src: &dyn Source,
) -> Json {
    let h = hello(s, sess, now);
    let budget = usize::try_from(s.params.hello_budget).unwrap_or(usize::MAX);
    // Long texts are cut to an equal share of the budget among the user items, so the sections
    // that are never dropped stay bounded.
    let item_max = (budget / 16).max(16);
    let x = s.sessions.get(&sess);
    let serves = x.is_some_and(|x| !x.principals.is_empty());
    let mut sections: Vec<(&'static str, Json)> = Vec::new();
    // 1.7 (§20.1, §20.3): a section with nothing in it is left out.
    let put = |sections: &mut Vec<(&'static str, Json)>, k: &'static str, v: Json| {
        if v.is_set() {
            sections.push((k, v));
        }
    };
    // 1.5 (§20.3, R3): the session first, since some clients keep no MCP session, and a call
    // without `session` then opens another one; never dropped.
    sections.push(("session", Json::S(sess.render())));
    if input.first {
        sections.push(("constitution", Json::strs(CONSTITUTION)));
    }
    put(
        &mut sections,
        "holds",
        Json::A(
            h.holds
                .iter()
                .map(|(t, n, until)| {
                    Json::obj()
                        .with("token", Json::S(t.render()))
                        .with("path", Json::S(path(s, *n)))
                        .with("lease_until", Json::S(time(*until)))
                })
                .collect(),
        ),
    );
    put(
        &mut sections,
        "principals",
        Json::strs(h.principals.iter().map(|p| principal(s, *p))),
    );
    put(
        &mut sections,
        "caps",
        Json::strs(h.caps.iter().map(ToString::to_string)),
    );
    let inbox_ids: Vec<NoteId> = input
        .delivered
        .map_or_else(|| h.inbox.clone(), <[_]>::to_vec);
    let inbox: Vec<Json> = inbox_ids
        .iter()
        .map(|e| inbox_item(s, *e, src, Some(item_max)))
        .collect();
    // The notes still waiting after this call, which the next `hello` or `talk wait` delivers and
    // the status line's `inbox` counts (1.5, §13): `total` counts them beside those shown.
    let waiting = forum_core::query::inbox(s, sess, now)
        .into_iter()
        .filter(|e| !inbox_ids.contains(e))
        .count();
    put(&mut sections, "inbox", inbox_json(inbox, waiting));
    let offers = Json::A(
        h.offers
            .iter()
            .map(|r| offer(s, r, item_max, Some(sess), src))
            .collect(),
    );
    if h.offers.is_empty() {
        let why = h.offers_why.clone().unwrap_or_default();
        sections.push((
            "offers",
            Json::obj().with("why", empty_why(s, &why, health, src)),
        ));
    } else {
        sections.push(("offers", offers));
    }
    if let Some(r) = some_hits(
        s,
        &input.hits.related,
        &input.hits.matched,
        item_max,
        Some(sess),
        src,
    ) {
        sections.push(("related", r));
    }
    if let Some(k) = some_hits(
        s,
        &input.hits.knowledge,
        &input.hits.matched,
        item_max,
        Some(sess),
        src,
    ) {
        sections.push(("knowledge", k));
    }
    put(&mut sections, "present", present(s, sess, &h.present));
    put(
        &mut sections,
        "orientation",
        Json::obj()
            .with_set("map", Json::S(h.orientation.map.0.clone()))
            .with_set("house_rules", Json::S(h.orientation.house_rules.0.clone())),
    );
    if let Some(u) = input.user_entry {
        sections.push(("user_entry", user_entry(s, u, input.route, src)));
    }
    if serves {
        if h.user_decisions.count > 0 {
            sections.push(("user_decisions", user_decisions(s, &h, src, item_max)));
        }
        if h.user_digest.count > 0 || h.user_digest.older.unwrap_or(0) > 0 {
            sections.push(("user_digest", user_digest(s, &h, src, item_max)));
        }
    }
    let with_cut = |secs: &[(&'static str, Json)], cut: &[&'static str]| {
        let mut j = Json::O(
            secs.iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        );
        if !cut.is_empty() {
            j.put("cut", Json::strs(cut.iter().copied()));
        }
        j
    };
    // Whole sections only: the inbox is never cut (1.5, §20.3), since the notes it delivers are
    // marked read, and the core delivers at most 10 of them (§13). 1.7: the dropped sections are
    // named in `cut`, which counts against the budget.
    let mut cut: Vec<&'static str> = Vec::new();
    for name in DROP_ORDER {
        if with_cut(&sections, &cut).size() <= budget {
            break;
        }
        if sections.iter().any(|(k, _)| *k == name) {
            sections.retain(|(k, _)| *k != name);
            cut.push(name);
        }
    }
    with_cut(&sections, &cut)
}

/// The other sessions of the caller's class that `present` lists, at most (1.7, §20.3).
pub const PRESENT_MINE_MAX: usize = forum_core::query::WHO_MAX;

/// `present` (1.7, §20.3): `{total, by_class, sessions?, shown?}`, the live sessions counted by
/// class, and the other live sessions of the caller's class, newest first, at most
/// `PRESENT_MINE_MAX`, each with its holds and when it was last seen. The newest live sessions
/// are read until that many are found, or `SCAN_MAX` of them were read (§18: the cost of a call
/// does not grow with the size of the forum); a list cut short carries `shown`.
#[must_use]
pub fn present(s: &State, sess: SessionId, who: &forum_core::query::WhoPage) -> Json {
    let Some(class) = s.sessions.get(&sess).map(|x| &x.class) else {
        return Json::Null;
    };
    let mine_total = who
        .by_class
        .iter()
        .find(|(c, _)| c == class)
        .map_or(0, |(_, k)| *k)
        .saturating_sub(u32::from(s.ix.live_sessions.contains(&sess)));
    let scan = usize::try_from(forum_core::query::SCAN_MAX).unwrap_or(usize::MAX);
    let mine: Vec<Json> =
        s.ix.live_sessions
            .iter()
            .rev()
            .take(scan)
            .filter(|x| **x != sess)
            .filter_map(|x| s.sessions.get(x).map(|y| (*x, y)))
            .filter(|(_, y)| &y.class == class)
            .take(PRESENT_MINE_MAX)
            .map(|(x, y)| {
                let holds =
                    s.ix.holds_by_session
                        .get(&x)
                        .into_iter()
                        .flatten()
                        .map(|t| node_ref(s, t.node));
                Json::obj()
                    .with("session", Json::S(x.render()))
                    .with_set("holds", Json::strs(holds))
                    .with("last_seen", Json::S(time(y.last_seen)))
            })
            .collect();
    let shown = mine.len();
    let by_class = Json::O(
        who.by_class
            .iter()
            .map(|(c, k)| (c.to_string(), Json::from(*k)))
            .collect(),
    );
    Json::obj()
        .with("total", super::format::total_json(who.total))
        .with("by_class", by_class)
        .with_set("sessions", Json::A(mine))
        .with_opt(
            "shown",
            super::format::shown_if_cut(shown, forum_core::Total::Exact(mine_total)),
        )
}

/// The inbox as `{items}`, with `shown` and `total` when notes still wait (§18): `total` counts
/// the notes shown and the `waiting` ones that a later `hello` or `talk wait` delivers (1.5, §13).
/// Nothing at all with no note shown or waiting (1.7).
fn inbox_json(items: Vec<Json>, waiting: usize) -> Json {
    let shown = items.len();
    let total = shown.saturating_add(waiting);
    let j = Json::obj().with_set("items", Json::A(items));
    if waiting == 0 {
        return j;
    }
    j.with("shown", Json::U(u64::try_from(shown).unwrap_or(u64::MAX)))
        .with("total", Json::U(u64::try_from(total).unwrap_or(u64::MAX)))
}

/// Whether a session serves a principal (the user sections and rule 12 read it).
#[must_use]
pub fn serves_principal(s: &State, sess: SessionId) -> bool {
    s.sessions
        .get(&sess)
        .is_some_and(|x| !x.principals.is_empty())
}

/// What a note delivered on a write says of an open agent question that another session asked
/// (1.8, §13): its answer may wait until the agent knows it, while it is open. In the lab's third
/// round, a worker whose `hold next` delivered a question answered it at once with "I am checking".
#[must_use]
pub fn answer_may_wait(s: &State, e: NoteId, sess: Option<SessionId>) -> Option<String> {
    let aq = s.aquestions.get(&e)?;
    if aq.state != forum_core::AqState::Open || Some(aq.asker) == sess {
        return None;
    }
    let until = s.notes.get(&e)?.until;
    Some(format!(
        "it may wait until you know it: open until {}",
        time(until)
    ))
}
