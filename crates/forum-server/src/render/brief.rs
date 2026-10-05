//! The brief within `brief.budget` (§10.4, design D.8.6, D.11.7), and the full and review views of
//! `view node` (§20.4, CS-22, D.8.17).
//!
//! The header comes first: the node's id, path, title, state, status, est, versions, holders, an
//! open node's wait (1.8), its owners (own, or inherited with the source path, D-3), the rollup
//! (CS-32), and, when the digest
//! is empty, the start of the body (1.4), each only when set (1.7, §20.1: no empty status, null
//! est, empty holders, or rollup of zeros); then the predecessors along the `continues` chain with
//! their summaries; then the sections that are not empty, in §10.4's order (1.6: an empty one is
//! left out). Within the budget: (1) the newest item of every non-empty section, each cut to an
//! equal share of the remaining bytes when they do not all fit; (2) further items in section order,
//! newest first, while they fit; (3) a section cut short shows `shown` and `total`, and one shown
//! whole shows no counts (1.7, §18). The minimum wins over the budget (§10.4).
//!
//! Agreements (1.7, §10.4): the house rules, the agreements of `/`, are left out of every brief but
//! the root's own, since `hello` shows them; and an agreement that an earlier brief of the same
//! response printed is named by its source alone (`as above`). In the lab's second round, the house
//! rules came 27 times in briefs, and a read of four nodes printed two agreements four times each.
//!
//! Section 9, everything else, is the node's entries that sections 1 to 8 do not show: its total
//! is exact when `EVERYTHING_MAX` entries cover the node's feed, and otherwise `≥k`, a lower bound
//! read from the entries scanned and the count of those the scan left (D.8.6).

use std::collections::BTreeSet;

use forum_core::query::{
    BACKLINKS_MAX, BriefData, BriefItem, NoMeter, PAGE_DEFAULT, Rollup, SectionName, backlinks,
    brief,
};
use forum_core::{
    Eid, EntryType, Life, NodeId, ROOT, Role, Seq, SessionId, State, TextField, Time, Total,
    ViewMode, WaitOn,
};

use super::entries::{brief_item, entry, node_state, shown_text, text_of};
use super::format::{
    dur, flags, is_cut, link_rel, node_ref, outcome, path, shown_if_cut, thousands, time,
    total_json,
};
use super::json::Json;
use super::source::Source;

/// The start of the body in the header, when the digest is empty (1.4): at most this many bytes,
/// note included, and at most `digest.max` (the digest it stands in for) and a quarter of
/// `brief.budget` (1.5, R3, D.14.7). At 256 bytes, round R3's six-line task was cut mid-sentence,
/// and 15 sessions of 16 read the node again at once.
pub const BODY_HEAD_MAX: usize = 2048;

/// The bound of `body_head` under `params`: `BODY_HEAD_MAX`, `digest.max`, and a quarter of
/// `brief.budget`, so that the header stays within the brief's budget (1.5, R3).
#[must_use]
pub fn body_head_max(p: &forum_core::Params) -> usize {
    let digest = usize::try_from(p.digest_max).unwrap_or(usize::MAX);
    let budget = usize::try_from(p.brief_budget).unwrap_or(usize::MAX);
    BODY_HEAD_MAX.min(digest).min(budget / 4)
}

/// The start of `body` within `max` bytes: the whole body when it fits; otherwise its head, cut
/// with "…", and how much was cut, `(<shown> of <total> bytes; view node full for the rest)`, the
/// note counted within `max` (1.5, R3, D.14.7). When `max` leaves no character of the body beside
/// the note (a `digest.max` of a few dozen bytes, which `Params::valid` allows), the head is cut
/// with "…" alone, as any long text is (`format::cut`): the bound holds, and some of the body
/// shows, down to a `max` of 3 bytes, below which the ellipsis alone stands for the body.
#[must_use]
pub fn body_head(body: &str, max: usize) -> String {
    if body.len() <= max {
        return body.to_string();
    }
    let total = body.len();
    let note = |shown: usize| format!(" ({shown} of {total} bytes; view node full for the rest)");
    // The note is longest with `shown` at `total`'s width.
    let room = max.saturating_sub(super::format::ELLIPSIS.len() + note(total).len());
    let mut end = room.min(total);
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        return super::format::cut(body, max);
    }
    let head = body.get(..end).unwrap_or("");
    format!("{head}{}{}", super::format::ELLIPSIS, note(end))
}

/// Entries of the feed that section 9 ("everything else") reads, newest first.
pub const EVERYTHING_MAX: u32 = 50;

