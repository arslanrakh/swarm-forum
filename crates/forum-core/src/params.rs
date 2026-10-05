//! Parameters (design D.3, §22).
//!
//! `Params` is part of the state, recorded by `Genesis` and by `ParamsChanged` (D.7), so replay never
//! depends on the running configuration. Shell-only parameters live in the same struct, so the
//! constitution is rendered from one place.

use alloc::collections::BTreeSet;

use crate::types::{DAY, Dur, HOUR, Label, MINUTE, SECOND};

/// The parameters of §22. Each field names its §22 parameter and default value.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Params {
    // holds (§10.1)
    /// `lease.first`: 15 min.
    pub lease_first: Dur,
    /// `lease.default`: 20 min.
    pub lease_default: Dur,
    /// `lease.max`: 2 h.
    pub lease_max: Dur,
    /// `lease.ceiling`: 8 h.
    pub lease_ceiling: Dur,
    /// `renew.max`: 3.
    pub renew_max: u32,
    /// `hold.per_session`: 2.
    pub hold_per_session: u32,
    // sessions (§3)
    /// `presence.silence`: 15 min.
    pub presence_silence: Dur,
    /// `presence.resolution`: 3 min; a call refreshes presence at most this often (CS-14).
    pub presence_resolution: Dur,
    /// `session.retain`: 24 h; then a silent session retires (CS-8).
    pub session_retain: Dur,
    // retries (§20.1, O1)
    /// `op.retain`: 24 h; how long an `op_id` record lives (CS-8).
    pub op_retain: Dur,
    // keys (§7, §9.1)
    /// `conversation.schemes`: `{chat, issue, mail}` (CS-36).
    pub conversation_schemes: BTreeSet<Label>,
    // people (§9)
    /// `commitment.followup`: 90 min.
    pub commitment_followup: Dur,
    /// `question.recheck`: 4 h.
    pub question_recheck: Dur,
    /// `question.max_rechecks`: 2.
    pub question_max_rechecks: u32,
    // principals and questions for them (§8.4, CS-2, CS-33)
    /// `user_question.due`: 72 h, raised to the addressee's p90 gap at a question's first stage.
    pub user_question_due: Dur,
    /// `user_question.due_absent`: 1 h, for an absent addressee (CS-2).
    pub user_question_due_absent: Dur,
    /// `user_question.max_pending`: 20 per principal (CS-33).
    pub user_question_max_pending: u32,
    /// `user_gap.window`: 28 days.
    pub user_gap_window: Dur,
    // (no forum-default addressee: the forum's owners are ROOT's `owners`, forum data that agents
    //  keep, D.2.2, D-3, FQ-5)
    // agent communication (§13)
    /// `deliver.window`: 24 h.
    pub deliver_window: Dur,
    /// `talk.wait_max`: 50 s (shell).
    pub talk_wait_max: Dur,
    // budgets (§10.4, §20.3, §5)
    /// `hello.budget`: 4,096 bytes (shell).
    pub hello_budget: u32,
    /// `brief.budget`: 16,384 bytes (shell).
    pub brief_budget: u32,
    /// `digest.max`: 2,048 bytes (the core validates).
    pub digest_max: u32,
    /// `leave_better.max`: 3.
    pub leave_better_max: u32,
    // refusals (§10.3)
    /// `refusal.ttl`: 7 days.
    pub refusal_ttl: Dur,
    /// `refusal.window`: 7 days.
    pub refusal_window: Dur,
    /// `refusal.backoff`: 1 h; one session's `cannot` (CS-7).
    pub refusal_backoff: Dur,
    /// `refusal.quorum`: 3 sessions make their class refuse (CS-7).
    pub refusal_quorum: u32,
    /// `too_big.attempts`: 3.
    pub too_big_attempts: u32,
    // pause, archive (§19, §5)
    /// `pause.ttl`: 2 h.
    pub pause_ttl: Dur,
    /// `archive.after`: 7 days.
    pub archive_after: Dur,
    // recurring work and capacity (§12, §17)
    /// `coverage.window`: 28 days.
    pub coverage_window: Dur,
    /// `capacity.window`: 7 days.
    pub capacity_window: Dur,
    // safety valves (§19)
    /// `write.per_hour`: 120 per session (shell).
    pub write_per_hour: u32,
    /// `queue.max_wait`: 10 s; then `limit(Busy)` (shell, CS-29).
    pub queue_max_wait: Dur,
    /// `text.max`: 65,536 bytes per free-text argument, after masking; then `limit(TextTooLong)`
    /// (the shell answers, `apply` repeats the check; L1, A-114).
    pub text_max: u32,
    /// `tree.max_depth`: 16.
    pub tree_max_depth: u32,
    /// `tree.max_children`: 500 open agent-made children (CS-10), persons and teams not counted
    /// (A-113).
    pub tree_max_children: u32,
    /// `settle.batch`: 1,000 deadlines per transaction (shell).
    pub settle_batch: u32,
    /// `instructions.max`: 512 characters (shell).
    pub instructions_max: u32,
    // tidying (§14, 1.8)
    /// `offer.stale`: 90 days; an offer that nothing touched this long carries `stale`. `None` only
    /// in parameters recorded before 1.8, which read without it (D.7.6): there `evolve` tracks no
    /// node's touch, so a history written before 1.8 replays to the state it wrote, and the first
    /// `ParamsChanged` of a 1.8 server turns the tracking on. `valid` requires it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub offer_stale: Option<Dur>,
}

