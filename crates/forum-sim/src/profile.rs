//! The profiles of D.15.4 in their light form: every rate is an integer (parts per million, minutes,
//! or milliseconds), so nothing the core sees comes from a float.

use forum_core::{DAY, Dur, HOUR, MINUTE};

/// Which profile of D.15.4.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    /// Principals with the review's message rates, teams of 8 sharing classes.
    Company,
    /// No principal but `user`, a worker class, at 1.1's production profile.
    NoUser,
}

impl Kind {
    /// The name on the command line.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Kind::Company => "company",
            Kind::NoUser => "no-user",
        }
    }

    /// Parses the name on the command line.
    #[must_use]
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "company" => Some(Kind::Company),
            "no-user" | "nouser" | "no_user" => Some(Kind::NoUser),
            _ => None,
        }
    }
}

/// How often a principal writes (review §4's mix, D.15.4).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Rate {
    /// Several times a day: 4 a day on average.
    SeveralDaily,
    /// About once a day.
    Daily,
    /// About once a week.
    Weekly,
    /// About once a month.
    Rarely,
    /// About once a quarter.
    AlmostNever,
}

impl Rate {
    /// Every rate, in the order of D.15.4's mix.
    pub const ALL: [Rate; 5] = [
        Rate::SeveralDaily,
        Rate::Daily,
        Rate::Weekly,
        Rate::Rarely,
        Rate::AlmostNever,
    ];

    /// The mean gap between two messages: the writes are a Poisson process.
    #[must_use]
    pub fn mean_gap(self) -> Dur {
        match self {
            Rate::SeveralDaily => Dur(6 * HOUR.0),
            Rate::Daily => DAY,
            Rate::Weekly => Dur(7 * DAY.0),
            Rate::Rarely => Dur(30 * DAY.0),
            Rate::AlmostNever => Dur(90 * DAY.0),
        }
    }

    /// The label in the report.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Rate::SeveralDaily => "several a day",
            Rate::Daily => "daily",
            Rate::Weekly => "weekly",
            Rate::Rarely => "rarely",
            Rate::AlmostNever => "almost never",
        }
    }
}

/// One run's configuration.
#[derive(Clone, Debug)]
pub struct Profile {
    /// Which profile.
    pub kind: Kind,
    /// The measured days.
    pub days: u64,
    /// Days run before the measurement, so that every principal has a message history within
    /// `user_gap.window` (28 days) when it starts: the p90 gap and absence read that history.
    pub warmup_days: u64,
    /// Days run after the measurement, so that the questions asked within it reach their fate (a
    /// chain can take weeks); everything else is measured within the measured days only.
    pub cooldown_days: u64,
    /// The seed.
    pub seed: u64,
    /// The probability, in ppm, that an agent makes the first `now` next call of its last
    /// response instead of its own next action (D.15.4's guided profile).
    pub p_follow: u32,
    /// Run `check_invariants` every this many simulated hours (0: never).
    pub check_every_hours: u64,

    // ---- company
    /// Principals at the start.
    pub principals: usize,
    /// Persons per team.
    pub team_size: usize,
    /// Agent classes, shared by every team.
    pub classes: usize,
    /// The mix of message rates, in permille, in the order of `Rate::ALL`.
    pub rate_mix: [u32; 5],
    /// Principals who never answer a question, in permille (drawn independently of the rate).
    pub never_answer: u32,
    /// Principals who leave, per year, in permille; scaled to the run by a daily hazard.
    pub leave_per_year: u32,
    /// A leaver whom an agent marks `gone` in the directory, in ppm, after 1 to 7 days.
    pub gone_mark: u32,
    /// A leaver is replaced by a joiner in the same team, after 0 to 14 days.
    pub replace_leavers: bool,
    /// Principals who write from two clients, as two aliases of one person, in ppm.
    pub alias: u32,
    /// A message that is a new request, in ppm; the others answer, check in, or both.
    pub request: u32,
    /// A visit that shows the pending digest entries to the principal, in ppm.
    pub show_digest: u32,

    // ---- workers, both profiles
    /// A worker session that vanishes mid-task (D.15.4: half of all sessions), in ppm.
    pub drop: u32,
    /// A task that needs a question for a human, in ppm.
    pub ask_user: u32,
    /// Such a question that carries a default, in ppm.
    pub ask_default: u32,
    /// A take answered with a spurious `cannot` (D.15.4: 0.2 to 3 %), in ppm.
    pub cannot: u32,
    /// A report or a digest showing addressed to the wrong principal by an agent's mistake, in ppm:
    /// the core must not let it answer, settle, or show anything (CS-4, CS-5).
    pub mistake: u32,
    /// A finished worker that asks `next` for more work, in ppm.
    pub continue_after: u32,
    /// A task's estimate, in minutes: uniform in this range.
    pub est_min: (u64, u64),
    /// The time between two checkpoints, in minutes: within the first lease (15 minutes).
    pub step_min: (u64, u64),
    /// How often the spawner looks for unattended work.
    pub spawn_every: Dur,
    /// Workers started per team (or per forum, with no user) and spawner tick, at most.
    pub spawn_max: u64,