/// A section's name in the rendered brief.
#[must_use]
pub fn section_name(n: SectionName) -> &'static str {
    match n {
        SectionName::Flags => "flags",
        SectionName::Unfinished => "unfinished",
        SectionName::SaidAndCommitments => "said_and_commitments",
        SectionName::DecisionsAndAgreements => "decisions_and_agreements",
        SectionName::UnsettledRequests => "unsettled_requests",
        SectionName::Objections => "objections",
        SectionName::Digest => "digest",
        SectionName::LastCheckpoint => "last_checkpoint",
        SectionName::Everything => "everything_else",
    }
}

/// The counts that are not 0, in order, as `1,204 open, 3,311 done` (1.7); None when all are 0.
#[must_use]
pub fn counts(cs: &[(u64, &str)]) -> Option<String> {
    let parts: Vec<String> = cs
        .iter()
        .filter(|(k, _)| *k > 0)
        .map(|(k, what)| format!("{} {what}", thousands(*k)))
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The descendants of a rollup (§5), the counts that are not 0: `5 open, 32 done` (1.7).
#[must_use]
pub fn below(sub: &forum_core::Subtree) -> Option<String> {
    counts(&[
        (sub.open, "open"),
        (sub.done, "done"),
        (sub.failed, "failed"),
        (sub.other, "other"),
        (sub.flagged, "flagged"),
    ])
}

/// The rollup as one line (CS-32), its counts that are not 0 (1.7): `children 3 open, 1 held;
/// below: 1,204 open, 3,311 done, 512 failed, 40 flagged`; None for a node whose counts are all 0,
/// such as a leaf. In the lab's second round, 131 of 155 rollups were all zeros.
#[must_use]
pub fn rollup(r: &Rollup) -> Option<String> {
    let children = counts(&[
        (u64::from(r.open_children), "open"),
        (u64::from(r.held_children), "held"),
        (u64::from(r.flagged_children), "flagged"),
    ])
    .map(|c| format!("children {c}"));
    let below = below(&r.sub).map(|b| format!("below: {b}"));
    match (children, below) {
        (None, None) => None,
        (Some(c), None) => Some(c),
        (None, Some(b)) => Some(b),
        (Some(c), Some(b)) => Some(format!("{c}; {b}")),
    }
}

/// The owners in force on `n`: its own, else the nearest ancestor's, with that ancestor (D-3).
fn owners(s: &State, n: NodeId) -> Option<(Vec<NodeId>, Option<NodeId>)> {
    let mut cur = n;
    for _ in 0..=s.nodes.len() {
        let node = s.nodes.get(&cur)?;
        if !node.owners.is_empty() {
            let from = (cur != n).then_some(cur);
            return Some((node.owners.iter().copied().collect(), from));
        }
        cur = node.place.as_ref()?.parent;
    }
    None
}

/// The `what` of an outside wait in the header, at most this many bytes (1.8, §10.4).
pub const WAIT_WHAT_MAX: usize = 200;

/// The header's `wait` (1.8, §10.4): an open node's wait, `{for, since, recheck?, due?,
/// resolved?}`. `for` is what it waits for, for an outside wait the `what` of the release that set
/// it, when the brief's last checkpoint is that release; `since` when it was set; `recheck` an
/// outside wait's `recheck_after`, and `due`, once that has come, "recheck due since <time>: check
/// it, then decide or release"; a wait of another kind that has resolved carries `resolved`. In
/// the lab's third round, the briefs of waiting nodes said only `open`, and an agent that held the
/// values an outside wait awaited left the node alone.
fn wait_json(s: &State, b: &BriefData, now: Time, src: &dyn Source) -> Option<Json> {
    let node = s.nodes.get(&b.node).filter(|x| x.is_open())?;
    let w = node.wait.as_ref()?;
    let resolved = forum_core::flags::resolved(s, &w.on, now);
    let for_ = match w.on {
        WaitOn::External(_) => release_what(b, w.set_at, src)
            .unwrap_or_else(|| "something outside the forum".to_string()),
        other => super::hello::wait_words(s, Some(other)),
    };
    let mut j = Json::obj()
        .with("for", Json::S(for_))
        .with("since", Json::S(time(w.set_at)));
    match w.on {
        WaitOn::External(t) => {
            j.put("recheck", Json::S(time(t)));
            if resolved {
                j.put(
                    "due",
                    Json::S(format!(
                        "recheck due since {}: check it, then decide or release",
                        time(t)
                    )),
                );
            }
        }
        WaitOn::Question { .. } | WaitOn::UserQuestion(_) | WaitOn::Node(_) | WaitOn::Time(_) => {
            if resolved {
                j.put("resolved", Json::Bool(true));
            }
        }
    }
    Some(j)
}

/// The `what` of the release that set a wait at `set_at`: the brief's last checkpoint, when it is
/// that release.
fn release_what(b: &BriefData, set_at: Time, src: &dyn Source) -> Option<String> {
    let e = b
        .sections
        .iter()
        .find(|sec| sec.name == SectionName::LastCheckpoint)?
        .items
        .iter()
        .find_map(|it| match it {
            BriefItem::Checkpoint(e) => Some(*e),
            _ => None,
        })?;
    let v = src.entry(e).filter(|v| v.at == set_at)?;
    let what = v.extra(TextField::What)?;
    Some(shown_text(&v, what, Some(WAIT_WHAT_MAX)))
}

/// The header's `state`: `node_state`, and, for a claim closed `done` with no active decision and
/// a retracted one, that decision (1.10, §10.4): "closed done, decision eX retracted". In the
/// lab's fifth round (l5), reviewed lemmas whose deciding decisions were retracted read as plain
/// `closed done`.
fn header_state(s: &State, n: NodeId) -> String {
    let state = node_state(s, n);
    match forum_core::query::no_basis(s, n) {
        Some(d) => format!("{state}, decision {} retracted", d.render()),
        None => state,
    }
}

/// The header of a brief, with `wait`, read at `now` (1.8).
fn header(s: &State, b: &BriefData, now: Time, src: &dyn Source) -> Json {
    let n = b.node;
    let node = s.nodes.get(&n);
    // §10.4: each field only when it is set (1.7, §20.1).
    let mut j = Json::obj()
        .with("id", Json::S(n.render()))
        .with("path", Json::S(path(s, n)))
        .with_set(
            "title",
            Json::S(node.map(|x| x.title.0.clone()).unwrap_or_default()),
        )
        .with("state", Json::S(header_state(s, n)))
        .with_set(
            "status",
            Json::S(node.map(|x| x.status.0.clone()).unwrap_or_default()),
        )
        .with_opt(
            "est",
            node.and_then(|x| x.est).map(|e| Json::U(u64::from(e))),
        )
        .with("version", Json::U(b.version.0))
        .with("digest_version", Json::U(b.digest_version.0))
        .with_set("holders", holders_json(&b.holders))
        .with_opt("wait", wait_json(s, b, now, src))
        // 1.12 (§10.4, §19): the pause over the node, with its reason.
        .with_opt(
            "paused",
            node.filter(|x| x.is_open())
                .and_then(|_| super::entries::paused_note(s, n, now, src))
                .map(Json::S),
        );
    if let Some((own, from)) = owners(s, n) {
        j.put("owners", Json::strs(own.iter().map(|o| path(s, *o))));
        if let Some(f) = from {
            j.put("owners_from", Json::S(path(s, f)));
        }
    }
    if let Some(r) = rollup(&b.rollup) {
        j.put("rollup", Json::S(r));
    }
    if let Some(x) = node
        && x.digest.is_empty()
        && !x.body.is_empty()
    {
        j.put(
            "body_head",
            Json::S(body_head(&x.body.0, body_head_max(&s.params))),
        );
    }
    // 1.8 (§10.4): what the node rests on that is closed failed or refuted.
    if !b.rests_on.is_empty() {
        j.put(
            "rests_on",
            Json::A(
                b.rests_on
                    .iter()
                    .map(|r| {
                        Json::obj()
                            .with("node", Json::S(path(s, r.node)))
                            .with("state", Json::S(format!("closed {}", outcome(&r.outcome))))
                            .with(
                                "via",
                                Json::S(match r.via {
                                    forum_core::query::RestsVia::DependsOn => "depends_on".into(),
                                    forum_core::query::RestsVia::Basis(d) => {
                                        format!("basis of {}", d.render())
                                    }
                                }),
                            )
                    })
                    .collect(),
            ),
        );
    }
    j
}

/// Holders as `[{session, until}]`, the brief header's and a list row's form.
#[must_use]
pub fn holders_json(hs: &[(SessionId, Time)]) -> Json {
    Json::A(
        hs.iter()
            .map(|(x, until)| {
                Json::obj()
                    .with("session", Json::S(x.render()))
                    .with("until", Json::S(time(*until)))
            })
            .collect(),
    )
}

/// An item of a part, rendered with its texts cut to the given number of bytes (None: whole).
type ItemFn<'a> = Box<dyn Fn(Option<usize>) -> Json + 'a>;

/// One part of the brief that the budget fills: the predecessors, or a section.
struct Part<'a> {
    key: Option<&'static str>,
    /// Each item rendered with its texts cut to the given number of bytes (None: whole).
    items: Vec<ItemFn<'a>>,
    /// The entry each item shows, if it shows one, in the order of `items`.
    eids: Vec<Option<Eid>>,
    /// The source of each item that prints an agreement's text, in the order of `items` (1.7).
    printed: Vec<Option<NodeId>>,
    total: Total,
    chosen: Vec<Json>,
}

