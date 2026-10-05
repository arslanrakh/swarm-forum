//! The report of one run: the "Reports" column of D.15.4 for its profile, measured, beside the
//! "Expected" column, with every contradiction and the trace that shows it. Floats appear here
//! only, in shares and percentiles of what the run counted.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use forum_core::{DAY, Dur, HOUR, NodeId, USER_PRINCIPAL, UqState};

use crate::observe::{UqKind, UqOutcome, stamp};
use crate::profile::{Kind, Rate};
use crate::sim::Sim;

fn pct(a: u64, b: u64) -> String {
    if b == 0 {
        "n/a".to_string()
    } else {
        format!("{:.1} %", 100.0 * a as f64 / b as f64)
    }
}

fn days(d: Dur) -> String {
    if d.0 >= DAY.0 {
        format!("{:.1} d", d.0 as f64 / DAY.0 as f64)
    } else {
        format!("{:.1} h", d.0 as f64 / HOUR.0 as f64)
    }
}

fn percentile(v: &mut [usize], p: usize) -> usize {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let rank = (v.len() * p).div_ceil(100).max(1);
    v[rank - 1]
}

/// The flags that reproduce a run of `p`: the profile, the days, the seed, and every flag whose
/// value differs from the profile's default.
#[must_use]
pub fn command_line(p: &crate::profile::Profile) -> String {
    let d = crate::profile::Profile::of(p.kind, p.days, p.seed);
    let mut out = format!(
        "--profile {} --days {} --seed {}",
        p.kind.name(),
        p.days,
        p.seed
    );
    if p.warmup_days != d.warmup_days {
        let _ = write!(out, " --warmup-days {}", p.warmup_days);
    }
    if p.cooldown_days != d.cooldown_days {
        let _ = write!(out, " --cooldown-days {}", p.cooldown_days);
    }
    if p.principals != d.principals {
        let _ = write!(out, " --principals {}", p.principals);
    }
    if p.p_follow != d.p_follow {
        let _ = write!(out, " --p-follow 0.{:06}", p.p_follow);
    }
    if p.check_every_hours != d.check_every_hours {
        let _ = write!(out, " --check-hours {}", p.check_every_hours);
    }
    out
}