/// `offer.stale`'s default (§22, 1.8): 90 days.
#[must_use]
pub const fn default_offer_stale() -> Dur {
    times(90, DAY)
}

impl Params {
    /// When a node touched at `touched` goes stale (1.8, §14): `touched + offer.stale`, None
    /// without a touch or without `offer.stale` (D.7.6).
    #[must_use]
    pub fn stale_at(&self, touched: Option<crate::types::Time>) -> Option<crate::types::Time> {
        touched.zip(self.offer_stale).map(|(t, d)| t.plus(d))
    }
}

/// Builds a duration from a count of a unit, saturating.
const fn times(n: u64, unit: Dur) -> Dur {
    Dur(n.saturating_mul(unit.0))
}

impl Default for Params {
    /// The default values of §22.
    fn default() -> Params {
        Params {
            lease_first: times(15, MINUTE),
            lease_default: times(20, MINUTE),
            lease_max: times(2, HOUR),
            lease_ceiling: times(8, HOUR),
            renew_max: 3,
            hold_per_session: 2,
            presence_silence: times(15, MINUTE),
            presence_resolution: times(3, MINUTE),
            session_retain: times(24, HOUR),
            op_retain: times(24, HOUR),
            conversation_schemes: ["chat", "issue", "mail"]
                .into_iter()
                .filter_map(Label::new)
                .collect(),
            commitment_followup: times(90, MINUTE),
            question_recheck: times(4, HOUR),
            question_max_rechecks: 2,
            user_question_due: times(72, HOUR),
            user_question_due_absent: times(1, HOUR),
            user_question_max_pending: 20,
            user_gap_window: times(28, DAY),
            deliver_window: times(24, HOUR),
            talk_wait_max: times(50, SECOND),
            hello_budget: 4_096,
            brief_budget: 16_384,
            digest_max: 2_048,
            leave_better_max: 3,
            refusal_ttl: times(7, DAY),
            refusal_window: times(7, DAY),
            refusal_backoff: times(1, HOUR),
            refusal_quorum: 3,
            too_big_attempts: 3,
            pause_ttl: times(2, HOUR),
            archive_after: times(7, DAY),
            coverage_window: times(28, DAY),
            capacity_window: times(7, DAY),
            write_per_hour: 120,
            queue_max_wait: times(10, SECOND),
            text_max: 65_536,
            tree_max_depth: 16,
            tree_max_children: 500,
            settle_batch: 1_000,
            instructions_max: 512,
            offer_stale: Some(default_offer_stale()),
        }
    }
}