impl Part<'_> {
    /// An empty section, which the brief leaves out (1.6, §10.4).
    fn empty(&self) -> bool {
        self.items.is_empty() && matches!(self.total, Total::Exact(0) | Total::AtLeast(0))
    }

    /// The part as the brief shows it: its items, and `shown` and `total` only when the budget or
    /// the core cut it short (1.7, §18).
    fn shown(&self, j: Json) -> Json {
        let j = j.with("items", Json::A(self.chosen.clone()));
        if is_cut(self.chosen.len(), self.total) {
            j.with(
                "shown",
                Json::from(u64::try_from(self.chosen.len()).unwrap_or(0)),
            )
            .with("total", total_json(self.total))
        } else {
            j
        }
    }
}

/// A brief as rendered (1.7): the JSON, the items beyond each part's first, the entries it shows,
/// and the agreements whose text it printed.
struct Rendered {
    json: Json,
    extra: usize,
    shown: BTreeSet<Eid>,
    printed: BTreeSet<NodeId>,
}

/// The entry a brief item shows, if it shows one.
fn item_eid(it: &BriefItem) -> Option<Eid> {
    match it {
        BriefItem::Said(e)
        | BriefItem::Commitment(e)
        | BriefItem::Decision(e)
        | BriefItem::Request(e)
        | BriefItem::Clarification(e)
        | BriefItem::Objection(e)
        | BriefItem::Checkpoint(e)
        | BriefItem::Entry(e) => Some(*e),
        BriefItem::Intent(i) => Some(i.eid),
        BriefItem::LateAnswer { answer, .. } => Some(*answer),
        BriefItem::UserQuestion { question, .. } => Some(*question),
        BriefItem::Flag(_)
        | BriefItem::Agreement { .. }
        | BriefItem::Digest
        | BriefItem::ExpiredHold(_) => None,
    }
}