/// The report of a finished run, in Markdown.
#[must_use]
#[allow(clippy::too_many_lines, reason = "one report, section by section")]
pub fn render(sim: &Sim) -> String {
    let mut o = String::new();
    let p = &sim.p;
    let obs = &sim.f.obs;
    let from = sim.measure_from;
    let _ = writeln!(
        o,
        "## Profile `{}`: {} days, seed {}, p_follow {}\n",
        p.kind.name(),
        p.days,
        p.seed,
        f64::from(p.p_follow) / 1e6
    );
    let _ = writeln!(
        o,
        "Reproduce: `cargo run --release -p forum-sim -- {}`.\n",
        command_line(p)
    );
    if p.warmup_days > 0 || p.cooldown_days > 0 {
        let _ = writeln!(
            o,
            "Warm-up: {} days before the measurement; cool-down: {} days after it, during which \
             only the fates of the questions asked within it are followed. Traces count days from \
             the warm-up's start, so the measurement is d{}–d{}.\n",
            p.warmup_days,
            p.cooldown_days,
            p.warmup_days,
            p.warmup_days + p.days
        );
    } else {
        let _ = writeln!(
            o,
            "Traces count days from the start of the measurement (d0–d{}).\n",
            p.days
        );
    }

    // ---------------------------------------------------------------- activity
    let calls: u64 = sim.f.calls.values().sum();
    let _ = writeln!(
        o,
        "### Activity (whole run, warm-up included unless marked)\n"
    );
    let _ = writeln!(o, "| What | Count |\n|---|---|");
    for (role, n) in &sim.m.sessions {
        let _ = writeln!(
            o,
            "| sessions opened in the measurement, {role} | {n} ({:.1} a day) |",
            *n as f64 / p.days.max(1) as f64
        );
    }
    let _ = writeln!(o, "| calls | {calls} |");
    let _ = writeln!(o, "| events | {} |", obs.n.events);
    let _ = writeln!(
        o,
        "| tasks created in the measurement | {} |",
        sim.m.tasks_created
    );
    let _ = writeln!(
        o,
        "| requests recorded / settled | {} / {} |",
        obs.n.requests, obs.n.settled
    );
    let _ = writeln!(
        o,
        "| holds issued / expired (sessions that vanished or overran) | {} / {} |",
        obs.n.holds, obs.n.expired
    );
    let _ = writeln!(
        o,
        "| worker sessions that vanished mid-task (measurement) | {} |",
        sim.m.drops
    );
    let _ = writeln!(
        o,
        "| spurious `cannot`s / `Refused` events | {} / {} |",
        sim.m.cannots, obs.n.refusals
    );
    let _ = writeln!(o, "| empty `next` | {} |", sim.m.next_empty);
    let _ = writeln!(
        o,
        "| nodes closed / reopened | {} / {} |",
        obs.n.closed, obs.n.reopened
    );
    if p.kind == Kind::Company {
        let _ = writeln!(
            o,
            "| principals at the start / leavers / gone marks / joiners (measurement) | {} / {} / {} / {} |",
            p.principals, sim.m.leavers, sim.m.gone_marks, sim.m.joiners
        );
        let _ = writeln!(
            o,
            "| answers given by principals | {} |",
            sim.m.answers_given
        );
        let _ = writeln!(o, "| reports of kind result | {} |", obs.n.results);
        let _ = writeln!(
            o,
            "| misaddressed reports injected / seen by the observer | {} / {} |",
            sim.m.mistakes_reports, obs.n.misaddressed_reports
        );
        let _ = writeln!(
            o,
            "| misaddressed digest showings injected / seen | {} / {} |",
            sim.m.mistakes_digest, obs.n.misaddressed_shows
        );
    } else {
        let _ = writeln!(
            o,
            "| people answers recorded with `heard` | {} |",
            sim.m.heard
        );
    }
    let _ = writeln!(
        o,
        "| invariant checks (`check_invariants`) / violations | {} / {} |",
        sim.m.checks,
        sim.m.violations.len()
    );
    let _ = writeln!(
        o,
        "| responses with next calls / with a first `now` call / calls followed | {} / {} / {} of {} responses |",
        sim.f.guided_with_items, sim.f.guided_with_now, sim.m.follows, sim.f.guided_responses
    );
    let _ = writeln!(
        o,
        "| keys stuck after their own firing | {} |",
        sim.f.stuck.len()
    );
    let _ = writeln!(o);
    if !sim.m.followed.is_empty() {
        let _ = writeln!(
            o,
            "Next calls followed (rule of §20.9, call, outcome); {} second holds given back:\n",
            sim.m.second_holds
        );
        let _ = writeln!(o, "| Rule | Call | Outcome | Count |\n|---|---|---|---|");
        for ((rule, call, outcome), v) in &sim.m.followed {
            let _ = writeln!(o, "| `{rule}` | `{call}` | {outcome} | {v} |");
        }
        let _ = writeln!(o);
    }
    if !sim.f.errors.is_empty() {
        let _ = writeln!(
            o,
            "Refused calls (each is an agent call the core answered with an error):\n"
        );
        let _ = writeln!(o, "| Call and error | Count |\n|---|---|");
        for (k, v) in &sim.f.errors {
            let _ = writeln!(o, "| `{}` | {v} |", k.replace('|', "/"));
        }
        let _ = writeln!(o);
    }
    if !sim.f.warnings.is_empty() {
        let w: Vec<String> = sim
            .f
            .warnings
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        let _ = writeln!(o, "Warnings: {}.\n", w.join(", "));
    }
    for (k, (t, _, c)) in &sim.m.violations {
        let _ = writeln!(
            o,
            "- invariant violation `{k}`: first at {}, seen by {c} checks",
            stamp(*t)
        );
    }
    for k in &sim.f.stuck {
        let _ = writeln!(o, "- stuck key: `{k}`");
    }
    if !sim.m.violations.is_empty() || !sim.f.stuck.is_empty() {
        let _ = writeln!(o);
    }

    // ---------------------------------------------------------------- common measures
    let red = sim.m.health[2];
    let samples: u64 = sim.m.health.iter().sum();
    let health_cell = format!(
        "{} of {samples} hourly samples red, {} yellow (`query::health` has no reason for questions, CS-9)",
        pct(red, samples),
        pct(sim.m.health[1], samples)
    );
    let mut bytes: Vec<usize> = sim.m.hellos.iter().map(|h| h.bytes).collect();
    let budget = sim.f.s.params.hello_budget as usize;
    let over = bytes.iter().filter(|&&b| b > budget).count();
    let max_items = sim
        .m
        .hellos
        .iter()
        .map(|h| h.decision_items.max(h.digest_items))
        .max()
        .unwrap_or(0);
    let max_dec = sim.m.hellos.iter().map(|h| h.decisions).max().unwrap_or(0);
    let max_dig = sim.m.hellos.iter().map(|h| h.digest).max().unwrap_or(0);
    let max_older = sim.m.hellos.iter().map(|h| h.older).max().unwrap_or(0);
    let p50 = percentile(&mut bytes, 50);
    let p99 = percentile(&mut bytes, 99);
    let maxb = bytes.iter().copied().max().unwrap_or(0);
    let hello_cell = format!(
        "{} hellos; structured size p50 {p50} B, p99 {p99} B, max {maxb} B ({over} over the 4,096 B budget before the shell's cut); at most {max_items} items in a user section; pending counts up to {max_dec} questions and {max_dig} digest entries, {max_older} older",
        sim.m.hellos.len()
    );

    match p.kind {
        Kind::Company => company(&mut o, sim, &health_cell, &hello_cell, from, sim.measure_to),
        Kind::NoUser => no_user(&mut o, sim, &health_cell, &hello_cell),
    }

    // ---------------------------------------------------------------- findings
    let _ = writeln!(o, "### Findings\n");
    if obs.findings.is_empty() && sim.m.violations.is_empty() && sim.f.stuck.is_empty() {
        let _ = writeln!(
            o,
            "None: no settlement, answer, or digest entry reached another principal, and no invariant failed.\n"
        );
    }
    let mut by_kind: BTreeMap<&str, Vec<&crate::observe::Finding>> = BTreeMap::new();
    for f in &obs.findings {
        by_kind.entry(f.what).or_default().push(f);
    }
    for (what, fs) in &by_kind {
        let _ = writeln!(
            o,
            "- **{what}**: {} times. First: {} — {}",
            fs.len(),
            stamp(fs[0].at),
            fs[0].detail
        );
    }
    let mut shown: BTreeSet<NodeId> = BTreeSet::new();
    for (k, (t, v, c)) in &sim.m.violations {
        let _ = writeln!(
            o,
            "- **invariant {:?} violated**: `{k}`, first at {}, seen by {c} checks.",
            v.inv,
            stamp(*t)
        );
        if let Some(n) = violation_node(sim, &v.detail) {
            describe_entry(&mut o, sim, &v.detail);
            if shown.insert(n) {
                trace(&mut o, sim, n);
            }
        }
    }
    for fs in by_kind.values() {
        if let Some(n) = fs.iter().find_map(|f| f.node)
            && shown.insert(n)
        {
            trace(&mut o, sim, n);
        }
    }
    let _ = writeln!(o);
    o
}