/// The schemes `conversation.schemes` may never contain: a user entry, an occurrence, and a person
/// name no conversation (§7, §9.1, D.2.2).
const NOT_CONVERSATIONS: [&str; 3] = ["user", "repeat", "person"];

impl Params {
    /// Every duration parameter, which must each be at least one second (A-68).
    fn durations(&self) -> [Dur; 24] {
        [
            self.lease_first,
            self.lease_default,
            self.lease_max,
            self.lease_ceiling,
            self.presence_silence,
            self.presence_resolution,
            self.session_retain,
            self.op_retain,
            self.commitment_followup,
            self.question_recheck,
            self.user_question_due,
            self.user_question_due_absent,
            self.user_gap_window,
            self.deliver_window,
            self.talk_wait_max,
            self.refusal_ttl,
            self.refusal_window,
            self.refusal_backoff,
            self.pause_ttl,
            self.archive_after,
            self.coverage_window,
            self.capacity_window,
            self.queue_max_wait,
            // 1.8: required by `valid`; a missing one reads as 0, which `valid` refuses.
            self.offer_stale.unwrap_or(Dur(0)),
        ]
    }

    /// Every count that must be at least 1 (`renew.max` may be 0).
    fn counts(&self) -> [u32; 11] {
        [
            self.hold_per_session,
            self.question_max_rechecks,
            self.too_big_attempts,
            self.leave_better_max,
            self.tree_max_children,
            self.settle_batch,
            self.write_per_hour,
            self.digest_max,
            self.instructions_max,
            self.user_question_max_pending,
            self.refusal_quorum,
        ]
    }

    /// The ranges and orders every consumer relies on (D.3, §22's rules for parameters). Genesis
    /// and `AdminCmd::SetParams` refuse params that fail it.
    #[must_use]
    pub fn valid(&self) -> bool {
        let durations = self.durations().iter().all(|d| *d >= SECOND);
        let leases = self.lease_first <= self.lease_default
            && self.lease_default <= self.lease_max
            && self.lease_max <= self.lease_ceiling;
        let counts = self.counts().iter().all(|c| *c >= 1);
        let user_questions = self.user_question_due_absent <= self.user_question_due;
        let refusals = self.refusal_backoff <= self.refusal_ttl;
        let presence = self.presence_resolution.0.saturating_mul(3) <= self.presence_silence.0;
        let retirement =
            self.session_retain > self.lease_ceiling && self.session_retain > self.presence_silence;
        let schemes = !self
            .conversation_schemes
            .iter()
            .any(|l| NOT_CONVERSATIONS.contains(&l.as_str()));
        let depth = self.tree_max_depth >= 3;
        // A-114: a digest within `digest.max` is never refused as too long.
        let text = self.text_max >= self.digest_max;
        let budgets = self.hello_budget >= 512 && self.brief_budget >= 1_024;
        durations
            && leases
            && counts
            && user_questions
            && refusals
            && presence
            && retirement
            && schemes
            && depth
            && text
            && budgets
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    reason = "test code: the deny list guards the crate's own code"
)]
mod tests {
    use super::*;

    fn with(f: impl FnOnce(&mut Params)) -> Params {
        let mut p = Params::default();
        f(&mut p);
        p
    }

    #[test]
    fn the_defaults_of_section_22_are_valid() {
        let p = Params::default();
        assert!(p.valid());
        assert_eq!(p.lease_first, Dur(900_000));
        assert_eq!(p.user_question_due, Dur(259_200_000));
        assert_eq!(p.text_max, 65_536);
        assert_eq!(p.digest_max, 2_048);
        assert_eq!(
            p.conversation_schemes
                .iter()
                .map(Label::as_str)
                .collect::<alloc::vec::Vec<_>>(),
            ["chat", "issue", "mail"]
        );
    }