/// The total of section 9: the node's entries that sections 1 to 8 do not show. `window` is the
/// node's newest entries (at most `EVERYTHING_MAX`), `seq` its entry count (entries have the seqs
/// `1..=seq`), and `shown` the entries the other sections show. When the window is the whole feed
/// the count is exact; otherwise it is `≥k`: the window's other entries, plus the entries before
/// it less the shown entries outside it, some of which may lie on other nodes (a request that a
/// merge moved keeps its entry on the merged node, §16).
fn everything_total(
    window: &[super::source::EntryView],
    seq: u64,
    shown: &BTreeSet<forum_core::Eid>,
) -> Total {
    let to32 = |k: u64| u32::try_from(k).unwrap_or(u32::MAX);
    let len = |k: usize| u64::try_from(k).unwrap_or(u64::MAX);
    let other = len(window.iter().filter(|v| !shown.contains(&v.eid)).count());
    let before = seq.saturating_sub(len(window.len()));
    if before == 0 {
        return Total::Exact(to32(other));
    }
    let shown_in_window = window.iter().filter(|v| shown.contains(&v.eid)).count();
    let shown_before = len(shown.len().saturating_sub(shown_in_window));
    Total::AtLeast(to32(
        other.saturating_add(before.saturating_sub(shown_before)),
    ))
}