/// The node whose trace shows a violation: the node itself, or the home of the entry it names.
fn violation_node(sim: &Sim, d: &forum_core::ViolationDetail) -> Option<NodeId> {
    let obs = &sim.f.obs;
    match d {
        forum_core::ViolationDetail::Node(n) => Some(*n),
        forum_core::ViolationDetail::Hold(t) => Some(t.node),
        forum_core::ViolationDetail::Entry(e) => obs
            .uqs
            .get(e)
            .and_then(|u| u.about)
            .or_else(|| obs.pqs.get(e).map(|q| q.node))
            .or_else(|| obs.requests.get(e).map(|r| r.node)),
        _ => None,
    }
}

/// What the observer knows of the entry a violation names.
fn describe_entry(o: &mut String, sim: &Sim, d: &forum_core::ViolationDetail) {
    let obs = &sim.f.obs;
    if let forum_core::ViolationDetail::Entry(e) = d {
        if let Some(u) = obs.uqs.get(e) {
            let _ = writeln!(
                o,
                "  The entry is a question for the user: {u:?}; in the state now: {:?}.",
                sim.f.s.uquestions.get(e)
            );
        }
        if let Some(q) = obs.pqs.get(e) {
            let _ = writeln!(
                o,
                "  The entry is a people question: {q:?}; in the state now: {:?}.",
                sim.f.s.pquestions.get(e)
            );
        }
    }
}

fn trace(o: &mut String, sim: &Sim, n: NodeId) {
    let _ = writeln!(o, "\nTrace of node #{}:\n\n```text", n.0);
    if let Some(t) = sim.f.obs.traces.get(&n) {
        for line in t.iter().take(60) {
            let _ = writeln!(o, "{line}");
        }
        if t.len() > 60 {
            let _ = writeln!(o, "… {} more", t.len() - 60);
        }
    }
    let _ = writeln!(o, "```");
}