    #[test]
    fn text_max_is_at_least_digest_max() {
        assert!(with(|p| p.text_max = p.digest_max).valid());
        assert!(!with(|p| p.text_max = p.digest_max - 1).valid());
        assert!(
            with(|p| {
                p.digest_max = 32;
                p.text_max = 64;
            })
            .valid()
        );
    }

    #[test]
    fn each_rule_refuses() {
        let bad: [fn(&mut Params); 23] = [
            |p| p.lease_first = Dur(999),
            |p| p.offer_stale = Some(Dur(999)),
            |p| p.offer_stale = None,
            |p| p.queue_max_wait = Dur(0),
            |p| p.lease_first = p.lease_default.plus_one(),
            |p| p.lease_default = p.lease_max.plus_one(),
            |p| p.lease_max = p.lease_ceiling.plus_one(),
            |p| p.hold_per_session = 0,
            |p| p.refusal_quorum = 0,
            |p| p.digest_max = 0,
            |p| p.instructions_max = 0,
            |p| p.user_question_due_absent = p.user_question_due.plus_one(),
            |p| p.refusal_backoff = p.refusal_ttl.plus_one(),
            |p| p.presence_resolution = Dur(p.presence_silence.0 / 3 + 1),
            |p| p.session_retain = p.lease_ceiling,
            |p| p.session_retain = p.presence_silence,
            |p| {
                p.conversation_schemes.insert(Label::new("user").unwrap());
            },
            |p| {
                p.conversation_schemes.insert(Label::new("repeat").unwrap());
            },
            |p| {
                p.conversation_schemes.insert(Label::new("person").unwrap());
            },
            |p| p.tree_max_depth = 2,
            |p| p.hello_budget = 511,
            |p| p.brief_budget = 1_023,
            |p| p.text_max = 0,
        ];
        for (i, f) in bad.into_iter().enumerate() {
            assert!(!with(f).valid(), "rule {i}");
        }
        assert!(with(|p| p.renew_max = 0).valid());
        assert!(with(|p| p.presence_resolution = Dur(p.presence_silence.0 / 3)).valid());
    }