/// The largest text cut at which `render` fits `share` bytes, or the smallest cut.
fn fit(render: &dyn Fn(Option<usize>) -> Json, share: usize) -> Json {
    let whole = render(None);
    if whole.size() <= share {
        return whole;
    }
    let (mut lo, mut hi) = (0_usize, whole.size());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if render(Some(mid)).size() <= share {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    render(Some(lo))
}

/// The brief of `n` for `sess` at `now`, within `brief.budget`; and how many items beyond each
/// part's first it holds (the budget property: over the budget only when that is 0).
#[must_use]
pub fn render_brief(
    s: &State,
    n: NodeId,
    sess: Option<SessionId>,
    now: Time,
    src: &dyn Source,
) -> (Json, usize) {
    let r = brief_and_shown(s, n, sess, now, src, &BTreeSet::new());
    (r.json, r.extra)
}

/// `render_brief`, and the entries the rendered brief shows, which the full view's feed page leaves
/// out (1.6, §20.4). An empty section is left out (1.6, §10.4): in the lab, full views of several
/// nodes were about half empty sections and repeated entries. `above` holds the agreements whose
/// text an earlier brief of the same response printed, which this one names by source (1.7).
fn brief_and_shown(
    s: &State,
    n: NodeId,
    sess: Option<SessionId>,
    now: Time,
    src: &dyn Source,
    above: &BTreeSet<NodeId>,
) -> Rendered {
    let b = brief(s, n, sess, now, &mut NoMeter);
    let budget = usize::try_from(s.params.brief_budget).unwrap_or(usize::MAX);
    let mut parts: Vec<Part> = Vec::new();
    // The predecessors, nearest first, each with its summary and unsettled requests.
    let preds: Vec<ItemFn<'_>> = b
        .continues
        .iter()
        .map(|p| {
            let p = p.clone();
            let f: ItemFn<'_> = Box::new(move |max| {
                let summary = src
                    .close_entry(p.node)
                    .map(|v| text_of(&v, max))
                    .unwrap_or_default();
                Json::obj()
                    .with("node", Json::S(path(s, p.node)))
                    .with("state", Json::S(node_state(s, p.node)))
                    .with("summary", Json::S(summary))
                    .with_set(
                        "unsettled",
                        Json::strs(p.unsettled.iter().map(|r| r.render())),
                    )
            });
            f
        })
        .collect();
    if !preds.is_empty() {
        let eids = vec![None; preds.len()];
        let printed = vec![None; preds.len()];
        parts.push(Part {
            key: None,
            items: preds,
            eids,
            printed,
            total: b.continues_total,
            chosen: Vec::new(),
        });
    }
    let shown_eids: BTreeSet<Eid> = b
        .sections
        .iter()
        .flat_map(|sec| sec.items.iter().filter_map(item_eid))
        .collect();
    for sec in &b.sections {
        // 1.7 (§10.4): the house rules only at `hello`, except in the root's own brief.
        let house_rule = |it: &BriefItem| match it {
            BriefItem::Agreement { source } => *source == ROOT && n != ROOT,
            _ => false,
        };
        let sec_items: Vec<BriefItem> = sec
            .items
            .iter()
            .filter(|it| !house_rule(it))
            .copied()
            .collect();
        let dropped = u32::try_from(sec.items.len().saturating_sub(sec_items.len())).unwrap_or(0);
        let mut printed: Vec<Option<NodeId>> = Vec::new();
        let (items, eids, total): (Vec<ItemFn<'_>>, Vec<Option<Eid>>, Total) =
            if sec.name == SectionName::Everything {
                let window = src.newest(n, EVERYTHING_MAX);
                let seq = s.nodes.get(&n).map_or(0, |x| x.seq.0);
                let total = everything_total(&window, seq, &shown_eids);
                let rest: Vec<_> = window
                    .into_iter()
                    .filter(|v| !shown_eids.contains(&v.eid))
                    .collect();
                let eids = rest.iter().map(|v| Some(v.eid)).collect();
                printed = vec![None; rest.len()];
                let items = rest
                    .into_iter()
                    .map(|v| {
                        let f: ItemFn<'_> = Box::new(move |max| entry(s, &v, max));
                        f
                    })
                    .collect();
                (items, eids, total)
            } else {
                let eids = sec_items.iter().map(item_eid).collect();
                let items = sec_items
                    .iter()
                    .map(|it| {
                        let it = *it;
                        // 1.7 (§10.4): an agreement an earlier brief printed, by its source.
                        let f: ItemFn<'_> = match it {
                            BriefItem::Agreement { source } if above.contains(&source) => {
                                printed.push(None);
                                Box::new(move |_| {
                                    Json::obj()
                                        .with("agreements", Json::s("as above"))
                                        .with("source", Json::S(path(s, source)))
                                })
                            }
                            BriefItem::Agreement { source } => {
                                printed.push(Some(source));
                                Box::new(move |max| brief_item(s, n, &it, src, max))
                            }
                            _ => {
                                printed.push(None);
                                Box::new(move |max| brief_item(s, n, &it, src, max))
                            }
                        };
                        f
                    })
                    .collect();
                (items, eids, Total::Exact(sec.total.saturating_sub(dropped)))
            };
        let part = Part {
            key: Some(section_name(sec.name)),
            items,
            eids,
            printed,
            total,
            chosen: Vec::new(),
        };
        // 1.6 (§10.4): an empty section is left out.
        if !part.empty() {
            parts.push(part);
        }
    }
    let assemble = |parts: &[Part]| -> Json {
        let mut j = Json::obj().with("node", header(s, &b, now, src));
        let mut sections = Vec::new();
        for p in parts {
            match p.key {
                None => j.put("continues", p.shown(Json::obj())),
                Some(name) => sections.push(p.shown(Json::obj().with("name", Json::s(name)))),
            }
        }
        j.with_set("sections", Json::A(sections))
    };
    // (1) The newest item of every non-empty part, each cut to an equal share when needed.
    let skeleton = assemble(&parts).size();
    let firsts: Vec<usize> = (0..parts.len())
        .filter(|&i| parts.get(i).is_some_and(|p| !p.items.is_empty()))
        .collect();
    let whole: Vec<Json> = firsts
        .iter()
        .filter_map(|&i| parts.get(i).and_then(|p| p.items.first()).map(|f| f(None)))
        .collect();
    let remaining = budget.saturating_sub(skeleton);
    let need: usize = whole.iter().map(|j| j.size() + 1).sum();
    let share = if firsts.is_empty() {
        0
    } else {
        remaining / firsts.len()
    };
    for (k, &i) in firsts.iter().enumerate() {
        let Some(p) = parts.get_mut(i) else { continue };
        let item = if need <= remaining {
            whole.get(k).cloned().unwrap_or(Json::Null)
        } else {
            match p.items.first() {
                Some(f) => fit(&**f, share.saturating_sub(1)),
                None => continue,
            }
        };
        p.chosen.push(item);
    }
    // (2) Further items in part order, newest first, while they fit.
    let mut used = assemble(&parts).size();
    let mut extra = 0_usize;
    'fill: for i in 0..parts.len() {
        let len = parts.get(i).map_or(0, |p| p.items.len());
        for k in 1..len {
            let Some(item) = parts.get(i).and_then(|p| p.items.get(k)).map(|f| f(None)) else {
                break;
            };
            let grow = item.size() + 1;
            if used + grow > budget {
                break 'fill;
            }
            used += grow;
            extra += 1;
            if let Some(p) = parts.get_mut(i) {
                p.chosen.push(item);
            }
        }
    }
    let mut out = assemble(&parts);
    // A count that gained a digit can push the brief a few bytes over: drop the last extras.
    while out.size() > budget && extra > 0 {
        if let Some(p) = parts.iter_mut().rev().find(|p| p.chosen.len() > 1) {
            p.chosen.pop();
            extra -= 1;
        } else {
            break;
        }
        out = assemble(&parts);
    }
    let shown: BTreeSet<Eid> = parts
        .iter()
        .flat_map(|p| p.eids.iter().take(p.chosen.len()).flatten().copied())
        .collect();
    let printed: BTreeSet<NodeId> = parts
        .iter()
        .flat_map(|p| p.printed.iter().take(p.chosen.len()).flatten().copied())
        .collect();
    Rendered {
        json: out,
        extra,
        shown,
        printed,
    }
}