#[allow(clippy::too_many_lines, reason = "one profile's table and its details")]
fn company(
    o: &mut String,
    sim: &Sim,
    health_cell: &str,
    hello_cell: &str,
    from: forum_core::Time,
    to: forum_core::Time,
) {
    let obs = &sim.f.obs;
    let p = &sim.p;
    // Questions an agent asked within the measurement.
    let qs: Vec<(&forum_core::QuestionId, &crate::observe::Uq)> = obs
        .uqs
        .iter()
        .filter(|(_, u)| u.created >= from && u.created <= to && u.kind == UqKind::Agent)
        .collect();
    let n = qs.len() as u64;
    let mut first = 0_u64;
    let mut later = 0_u64;
    let mut other = 0_u64;
    let mut defaulted = 0_u64;
    let mut lapsed = 0_u64;
    let mut pending = 0_u64;
    let mut with_default = 0_u64;
    let mut first_no_default = 0_u64;
    let mut by_rate: BTreeMap<Rate, (u64, u64)> = BTreeMap::new();
    let mut first_is_requester = 0_u64;
    for (_, u) in &qs {
        if u.has_default {
            with_default += 1;
        }
        let to0 = u.to.first().copied().unwrap_or(USER_PRINCIPAL);
        let rate = obs.person_of(to0).map(|i| sim.people[i].rate);
        let e = rate.map(|r| by_rate.entry(r).or_default());
        let about_req = u
            .about
            .and_then(|a| sim.tasks.get(&a))
            .and_then(|t| t.requester);
        if about_req.is_some() && obs.person_of(to0) == about_req {
            first_is_requester += 1;
        }
        let answered_first = match u.outcome {
            UqOutcome::Answered { by: Some(b), stage } => {
                if obs.same_person(b, to0) {
                    first += 1;
                    true
                } else {
                    let at_stage = u.to.get(usize::from(stage)).copied();
                    if at_stage.is_some_and(|x| obs.same_person(b, x)) {
                        later += 1;
                    } else {
                        other += 1;
                    }
                    false
                }
            }
            UqOutcome::Answered { by: None, .. } => {
                other += 1;
                false
            }
            UqOutcome::Defaulted { .. } => {
                defaulted += 1;
                false
            }
            UqOutcome::Lapsed => {
                lapsed += 1;
                false
            }
            UqOutcome::Pending => {
                pending += 1;
                false
            }
        };
        if answered_first && !u.has_default {
            first_no_default += 1;
        }
        if let Some(e) = e {
            e.0 += 1;
            if answered_first {
                e.1 += 1;
            }
        }
    }
    let resolved = n - pending;
    // Lapses within the measurement, by the first addressee of the question.
    let lapses_in: Vec<&crate::observe::Uq> = obs
        .uqs
        .values()
        .filter(|u| {
            u.outcome == UqOutcome::Lapsed && u.resolved_at.is_some_and(|t| t >= from && t <= to)
        })
        .collect();
    let weeks = p.days as f64 / 7.0;
    let principals = p.principals as f64;
    let mut per_person_week: BTreeMap<(Option<usize>, u64), u64> = BTreeMap::new();
    for u in &lapses_in {
        let to0 = u.to.first().copied().unwrap_or(USER_PRINCIPAL);
        let wk = u.resolved_at.map_or(0, |t| t.since(from).0 / (7 * DAY.0));
        *per_person_week.entry((obs.person_of(to0), wk)).or_default() += 1;
    }
    let max_ppw = per_person_week.values().copied().max().unwrap_or(0);
    let to_user_lapses = lapses_in
        .iter()
        .filter(|u| u.to.iter().all(|&x| x == USER_PRINCIPAL))
        .count();
    let settle_wrong = obs
        .findings
        .iter()
        .filter(|f| f.what.starts_with("settlement"))
        .count();
    let answer_wrong = obs
        .findings
        .iter()
        .filter(|f| f.what.starts_with("answer"))
        .count();
    let digest_shown_wrong = obs
        .findings
        .iter()
        .filter(|f| f.what == "digest entry shown to another principal")
        .count();
    let digest_aud_wrong = obs
        .findings
        .iter()
        .filter(|f| f.what.starts_with("digest entry for"))
        .count();
    let digests_in: Vec<&crate::observe::Dg> = obs
        .digests
        .values()
        .filter(|d| d.created >= from && d.created <= to)
        .collect();
    let digest_user = digests_in.iter().filter(|d| d.to == USER_PRINCIPAL).count();
    let digest_shown = digests_in.iter().filter(|d| d.shown.is_some()).count();

    let _ = writeln!(o, "### Against D.15.4 (company)\n");
    let _ = writeln!(
        o,
        "| Report | Measured | Expected (D.15.4) | Verdict |\n|---|---|---|---|"
    );
    let share = pct(first, n);
    let verdict_share = if n == 0 {
        "not measured"
    } else if first * 100 >= n * 75 {
        "met (the review's figure is for 1,000 principals)"
    } else {
        "below: see the breakdown"
    };
    let _ = writeln!(
        o,
        "| share of questions answered by their first addressee | {share} ({first} of {n} asked in the measurement; {} of the {resolved} resolved) | about 81 % at 1,000 principals | {verdict_share} |",
        pct(first, resolved)
    );
    let _ = writeln!(
        o,
        "| lapses per principal and week | {:.3} ({} lapses, {} principals, {:.1} weeks); at most {max_ppw} for one first addressee in one week; {to_user_lapses} lapsed with `user` as the only stage | none stated; no red share from questions | reported |",
        lapses_in.len() as f64 / (principals * weeks),
        lapses_in.len(),
        p.principals,
        weeks
    );
    let _ = writeln!(
        o,
        "| settlements that reach the wrong principal | {settle_wrong} of {} settlements ({answer_wrong} answers; {} misaddressed reports were injected and none answered) | nothing misdirected | {} |",
        obs.n.settled,
        sim.m.mistakes_reports,
        if settle_wrong + answer_wrong == 0 {
            "met"
        } else {
            "**contradicted**"
        }
    );
    let _ = writeln!(
        o,
        "| digest entries that reach the wrong principal | {digest_aud_wrong} addressed outside the item's requesters, askees, and owners; {digest_shown_wrong} shown to another person ({} misaddressed showings injected); of {} entries made in the measurement, {digest_user} for `user`, {digest_shown} shown | nothing misdirected | {} |",
        sim.m.mistakes_digest,
        digests_in.len(),
        if digest_aud_wrong + digest_shown_wrong == 0 {
            "met"
        } else {
            "**contradicted**"
        }
    );
    let _ = writeln!(
        o,
        "| `health`'s red share | {health_cell} | no red share from questions | reported |"
    );
    let _ = writeln!(
        o,
        "| `hello`'s size | {hello_cell} | within `hello.budget`, at most 5 items in each user section (D.15.2) | {} |\n",
        if sim
            .m
            .hellos
            .iter()
            .all(|h| h.decision_items <= 5 && h.digest_items <= 5)
        {
            "met for the items; bytes are a proxy"
        } else {
            "**contradicted**"
        }
    );

    let _ = writeln!(o, "### Questions for the user asked in the measurement\n");
    let _ = writeln!(o, "| Outcome | Count | Share |\n|---|---|---|");
    for (k, v) in [
        ("answered by the first addressee", first),
        ("answered by a later stage's addressee", later),
        ("answered by someone else", other),
        ("defaulted", defaulted),
        ("lapsed", lapsed),
        ("still pending at the end", pending),
    ] {
        let _ = writeln!(o, "| {k} | {v} | {} |", pct(v, n));
    }
    let _ = writeln!(
        o,
        "| total ({with_default} with a default; the first addressee was the requester in {first_is_requester}) | {n} | |"
    );
    let _ = writeln!(
        o,
        "\nPending questions per person: at most {} at one hourly sample; {} person-samples above `user_question.max_pending` ({}), and {} `user_saturated` warnings. Open work nodes with no mover: {} seen (at most {} at once); waiting nodes older than `capacity.window`: {}.",
        sim.m.max_pending_person,
        sim.m.saturated_samples,
        sim.f.s.params.user_question_max_pending,
        sim.f.warnings.get("UserSaturated").copied().unwrap_or(0),
        sim.m.parked.len(),
        sim.m.parked_peak,
        sim.m.waited_long.len()
    );
    let _ = writeln!(
        o,
        "\nWithout a default: {} answered by the first addressee ({first_no_default} of {}).\n",
        pct(first_no_default, n - with_default),
        n - with_default
    );
    let _ = writeln!(o, "By the first addressee's message rate (D.15.4's mix):\n");
    let _ = writeln!(
        o,
        "| Rate | Questions | Answered by the first addressee |\n|---|---|---|"
    );
    for (r, (k, a)) in &by_rate {
        let _ = writeln!(o, "| {} | {k} | {} |", r.name(), pct(*a, *k));
    }
    let _ = writeln!(o);
    // How long the questions asked in the measurement took to reach their fate.
    let mut fate: Vec<u64> = qs
        .iter()
        .filter_map(|(_, u)| u.resolved_at.map(|t| t.since(u.created).0))
        .collect();
    fate.sort_unstable();
    let at = |p: usize| -> Dur {
        if fate.is_empty() {
            Dur(0)
        } else {
            Dur(fate[((fate.len() * p).div_ceil(100)).max(1) - 1])
        }
    };
    let _ = writeln!(
        o,
        "Time from a question to its fate (answer, default, or lapse): median {}, p90 {}, max {}. The longest wait of a waiting node at an hourly sample: {}.\n",
        days(at(50)),
        days(at(90)),
        days(at(100)),
        days(sim.m.max_wait)
    );
    let _ = writeln!(
        o,
        "Daily state of the measurement (open tasks, candidates, waiting nodes, pending questions, pending digest entries, of them for `user`):\n"
    );
    daily(o, sim);
    // The longest wait, with its requester's rate and its trace.
    let longest = sim
        .m
        .waited_long
        .iter()
        .max_by_key(|&(n, d)| (*d, std::cmp::Reverse(*n)));
    if let Some((&n, &d)) = longest {
        let who = sim
            .tasks
            .get(&n)
            .and_then(|t| t.requester)
            .map(|i| &sim.people[i]);
        let _ = writeln!(
            o,
            "Waiting nodes older than `capacity.window`: {} (every such wait is on a pending question, which has its deadline). The longest, #{}, waited {}; its requester writes {}{}.",
            sim.m.waited_long.len(),
            n.0,
            days(d),
            who.map_or("(none)", |p| p.rate.name()),
            if who.is_some_and(|p| p.never_answers) {
                " and never answers"
            } else {
                ""
            }
        );
        let mut by: BTreeMap<String, u64> = BTreeMap::new();
        for n in sim.m.waited_long.keys() {
            let k = match sim
                .tasks
                .get(n)
                .and_then(|t| t.requester)
                .map(|i| &sim.people[i])
            {
                Some(p) if p.never_answers => format!("{}, never answers", p.rate.name()),
                Some(p) => p.rate.name().to_string(),
                None => "no requester".to_string(),
            };
            *by.entry(k).or_default() += 1;
        }
        let by: Vec<String> = by.iter().map(|(k, v)| format!("{v} {k}")).collect();
        let _ = writeln!(o, "By the requester's message rate: {}.", by.join(", "));
        trace(o, sim, n);
        let _ = writeln!(o);
    }
}