    // ---- no user
    /// Jobs a scheduler hands to the swarm per day (as external requests).
    pub jobs_per_day: u64,
    /// Sessions that wake up and ask for work whatever the backlog, per day.
    pub polls_per_day: u64,
    /// A task that asks a person outside the swarm in its conversation, in ppm.
    pub ask_person: u32,
    /// Such a question that is blocking (escalates to the user), in ppm.
    pub person_blocking: u32,
    /// The person answers at all, in ppm.
    pub person_answers: u32,
    /// The mean delay of the person's answer.
    pub person_delay: Dur,
    /// A task that splits off an offered sub-task, in ppm.
    pub split: u32,
}

impl Profile {
    /// The company profile of D.15.4 at 200 principals.
    #[must_use]
    pub fn company(days: u64, seed: u64) -> Profile {
        Profile {
            kind: Kind::Company,
            days,
            warmup_days: 28,
            cooldown_days: 28,
            seed,
            p_follow: 0,
            check_every_hours: 24,
            principals: 200,
            team_size: 8,
            classes: 20,
            rate_mix: [300, 350, 200, 100, 50],
            never_answer: 100,
            leave_per_year: 150,
            gone_mark: 500_000,
            replace_leavers: true,
            alias: 100_000,
            request: 500_000,
            show_digest: 900_000,
            drop: 500_000,
            ask_user: 300_000,
            ask_default: 500_000,
            cannot: 10_000,
            mistake: 20_000,
            continue_after: 500_000,
            est_min: (20, 180),
            step_min: (8, 12),
            spawn_every: Dur(5 * MINUTE.0),
            spawn_max: 2,
            jobs_per_day: 21,
            polls_per_day: 6,
            ask_person: 0,
            person_blocking: 0,
            person_answers: 0,
            person_delay: Dur(0),
            split: 0,
        }
    }

    /// The no-user profile of D.15.4 at 1.1's production profile (about 25,000 sessions a year,
    /// about 68 a day).
    #[must_use]
    pub fn no_user(days: u64, seed: u64) -> Profile {
        Profile {
            kind: Kind::NoUser,
            warmup_days: 0,
            cooldown_days: 0,
            principals: 0,
            team_size: 0,
            classes: 1,
            rate_mix: [0; 5],
            never_answer: 0,
            leave_per_year: 0,
            gone_mark: 0,
            replace_leavers: false,
            alias: 0,
            request: 0,
            show_digest: 0,
            jobs_per_day: 21,
            polls_per_day: 6,
            ask_person: 200_000,
            person_blocking: 500_000,
            person_answers: 500_000,
            person_delay: Dur(6 * HOUR.0),
            split: 100_000,
            ..Profile::company(days, seed)
        }
    }

    /// The profile `kind` with its defaults.
    #[must_use]
    pub fn of(kind: Kind, days: u64, seed: u64) -> Profile {
        match kind {
            Kind::Company => Profile::company(days, seed),
            Kind::NoUser => Profile::no_user(days, seed),
        }
    }

    /// The number of teams.
    #[must_use]
    pub fn teams(&self) -> usize {
        if self.team_size == 0 {
            0
        } else {
            self.principals.div_ceil(self.team_size)
        }
    }

    /// The daily hazard of leaving, in ppm: `leave_per_year / 365` (a year's 15 % scaled to a day).
    #[must_use]
    pub fn leave_per_day(&self) -> u32 {
        // permille per year to ppm per day
        self.leave_per_year * 1_000 / 365
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_company_mix_is_d_15_4s() {
        let p = Profile::company(30, 1);
        assert_eq!(p.rate_mix.iter().sum::<u32>(), 1_000);
        assert_eq!(p.rate_mix, [300, 350, 200, 100, 50]);
        assert_eq!(p.never_answer, 100);
        assert_eq!(p.leave_per_year, 150);
        assert_eq!(p.teams(), 25);
        assert_eq!(p.team_size, 8);
        // 15 % a year is 411 ppm a day: about 2.5 of 200 principals leave in 30 days.
        assert_eq!(p.leave_per_day(), 410);
        assert_eq!(p.drop, 500_000);
    }

    #[test]
    fn the_no_user_profile_has_no_principal() {
        let p = Profile::no_user(30, 1);
        assert_eq!(p.kind, Kind::NoUser);
        assert_eq!(p.principals, 0);
        assert_eq!(p.teams(), 0);
        assert_eq!(p.classes, 1);
        assert!(p.jobs_per_day > 0);
        assert_eq!(Kind::parse("no-user"), Some(Kind::NoUser));
        assert_eq!(Kind::parse("company"), Some(Kind::Company));
        assert_eq!(Kind::parse("x"), None);
    }

    #[test]
    fn rates_are_ordered_by_gap() {
        let gaps: Vec<u64> = Rate::ALL.iter().map(|r| r.mean_gap().0).collect();
        assert!(gaps.windows(2).all(|w| w[0] < w[1]));
    }
}