/// The default `capacity` of a node (§5), which the full view leaves out (1.7).
const CAPACITY_DEFAULT: u32 = 1;

/// Every content field of a node that is set (§20.4 `full`; 1.7, §20.1: no empty text, list, or
/// object, no priority of 0, and no capacity of 1). In the lab's second round, a full view of four
/// nodes spent 545 bytes on default fields.
fn content(s: &State, n: NodeId, src: &dyn Source) -> Json {
    let Some(x) = s.nodes.get(&n) else {
        return Json::obj();
    };
    let mut j = Json::obj()
        .with_set("title", Json::S(x.title.0.clone()))
        .with_set("body", Json::S(x.body.0.clone()))
        .with_set("status", Json::S(x.status.0.clone()))
        .with_set("digest", Json::S(x.digest.0.clone()))
        .with_set("agreements", Json::S(x.agreements.0.clone()))
        .with_set(
            "fields",
            Json::O(
                x.fields
                    .iter()
                    .map(|(k, v)| (k.0.clone(), Json::S(v.0.clone())))
                    .collect(),
            ),
        )
        .with_set(
            "files",
            Json::O(
                x.files
                    .iter()
                    .map(|(k, v)| {
                        let shown = src
                            .hidden_file(n, &k.0)
                            .map_or_else(|| v.0.clone(), |r| format!("[hidden: {}]", r.0));
                        (k.0.clone(), Json::S(shown))
                    })
                    .collect(),
            ),
        )
        .with_opt("kind", x.kind.as_ref().map(|k| Json::S(k.to_string())))
        .with_set("tags", Json::strs(x.tags.iter().map(ToString::to_string)))
        .with_set("needs", Json::strs(x.needs.iter().map(ToString::to_string)))
        .with_opt(
            "capacity",
            (x.capacity != CAPACITY_DEFAULT).then(|| Json::from(x.capacity)),
        )
        .with_opt(
            "priority",
            (x.priority != 0).then(|| Json::I(i64::from(x.priority))),
        )
        .with_opt("est", x.est.map(Json::from))
        .with_opt("due", x.due.map(|t| Json::S(time(t))))
        .with_set(
            "depends_on",
            Json::strs(x.depends_on.iter().map(|d| node_ref(s, *d))),
        )
        .with_set("keys", Json::strs(x.keys.keys().map(ToString::to_string)))
        .with_set(
            "links",
            Json::A(
                x.links
                    .iter()
                    .map(|(r, to)| {
                        Json::obj()
                            .with("rel", Json::s(link_rel(*r)))
                            .with("to", Json::S(node_ref(s, *to)))
                    })
                    .collect(),
            ),
        )
        .with_set(
            "flags",
            flags(s.ix.node.get(&n).map(|i| i.flags).unwrap_or_default()),
        )
        .with("created_at", Json::S(time(x.created_at)));
    match &x.role {
        Role::Plain => {}
        Role::Series(series) => {
            j.put(
                "repeat",
                Json::obj()
                    .with("cron", Json::S(series.repeat.cron_text.0.clone()))
                    .with("tz", Json::S(series.repeat.tz.name.clone()))
                    .with("run_within", Json::S(dur(series.repeat.run_within))),
            );
        }
        Role::Occurrence(o) => {
            j.put(
                "occurrence",
                Json::obj()
                    .with("series", Json::S(node_ref(s, o.series)))
                    .with("slot", Json::S(time(o.slot)))
                    .with("window_end", Json::S(time(o.window_end))),
            );
        }
        Role::Signal(k) => {
            j.put(
                "signal",
                Json::obj()
                    .with("a", Json::S(node_ref(s, k.a)))
                    .with("b", Json::S(node_ref(s, k.b))),
            );
        }
    }
    if let Life::Closed(c) = &x.life {
        j.put(
            "closed",
            Json::obj()
                .with("at", Json::S(time(c.at)))
                .with("outcome", Json::S(super::format::outcome(&c.outcome)))
                .with_opt(
                    "merged_into",
                    c.merged_into.map(|m| Json::S(node_ref(s, m))),
                ),
        );
    }
    j
}