#[allow(clippy::too_many_lines, reason = "one profile's table and its details")]
fn no_user(o: &mut String, sim: &Sim, health_cell: &str, hello_cell: &str) {
    let obs = &sim.f.obs;
    let s = &sim.f.s;
    let now = sim.f.now;
    let from = sim.measure_from;
    let window = s.params.capacity_window;
    // Waiting at the end.
    let mut by_kind: BTreeMap<&str, (u64, Dur)> = BTreeMap::new();
    for n in &s.ix.next.waiting {
        if let Some(w) = s.nodes.get(n).and_then(|x| x.wait.as_ref()) {
            let k = match w.on {
                forum_core::WaitOn::Question { .. } => "a people question",
                forum_core::WaitOn::UserQuestion(_) => "a question for the user",
                forum_core::WaitOn::Node(_) => "a node",
                forum_core::WaitOn::Time(_) => "a time",
                forum_core::WaitOn::External(_) => "something outside",
            };
            let e = by_kind.entry(k).or_default();
            e.0 += 1;
            e.1 = e.1.max(now.since(w.set_at));
        }
    }
    let waiting: Vec<String> = by_kind
        .iter()
        .map(|(k, (c, a))| format!("{c} on {k} (oldest {})", days(*a)))
        .collect();
    let pending_uq: Vec<Dur> = s
        .uquestions
        .iter()
        .filter(|(_, u)| u.state == UqState::Pending)
        .filter_map(|(q, _)| obs.uqs.get(q).map(|x| now.since(x.created)))
        .collect();
    let open_pq = s
        .pquestions
        .values()
        .filter(|x| x.state == forum_core::PqState::Open)
        .count();
    let cands = s.ix.next.nodes;
    let oldest_cand =
        s.ix.next
            .of
            .values()
            .map(|c| now.since(c.since))
            .max()
            .unwrap_or(Dur(0));
    let parked_end: Vec<NodeId> = sim
        .m
        .parked
        .keys()
        .copied()
        .filter(|n| {
            s.nodes
                .get(n)
                .is_some_and(|x| matches!(x.life, forum_core::Life::Open))
                && s.ix.node.get(n).is_none_or(|x| x.live == 0)
                && !s.ix.next.of.contains_key(&forum_core::Cand::Node(*n))
                && !s.ix.next.waiting.contains(n)
        })
        .collect();
    // Lapses and their assumptions.
    let lapsed: Vec<&crate::observe::Uq> = obs
        .uqs
        .values()
        .filter(|u| u.outcome == UqOutcome::Lapsed && u.resolved_at.is_some_and(|t| t >= from))
        .collect();
    let lapsed_marked = lapsed.iter().filter(|u| u.marked > 0).count();
    let lapsed_assumed = lapsed.iter().filter(|u| u.assumed).count();
    let lapsed_open = lapsed.iter().filter(|u| !u.assumed).count();
    // Lapsed escalations whose people question was answered late: the answer withdrew the lapse's
    // marks (D.8.15), so no assumption is ever recorded and the user question never finishes.
    let withdrawn = lapsed
        .iter()
        .filter(|u| {
            !u.assumed
                && u.escalates
                    .is_some_and(|q| obs.pqs.get(&q).is_some_and(|x| x.end == Some("answered")))
        })
        .count();
    let e1_failed = sim
        .m
        .violations
        .values()
        .any(|(_, v, _)| matches!(v.inv, forum_core::Inv::E1 | forum_core::Inv::M1));
    let by_src = |k: UqKind| {
        obs.uqs
            .values()
            .filter(|u| u.created >= from && u.kind == k)
            .count()
    };
    let defaulted = obs
        .uqs
        .values()
        .filter(|u| u.created >= from && matches!(u.outcome, UqOutcome::Defaulted { .. }))
        .count();
    let pqs_in: Vec<&crate::observe::Pq> = obs.pqs.values().filter(|q| q.created >= from).collect();
    let pq_end = |e: &str| pqs_in.iter().filter(|q| q.end == Some(e)).count();
    // The digest.
    let digests_in: Vec<&crate::observe::Dg> =
        obs.digests.values().filter(|d| d.created >= from).collect();
    let mut by_item: BTreeMap<&str, u64> = BTreeMap::new();
    for d in &digests_in {
        let k = match d.item {
            forum_core::DigestItem::Assumption { .. } => "assumption",
            forum_core::DigestItem::DefaultApplied { .. } => "default applied",
            forum_core::DigestItem::RefusalDrop { .. } => "refusal drop",
            forum_core::DigestItem::Lapsed { .. } => "lapsed",
        };
        *by_item.entry(k).or_default() += 1;
    }
    let items: Vec<String> = by_item.iter().map(|(k, v)| format!("{v} {k}")).collect();
    let pending_user =
        s.ix.pending_digest_by
            .get(&USER_PRINCIPAL)
            .map_or(0, BTreeSet::len);
    let left_user = s.digest_left.get(&USER_PRINCIPAL).copied().unwrap_or(0);
    let drops_in = obs
        .n
        .refusal_drops
        .iter()
        .filter(|(t, _)| *t >= from)
        .count();

    let _ = writeln!(o, "### Against D.15.4 (no user)\n");
    let _ = writeln!(
        o,
        "| Report | Measured | Expected (D.15.4) | Verdict |\n|---|---|---|---|"
    );
    let _ = writeln!(
        o,
        "| objects still waiting at the end | {} waiting nodes ({}); {} questions for the user pending (oldest {}); {open_pq} people questions open; {cands} candidates (oldest {}); {} unrouted entries | nothing waiting past its chain | {} |",
        s.ix.next.waiting.len(),
        if waiting.is_empty() {
            "none".to_string()
        } else {
            waiting.join(", ")
        },
        pending_uq.len(),
        days(pending_uq.iter().copied().max().unwrap_or(Dur(0))),
        days(oldest_cand),
        s.ix.unrouted.len(),
        if pending_uq.iter().all(|&d| d <= window) {
            "met"
        } else {
            "**contradicted**"
        }
    );
    let _ = writeln!(
        o,
        "| objects that waited longer than `capacity.window` ({}) | {} waiting nodes (longest wait {}); {} candidates offered that long untaken (oldest {}); the longest pending question {} | none (the yellow items no rule moved, D.8.22) | {} |",
        days(window),
        sim.m.waited_long.len(),
        days(sim.m.max_wait),
        sim.m.long_candidates.len(),
        days(sim.m.max_candidate_age),
        days(sim.m.max_uq_pending),
        if sim.m.waited_long.is_empty() && sim.m.long_candidates.is_empty() {
            "met"
        } else {
            "**contradicted**"
        }
    );
    let _ = writeln!(
        o,
        "| nodes parked | {} open work nodes seen with no mover (no hold, no candidate, no unresolved wait), at most {} at once; {} still at the end | no node parked | {} |",
        sim.m.parked.len(),
        sim.m.parked_peak,
        parked_end.len(),
        if sim.m.parked.is_empty() {
            "met"
        } else {
            "**contradicted**"
        }
    );
    let _ = writeln!(
        o,
        "| lapses and their assumptions | {} lapses ({:.1} a day): {lapsed_marked} marked waiting or `about` nodes `proceed_on_assumption`, {lapsed_assumed} assumptions recorded by a decision, {lapsed_open} not recorded at the end, {withdrawn} of them because the person answered the escalated question after the lapse; questions asked: {} by agents, {} escalations, {} refusals; {defaulted} defaulted; people questions: {} asked, {} answered, {} expired, {} escalated | every question ends at the last due of its chain | {} |",
        lapsed.len(),
        lapsed.len() as f64 / sim.p.days.max(1) as f64,
        by_src(UqKind::Agent),
        by_src(UqKind::Escalation),
        by_src(UqKind::Refusal),
        pqs_in.len(),
        pq_end("answered"),
        pq_end("expired"),
        pq_end("escalated"),
        if !pending_uq.iter().all(|&d| d <= window) {
            "**contradicted**"
        } else if e1_failed {
            "met for the questions; **E1 (f) and M1 fail** on a withdrawn lapse (Findings)"
        } else {
            "met"
        }
    );
    let _ = writeln!(
        o,
        "| `health`'s red share | {health_cell} | no red share from questions | reported |"
    );
    let _ = writeln!(
        o,
        "| the digest's size | {} entries made in the measurement ({}), all for `user`: {} for `user`; {pending_user} pending for `user` at the end, {left_user} left the pending list unshown (DIGEST_RETAIN 28 d) | records only (D.15.1) | reported |",
        digests_in.len(),
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        },
        digests_in.iter().filter(|d| d.to == USER_PRINCIPAL).count()
    );
    let _ = writeln!(
        o,
        "| `hello`'s size | {hello_cell} | within budget | {} |",
        if sim
            .m
            .hellos
            .iter()
            .all(|h| h.bytes <= s.params.hello_budget as usize)
        {
            "met (proxy)"
        } else {
            "see the bytes"
        }
    );
    let _ = writeln!(
        o,
        "| refusal drops (E1) | {drops_in} nodes dropped after every active class refused ({:.2} a day) | 0 to 4 a day (10,000-agent profile) | reported |\n",
        drops_in as f64 / sim.p.days.max(1) as f64
    );
    let _ = writeln!(
        o,
        "Daily state of the measurement (open tasks, candidates, waiting nodes, pending questions, pending digest entries, of them for `user`):\n"
    );
    daily(o, sim);
    if !sim.m.parked.is_empty() {
        let _ = writeln!(o, "Parked nodes (first seen):\n");
        for (n, t) in sim.m.parked.iter().take(10) {
            let _ = writeln!(o, "- #{} at {}", n.0, stamp(*t));
        }
        for n in sim.m.parked.keys().take(2) {
            trace(o, sim, *n);
        }
        let _ = writeln!(o);
    }
    if !sim.m.waited_long.is_empty() || !sim.m.long_candidates.is_empty() {
        let _ = writeln!(o, "Objects older than `capacity.window`:\n");
        for (n, d) in sim.m.waited_long.iter().take(10) {
            let _ = writeln!(o, "- waiting node #{}: {}", n.0, days(*d));
        }
        for (n, d) in sim.m.long_candidates.iter().take(10) {
            let _ = writeln!(o, "- candidate #{}: {}", n.0, days(*d));
        }
        if let Some(n) = sim
            .m
            .waited_long
            .keys()
            .chain(sim.m.long_candidates.keys())
            .next()
        {
            trace(o, sim, *n);
        }
        let _ = writeln!(o);
    }
}

fn daily(o: &mut String, sim: &Sim) {
    let _ = writeln!(
        o,
        "| Day | Open tasks | Candidates | Waiting | Pending questions | Pending digest | of them `user` |\n|---|---|---|---|---|---|---|"
    );
    for (i, d) in sim.m.daily.iter().enumerate() {
        if i % 3 == 0 || i + 1 == sim.m.daily.len() {
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | {} |",
                i, d[0], d[1], d[2], d[3], d[4], d[5]
            );
        }
    }
    let _ = writeln!(o);
}
