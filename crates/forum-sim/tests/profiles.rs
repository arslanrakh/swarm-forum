//! The profiles end to end, small: a run repeats with its seed, the simulated agents exercise what
//! the reports read, and the core's guarantees that the reports check hold on a short horizon.

use forum_sim::observe::{UqKind, UqOutcome};
use forum_sim::profile::{Kind, Profile};
use forum_sim::report;

/// A company of 24 principals (3 teams), 2 days of warm-up, 3 measured days, 2 of cool-down.
fn small_company(seed: u64) -> Profile {
    let mut p = Profile::company(3, seed);
    p.principals = 24;
    p.warmup_days = 2;
    p.cooldown_days = 2;
    p
}

fn small_no_user(seed: u64) -> Profile {
    let mut p = Profile::no_user(4, seed);
    p.jobs_per_day = 30;
    p
}

#[test]
fn a_run_repeats_with_its_seed() {
    let a = report::render(&forum_sim::run(small_company(7)));
    let b = report::render(&forum_sim::run(small_company(7)));
    assert_eq!(a, b);
    let c = report::render(&forum_sim::run(small_company(8)));
    assert_ne!(a, c);
    let x = report::render(&forum_sim::run(small_no_user(7)));
    let y = report::render(&forum_sim::run(small_no_user(7)));
    assert_eq!(x, y);
}

#[test]
fn the_company_profile_exercises_its_reports() {
    let sim = forum_sim::run(small_company(3));
    let obs = &sim.f.obs;
    assert_eq!(sim.p.kind, Kind::Company);
    assert!(sim.people.len() >= 24);
    assert_eq!(sim.teams.len(), 3);
    assert!(sim.m.tasks_created > 0, "no work");
    assert!(obs.n.settled > 0, "nothing settled");
    assert!(!obs.uqs.is_empty(), "no question for the user");
    assert!(
        obs.uqs
            .values()
            .any(|u| matches!(u.outcome, UqOutcome::Answered { .. })),
        "nobody answered"
    );
    assert!(sim.m.drops > 0, "no session vanished");
    assert!(!sim.m.hellos.is_empty());
    assert!(
        sim.m
            .hellos
            .iter()
            .all(|h| h.decision_items <= 5 && h.digest_items <= 5)
    );
    // Every question's first addressee is its requester, unless the requester is gone.
    for u in obs.uqs.values() {
        assert!(!u.to.is_empty());
        assert_eq!(u.to.last(), Some(&forum_core::USER_PRINCIPAL));
    }
    assert!(obs.findings.is_empty(), "{:?}", obs.findings);
    assert!(sim.m.violations.is_empty(), "{:?}", sim.m.violations);
    assert!(sim.f.stuck.is_empty());
}

#[test]
fn reports_to_the_wrong_principal_answer_and_settle_nothing() {
    let mut p = small_company(5);
    // Every report and every digest showing goes to another principal.
    p.mistake = 1_000_000;
    let sim = forum_sim::run(p);
    let obs = &sim.f.obs;
    assert!(sim.m.mistakes_reports > 0);
    assert_eq!(obs.n.misaddressed_reports, sim.m.mistakes_reports);
    // No user request settles: none was ever reported to its requester.
    assert_eq!(
        obs.n.settled, 0,
        "a request settled by a report to someone else"
    );
    assert!(obs.findings.is_empty(), "{:?}", obs.findings);
    assert_eq!(
        sim.f.warnings.get("ResultAnsweredNothing").copied(),
        Some(sim.m.mistakes_reports)
    );
}

#[test]
fn the_no_user_profile_has_only_user_and_nothing_waits_for_ever() {
    let sim = forum_sim::run(small_no_user(2));
    let obs = &sim.f.obs;
    assert_eq!(sim.p.kind, Kind::NoUser);
    assert!(sim.people.is_empty());
    // Every principal the forum knows is `user`.
    assert!(obs.person.values().all(Option::is_none));
    assert!(
        obs.uqs
            .values()
            .all(|u| u.to == vec![forum_core::USER_PRINCIPAL])
    );
    assert!(
        obs.digests
            .values()
            .all(|d| d.to == forum_core::USER_PRINCIPAL)
    );
    assert!(sim.m.tasks_created > 0);
    assert!(obs.n.settled > 0);
    // Nobody serves `user`, so every question ends within its chain's one stage, which is
    // `due_absent` long; a refusal question keeps the long due (D.8.12).
    let due_absent = sim.f.s.params.user_question_due_absent;
    for u in obs.uqs.values().filter(|u| u.kind != UqKind::Refusal) {
        if let Some(t) = u.resolved_at {
            assert!(t.since(u.created) <= due_absent, "{u:?}");
        }
    }
    assert!(sim.m.parked.is_empty(), "{:?}", sim.m.parked);
    assert!(sim.m.waited_long.is_empty());
    assert!(obs.findings.is_empty(), "{:?}", obs.findings);
}

/// Guided agents (D.15.4) follow `now` items at once and keep their bookkeeping: before the fix, a
/// followed call came a step of work late, past the first lease, and its split's part was never
/// worked, so the guided company opened 40 times the unguided worker sessions.
#[test]
fn guided_agents_work_like_unguided_ones() {
    let worker_sessions =
        |sim: &forum_sim::sim::Sim| sim.m.sessions.get("worker").copied().unwrap_or(0);
    for (plain, mut guided) in [
        (small_company(3), small_company(3)),
        (small_no_user(3), small_no_user(3)),
    ] {
        guided.p_follow = 900_000;
        let plain = forum_sim::run(plain);
        let guided = forum_sim::run(guided);
        assert!(guided.m.follows > 0);
        let (a, b) = (worker_sessions(&plain), worker_sessions(&guided));
        assert!(b * 4 <= a * 5, "{b} worker sessions guided, {a} unguided");
        assert!(guided.f.obs.n.holds * 4 <= plain.f.obs.n.holds * 5);
        // No followed call is refused for a hold that ran out while the agent worked.
        assert!(
            guided.m.followed.keys().all(|(_, _, o)| o == "made"),
            "{:?}",
            guided.m.followed
        );
        assert!(guided.m.long_candidates.is_empty());
        assert!(
            guided.f.obs.findings.is_empty(),
            "{:?}",
            guided.f.obs.findings
        );
        assert!(guided.m.violations.is_empty(), "{:?}", guided.m.violations);
    }
}

#[test]
fn half_of_the_worker_sessions_vanish_mid_task() {
    let sim = forum_sim::run(small_no_user(4));
    let holds = sim.f.obs.n.holds as f64;
    let expired = sim.f.obs.n.expired as f64;
    // A vanished session's hold expires; about half the sessions vanish during their first task.
    let share = expired / holds;
    assert!((0.3..0.6).contains(&share), "{share}");
}