/// What rests on a node (CS-22): page `page` of each list, with `shown/total` when it is cut
/// short (1.7, §18), and the hint "page: k+1 for more" when a list goes on: for an exact total,
/// when the pages so far do not reach it; for a `≥k` total, only when this page is full, so that
/// an agent that follows the hint stops at the first page that is not. A list that nothing rests on
/// is left out, and so is the whole when all three are (1.7: in the lab's second round, such empty
/// lists were 440 bytes of a read of four nodes).
fn rests_on(s: &State, n: NodeId, page: u32) -> Json {
    let bl = backlinks(s, n, page, &mut NoMeter);
    let next = u64::from(page).saturating_add(1);
    let list = |items: Vec<Json>, t: Total| {
        if items.is_empty() && matches!(t, Total::Exact(0)) {
            return Json::Null;
        }
        let more = match t {
            Total::Exact(k) => {
                next.saturating_mul(u64::try_from(BACKLINKS_MAX).unwrap_or(0)) < u64::from(k)
            }
            Total::AtLeast(_) => !items.is_empty() && items.len() >= BACKLINKS_MAX,
        };
        let cut = shown_if_cut(items.len(), t);
        Json::obj()
            .with("items", Json::A(items))
            .with_opt("shown", cut)
            .with_opt(
                "more",
                more.then(|| Json::S(format!("page: {next} for more"))),
            )
    };
    let links = bl
        .links
        .iter()
        .map(|(r, from)| {
            Json::obj()
                .with("rel", Json::s(link_rel(*r)))
                .with("from", Json::S(node_ref(s, *from)))
        })
        .collect();
    let deps = bl
        .dependents
        .iter()
        .map(|d| Json::S(node_ref(s, *d)))
        .collect();
    let cited = bl
        .cited_by
        .iter()
        .map(|d| {
            let at = s
                .decisions
                .get(d)
                .map(|x| node_ref(s, x.node))
                .unwrap_or_default();
            Json::obj()
                .with("decision", Json::S(d.render()))
                .with("at", Json::S(at))
        })
        .collect();
    Json::obj()
        .with_set("links", list(links, bl.links_total))
        .with_set("dependents", list(deps, bl.dependents_total))
        .with_set("cited_by", list(cited, bl.cited_by_total))
}