    /// One setter per duration parameter, each to 999 ms, below §22's "every duration is at least
    /// 1 s" (A-68), which T4 and `check_fire` rely on (D.4.9).
    const SHORT: Dur = Dur(999);
    /// A parameter by name, and how to set it out of range.
    type Setter = (&'static str, fn(&mut Params));
    const DURATIONS: [Setter; 24] = [
        ("lease_first", |p| p.lease_first = SHORT),
        ("offer_stale", |p| p.offer_stale = Some(SHORT)),
        ("lease_default", |p| p.lease_default = SHORT),
        ("lease_max", |p| p.lease_max = SHORT),
        ("lease_ceiling", |p| p.lease_ceiling = SHORT),
        ("presence_silence", |p| p.presence_silence = SHORT),
        ("presence_resolution", |p| p.presence_resolution = SHORT),
        ("session_retain", |p| p.session_retain = SHORT),
        ("op_retain", |p| p.op_retain = SHORT),
        ("commitment_followup", |p| p.commitment_followup = SHORT),
        ("question_recheck", |p| p.question_recheck = SHORT),
        ("user_question_due", |p| p.user_question_due = SHORT),
        ("user_question_due_absent", |p| {
            p.user_question_due_absent = SHORT;
        }),
        ("user_gap_window", |p| p.user_gap_window = SHORT),
        ("deliver_window", |p| p.deliver_window = SHORT),
        ("talk_wait_max", |p| p.talk_wait_max = SHORT),
        ("refusal_ttl", |p| p.refusal_ttl = SHORT),
        ("refusal_window", |p| p.refusal_window = SHORT),
        ("refusal_backoff", |p| p.refusal_backoff = SHORT),
        ("pause_ttl", |p| p.pause_ttl = SHORT),
        ("archive_after", |p| p.archive_after = SHORT),
        ("coverage_window", |p| p.coverage_window = SHORT),
        ("capacity_window", |p| p.capacity_window = SHORT),
        ("queue_max_wait", |p| p.queue_max_wait = SHORT),
    ];

    /// One setter per count that must be at least 1, each to 0.
    const COUNTS: [Setter; 11] = [
        ("hold_per_session", |p| p.hold_per_session = 0),
        ("question_max_rechecks", |p| p.question_max_rechecks = 0),
        ("too_big_attempts", |p| p.too_big_attempts = 0),
        ("leave_better_max", |p| p.leave_better_max = 0),
        ("tree_max_children", |p| p.tree_max_children = 0),
        ("settle_batch", |p| p.settle_batch = 0),
        ("write_per_hour", |p| p.write_per_hour = 0),
        ("digest_max", |p| p.digest_max = 0),
        ("instructions_max", |p| p.instructions_max = 0),
        ("user_question_max_pending", |p| {
            p.user_question_max_pending = 0;
        }),
        ("refusal_quorum", |p| p.refusal_quorum = 0),
    ];

    #[test]
    fn every_duration_and_every_count_is_checked() {
        // Every field, by name and without `..`: a new parameter does not compile here until it
        // is classified, as a duration (DURATIONS), a count (COUNTS), or a field with its own
        // rule, which `each_rule_refuses` covers.
        let Params {
            lease_first: _,
            lease_default: _,
            lease_max: _,
            lease_ceiling: _,
            renew_max: _, // may be 0
            hold_per_session: _,
            presence_silence: _,
            presence_resolution: _,
            session_retain: _,
            op_retain: _,
            conversation_schemes: _, // never user, repeat, or person
            commitment_followup: _,
            question_recheck: _,
            question_max_rechecks: _,
            user_question_due: _,
            user_question_due_absent: _,
            user_question_max_pending: _,
            user_gap_window: _,
            deliver_window: _,
            talk_wait_max: _,
            hello_budget: _, // at least 512
            brief_budget: _, // at least 1,024
            digest_max: _,
            leave_better_max: _,
            refusal_ttl: _,
            refusal_window: _,
            refusal_backoff: _,
            refusal_quorum: _,
            too_big_attempts: _,
            pause_ttl: _,
            archive_after: _,
            coverage_window: _,
            capacity_window: _,
            write_per_hour: _,
            queue_max_wait: _,
            text_max: _,       // at least digest_max
            tree_max_depth: _, // at least 3
            tree_max_children: _,
            settle_batch: _,
            instructions_max: _,
            offer_stale: _,
        } = Params::default();
        let defaults = Params::default();
        assert_eq!(DURATIONS.len(), defaults.durations().len());
        assert_eq!(COUNTS.len(), defaults.counts().len());
        assert!(!defaults.durations().contains(&SHORT));
        assert!(!defaults.counts().contains(&0));
        for (name, set) in DURATIONS {
            let p = with(set);
            // The setter reaches a field that `durations()` lists, and `valid` refuses it, even
            // where no order rule would (`user_gap_window`, `capacity_window`, ...).
            assert!(
                p.durations().contains(&SHORT),
                "{name} is not in durations()"
            );
            assert!(!p.valid(), "{name} = 999 ms is accepted");
        }
        for (name, set) in COUNTS {
            let p = with(set);
            assert!(p.counts().contains(&0), "{name} is not in counts()");
            assert!(!p.valid(), "{name} = 0 is accepted");
        }
    }

    #[test]
    fn one_second_is_the_shortest_duration() {
        // At exactly 1 s, a duration with no order rule of its own is accepted.
        let at_one_second: [fn(&mut Params); 6] = [
            |p| p.user_gap_window = SECOND,
            |p| p.capacity_window = SECOND,
            |p| p.coverage_window = SECOND,
            |p| p.queue_max_wait = SECOND,
            |p| p.talk_wait_max = SECOND,
            |p| p.op_retain = SECOND,
        ];
        for set in at_one_second {
            assert!(with(set).valid());
        }
    }

    trait PlusOne {
        fn plus_one(self) -> Self;
    }
    impl PlusOne for Dur {
        fn plus_one(self) -> Dur {
            Dur(self.0 + 1)
        }
    }
}