/// `view node` of one node in `view` (§20.4): the brief; the full view, which is that whole brief
/// with every section, then every content field, the first feed page, and what rests on the node
/// (1.4, R27, CS-22); or the review view (D.8.17).
#[must_use]
pub fn view_node(
    s: &State,
    n: NodeId,
    view: ViewMode,
    page: u32,
    sess: Option<SessionId>,
    now: Time,
    src: &dyn Source,
) -> Json {
    view_node_after(s, n, view, page, sess, now, src, &mut BTreeSet::new())
}

/// `view_node` of one of the nodes of a response (1.7, §10.4): `printed` holds the agreements
/// whose text an earlier brief of the response printed, which this one names by source, and gains
/// those this one prints.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "view_node's arguments and the response's printed agreements"
)]
pub fn view_node_after(
    s: &State,
    n: NodeId,
    view: ViewMode,
    page: u32,
    sess: Option<SessionId>,
    now: Time,
    src: &dyn Source,
    printed: &mut BTreeSet<NodeId>,
) -> Json {
    match view {
        ViewMode::Brief => {
            let r = brief_and_shown(s, n, sess, now, src, printed);
            printed.extend(r.printed);
            r.json
        }
        ViewMode::Full => {
            let r = brief_and_shown(s, n, sess, now, src, printed);
            printed.extend(r.printed.iter().copied());
            let in_brief = r.shown;
            let upto = s
                .nodes
                .get(&n)
                .map_or(Seq(0), |x| Seq(x.seq.0.min(u64::from(PAGE_DEFAULT))));
            let feed = src.feed(n, Seq(0), upto, &BTreeSet::new(), PAGE_DEFAULT);
            let cut = shown_if_cut(feed.entries.len(), feed.total);
            // 1.6 (§20.4): the page leaves out the entries the brief shows, and names them.
            let (repeated, rest): (Vec<_>, Vec<_>) =
                feed.entries.iter().partition(|v| in_brief.contains(&v.eid));
            let page_j = Json::obj()
                .with_set(
                    "entries",
                    Json::A(rest.iter().map(|v| entry(s, v, None)).collect()),
                )
                .with_set(
                    "in_brief",
                    Json::strs(repeated.iter().map(|v| v.eid.render())),
                )
                .with_opt("shown", cut);
            Json::obj()
                .with("brief", r.json)
                .with("content", content(s, n, src))
                .with_set("feed", page_j)
                .with_set("rests_on", rests_on(s, n, page))
        }
        ViewMode::Review => {
            let x = s.nodes.get(&n);
            let seq = x.map_or(Seq(0), |x| x.seq);
            let types: BTreeSet<EntryType> = [EntryType::Approve, EntryType::Object].into();
            let page_e = src.feed(n, Seq(0), seq, &types, forum_core::query::PAGE_MAX);
            let mut j = Json::obj()
                .with("id", Json::S(n.render()))
                .with("path", Json::S(path(s, n)))
                .with("state", Json::S(node_state(s, n)));
            if let Some(x) = x {
                j.put("version", Json::U(x.version.0));
                j = j
                    .with_set("title", Json::S(x.title.0.clone()))
                    .with_set("body", Json::S(x.body.0.clone()));
                j = j.with_set(
                    "fields",
                    Json::O(
                        x.fields
                            .iter()
                            .map(|(k, v)| (k.0.clone(), Json::S(v.0.clone())))
                            .collect(),
                    ),
                );
                j = j.with_set(
                    "files",
                    Json::O(
                        x.files
                            .iter()
                            .map(|(k, v)| {
                                let shown = src
                                    .hidden_file(n, &k.0)
                                    .map_or_else(|| v.0.clone(), |r| format!("[hidden: {}]", r.0));
                                (k.0.clone(), Json::S(shown))
                            })
                            .collect(),
                    ),
                );
                j = j
                    .with_set("digest", Json::S(x.digest.0.clone()))
                    .with_set("status", Json::S(x.status.0.clone()));
                if let Life::Closed(c) = &x.life {
                    let summary = src
                        .close_entry(n)
                        .map(|v| text_of(&v, None))
                        .unwrap_or_default();
                    j.put(
                        "closure",
                        Json::obj()
                            .with("at", Json::S(time(c.at)))
                            .with("outcome", Json::S(super::format::outcome(&c.outcome)))
                            .with("summary", Json::S(summary)),
                    );
                }
            }
            let cut = shown_if_cut(page_e.entries.len(), page_e.total);
            let reviews = Json::obj()
                .with_set(
                    "entries",
                    Json::A(page_e.entries.iter().map(|v| entry(s, v, None)).collect()),
                )
                .with_opt("shown", cut);
            j.with_set("reviews", reviews)
        }
    }
}
