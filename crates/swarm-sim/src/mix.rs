//! The load model of design D.15.2: the three profiles, the base and heavy mixes, and the call
//! mix per session.
//!
//! **Profiles.** Every session is a new agent (§1). A profile sets the mean session length and the
//! calls per agent-minute; 10,000 live agents then make 125, 250, or 500 calls a second.
//!
//! **The mix per session** is the review's (D.15.2, TP §1.1), scaled to the profile's rate. The
//! review counts, per session of its own length (a mean of 740 s): one `hello`, 1.3 `next`, 0.2
//! `take`, 1.5 `view node`, 0.5 `find`, 0.3 other views, 0.5 `create` (0.3 new and 0.2 that join),
//! 0.2 `update`, 0.35 `close`, 0.25 `release`, 0.1 `renew`, 0.3 `said` and 0.3 confirmations, 0.2
//! `heard`, requests, and questions, 0.2 `say`, 0.35 `decide`, 0.15 `to_user`, 0.05 `from_user`,
//! 0.03 `route`, 0.1 `who`, 0.1 `status`, 0.05 key, link, move, merge, and reopen, 0.05 `check`, and
//! 0.02 `health`; a checkpoint every 5 minutes; and about a third of live sessions parked in `talk
//! wait`. A session of the profile (30 minutes at 1.5 calls a minute at base) makes more calls than
//! the review's, so the counts keep their proportions: [`WORKER`] weights the draws of a working
//! session by the review's counts (a weight of 10 per 0.1 a session), after its forced calls (the
//! `hello` first, a checkpoint whenever one is due, and the end of the session). [`PRINCIPAL`] adds
//! the user side for the one session in ten that serves a principal (D.12.8), ten times the
//! review's per-session counts, since only those sessions make them. A working session adjusts a
//! draw to what it holds, as an agent would: with nothing held it takes work before it updates,
//! closes, or decides, and with `hold.per_session` held it closes or hands off one before it takes
//! more. A parked session waits in `talk wait`, with a read of the mix now and then. What the driver does not send: `route`, `answer`,
//! `approve`, `object`, and `resolve` (0.11 a session together; the tool table of the review's load
//! model adds 0.05 key and link and 0.05 `check` to the counts above); confirmations are sent as
//! `said`.
//!
//! With waits counted as reads (as D.5.2 and the review count them), about 55 % of calls are
//! reads. The **heavy mix** is 2.5 times the base rate, with the same composition, and a **burst**
//! triples the rate while it lasts.

use crate::record::Category;

/// A profile of D.15.2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    /// Its name.
    pub name: &'static str,
    /// The mean session length, in minutes of the forum's time.
    pub session_min: f64,
    /// Calls per agent-minute of the forum's time.
    pub calls_per_min: f64,
}

/// 60-minute sessions, 0.75 calls per agent-minute: 125 calls a second at 10,000 agents.
pub const LOW: Profile = Profile {
    name: "low",
    session_min: 60.0,
    calls_per_min: 0.75,
};
/// 30-minute sessions, 1.5 calls per agent-minute: 250 calls a second at 10,000 agents.
pub const BASE: Profile = Profile {
    name: "base",
    session_min: 30.0,
    calls_per_min: 1.5,
};
/// 12-minute sessions, 3 calls per agent-minute: 500 calls a second at 10,000 agents.
pub const HIGH: Profile = Profile {
    name: "high",
    session_min: 12.0,
    calls_per_min: 3.0,
};

/// A profile by name.
#[must_use]
pub fn profile(name: &str) -> Option<Profile> {
    [LOW, BASE, HIGH].into_iter().find(|p| p.name == name)
}

/// The heavy mix's rate, relative to the base (D.15.2).
pub const HEAVY: f64 = 2.5;
/// A burst's rate, relative to the mix it interrupts (D.15.1).
pub const BURST: f64 = 3.0;

/// One kind of call a simulated agent makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// `session hello`.
    Hello,
    /// `session hello` whose intent is a gate's chosen search (G-9's worst case), recorded apart.
    HelloSearch,
    /// `hold next`.
    Next,
    /// `hold take` of a node seen before.
    Take,
    /// `view node`.
    ViewNode,
    /// `view find {text}`.
    FindText,
    /// `view find {key}`.
    FindKey,
    /// `view list`.
    List,
    /// `view feed`.
    Feed,
    /// `view history`.
    History,
    /// `hold check`.
    Check,
    /// `control health`.
    Health,
    /// `node create` of new work.
    Create,
    /// `node create` with a key that an open node carries: it joins that node (D.8.9).
    CreateJoin,
    /// `node update` of a held node.
    Update,
    /// `node close` of a held node.
    Close,
    /// `hold release`.
    Release,
    /// `hold renew`.
    Renew,
    /// `hold checkpoint`.
    Checkpoint,
    /// `talk say` to the holders of a node.
    Say,
    /// `talk decide`.
    Decide,
    /// `talk said` in a conversation.
    Said,
    /// `talk heard`.
    Heard,
    /// `talk ask` to agents.
    AskAgents,
    /// `talk who`.
    Who,
    /// `node key {add}`.
    Key,
    /// `node link`.
    Link,
    /// `session status`.
    Status,
    /// `talk from_user`.
    FromUser,
    /// `talk to_user`.
    ToUser,
    /// `talk ask {to: user}`.
    AskUser,
    /// `view requests`.
    Requests,
    /// `view user_log`.
    UserLog,
    /// `talk wait`.
    Wait,
    /// `control pause` of `/`.
    Pause,
    /// `control unpause` of `/`.
    Unpause,
    /// `talk say {to: holders:/**}`, the broadcast of G-1.
    SayHolders,
}

impl Action {
    /// Its name in reports: `<tool>.<action>`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Action::Hello => "session.hello",
            Action::HelloSearch => "session.hello_search",
            Action::Next => "hold.next",
            Action::Take => "hold.take",
            Action::ViewNode => "view.node",
            Action::FindText => "view.find_text",
            Action::FindKey => "view.find_key",
            Action::List => "view.list",
            Action::Feed => "view.feed",
            Action::History => "view.history",
            Action::Check => "hold.check",
            Action::Health => "control.health",
            Action::Create => "node.create",
            Action::CreateJoin => "node.create_join",
            Action::Update => "node.update",
            Action::Close => "node.close",
            Action::Release => "hold.release",
            Action::Renew => "hold.renew",
            Action::Checkpoint => "hold.checkpoint",
            Action::Say => "talk.say",
            Action::Decide => "talk.decide",
            Action::Said => "talk.said",
            Action::Heard => "talk.heard",
            Action::AskAgents => "talk.ask_agents",
            Action::Who => "talk.who",
            Action::Key => "node.key",
            Action::Link => "node.link",
            Action::Status => "session.status",
            Action::FromUser => "talk.from_user",
            Action::ToUser => "talk.to_user",
            Action::AskUser => "talk.ask_user",
            Action::Requests => "view.requests",
            Action::UserLog => "view.user_log",
            Action::Wait => "talk.wait",
            Action::Pause => "control.pause",
            Action::Unpause => "control.unpause",
            Action::SayHolders => "talk.say_holders",
        }
    }

    /// Its latency class (D.12.8, G-1).
    #[must_use]
    pub fn category(self) -> Category {
        match self {
            Action::Hello | Action::HelloSearch => Category::Hello,
            Action::Next => Category::Next,
            Action::ViewNode
            | Action::FindText
            | Action::FindKey
            | Action::List
            | Action::Feed
            | Action::History
            | Action::Check
            | Action::Who
            | Action::Requests
            | Action::UserLog => Category::Read,
            Action::Health => Category::Health,
            Action::Wait => Category::Wait,
            _ => Category::Write,
        }
    }

    /// The refusals the mix may meet without anything being wrong: contention on a slot, a hold
    /// that ended (a compressed clock makes leases short), a version someone else moved, and the
    /// pause of G-1.
    #[must_use]
    pub fn expected(self) -> &'static [&'static str] {
        match self {
            Action::Take => &["full", "closed", "paused", "holds_limit", "not_found"],
            Action::Next => &["paused", "holds_limit"],
            Action::Update
            | Action::Close
            | Action::Release
            | Action::Renew
            | Action::Checkpoint => &[
                "stale_token",
                "stale_version",
                "closed",
                "limit(renews)",
                "paused",
            ],
            _ => &[],
        }
    }

    /// Whether it gives the agent a hold: work taken or made.
    #[must_use]
    pub fn acquires(self) -> bool {
        matches!(
            self,
            Action::Next | Action::Take | Action::Create | Action::CreateJoin
        )
    }

    /// Whether it needs a hold of the agent's own.
    #[must_use]
    pub fn needs_hold(self) -> bool {
        matches!(
            self,
            Action::Update
                | Action::Close
                | Action::Release
                | Action::Renew
                | Action::Checkpoint
                | Action::Decide
        )
    }

    /// Whether the server answers it with a stub's `invalid` at the commit this driver runs
    /// against ([`STUBBED`]): its answer is counted as stubbed only if it is the stub's, and it is
    /// skipped under `--stubs skip`. An `invalid` of any other action is an error, whatever its
    /// hint.
    #[must_use]
    pub fn stubbed_at_writing(self) -> bool {
        STUBBED.contains(&self)
    }
}

/// The actions whose command the server answers with a stub's `invalid` (D.13.1): none since
/// WP-D7 merged. The fifteen this driver listed while WP-D4 to WP-D7 were stubs (the create that
/// joins, `say`, `decide`, `said`, `heard`, the agent questions, `who`, keys, links, `session
/// status`, the user log, the user questions, `holders:` notes, and `talk wait`) are built; a
/// command that starts as a stub goes here until its package lands.
pub const STUBBED: &[Action] = &[];

/// The draws of a working session, by weight: 10 per 0.1 call in the review's session (see the
/// module's documentation). Its forced calls come first: the `hello`, a checkpoint when a hold's
/// is due, and the end of the session.
pub const WORKER: &[(Action, u32)] = &[
    (Action::Next, 130),
    (Action::Take, 20),
    (Action::ViewNode, 150),
    (Action::FindText, 30),
    (Action::FindKey, 20),
    (Action::List, 10),
    (Action::Feed, 10),
    (Action::History, 10),
    (Action::Create, 30),
    (Action::CreateJoin, 20),
    (Action::Update, 20),
    (Action::Close, 35),
    (Action::Release, 25),
    (Action::Renew, 10),
    (Action::Said, 60),
    (Action::Heard, 10),
    (Action::AskAgents, 10),
    (Action::Say, 20),
    (Action::Decide, 35),
    (Action::Who, 10),
    (Action::Status, 10),
    (Action::Key, 3),
    (Action::Link, 2),
    (Action::Check, 5),
    (Action::Health, 2),
];

/// What a session that serves a principal adds to [`WORKER`] (CS-1 to CS-5): the user side, ten
/// times the review's per-session counts, since one session in ten makes them.
pub const PRINCIPAL: &[(Action, u32)] = &[
    (Action::ToUser, 150),
    (Action::FromUser, 50),
    (Action::AskUser, 20),
    (Action::Requests, 10),
    (Action::UserLog, 10),
];

/// Of a parked session's calls, one in this many is a read rather than a wait.
pub const WAITER_READ_EVERY: u32 = 5;

/// Draws an action from `table` (and `extra`) with `pick` in `0..total`.
#[must_use]
pub fn draw(table: &[(Action, u32)], extra: &[(Action, u32)], mut pick: u32) -> Action {
    for (a, w) in table.iter().chain(extra) {
        if pick < *w {
            return *a;
        }
        pick -= w;
    }
    Action::ViewNode
}

/// The total weight of `table` and `extra`.
#[must_use]
pub fn total(table: &[(Action, u32)], extra: &[(Action, u32)]) -> u32 {
    table.iter().chain(extra).map(|(_, w)| w).sum()
}

/// The reads of `table` and `extra`, with their weights: what a session draws from when the action
/// it drew cannot be made now.
#[must_use]
pub fn reads(table: &[(Action, u32)], extra: &[(Action, u32)]) -> Vec<(Action, u32)> {
    table
        .iter()
        .chain(extra)
        .filter(|(a, _)| a.category().is_read())
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profiles_give_d_15_2s_rates_at_10_000_agents() {
        for (p, rate) in [(LOW, 125.0), (BASE, 250.0), (HIGH, 500.0)] {
            assert!((p.calls_per_min * 10_000.0 / 60.0 - rate).abs() < 1.0);
        }
        assert_eq!(profile("base"), Some(BASE));
        assert_eq!(profile("nope"), None);
    }

    fn weight(a: Action) -> f64 {
        f64::from(
            WORKER
                .iter()
                .filter(|(x, _)| *x == a)
                .map(|(_, w)| *w)
                .sum::<u32>(),
        )
    }

    #[test]
    fn a_session_draws_d_15_2s_counts_in_their_proportions() {
        // The review's session: its 7.18 discrete calls after the hello, less the user side
        // (PRINCIPAL's) and route, answer, approve, object, and resolve, which the driver does not
        // send.
        let per_session = f64::from(total(WORKER, &[])) / 100.0;
        assert!((per_session - 6.87).abs() < 0.01, "{per_session}");
        // Per review session: 1.3 next and 1.5 view node (D.15.2), 0.5 find, 0.5 create.
        for (a, want) in [
            (vec![Action::Next], 1.3),
            (vec![Action::ViewNode], 1.5),
            (vec![Action::FindText, Action::FindKey], 0.5),
            (vec![Action::Create, Action::CreateJoin], 0.5),
            (vec![Action::Close], 0.35),
            (vec![Action::Decide], 0.35),
        ] {
            let got: f64 = a.iter().map(|x| weight(*x)).sum::<f64>() / 100.0;
            assert!(
                (got - want).abs() < 1e-9,
                "{a:?}: {got} per session, not {want}"
            );
        }
        // The profile's session at base, 30 minutes at 1.5 calls a minute: 45 calls, of which one
        // hello and about six checkpoints are forced; view node and next keep D.15.2's ratio.
        let ratio = weight(Action::ViewNode) / weight(Action::Next);
        assert!((ratio - 1.5 / 1.3).abs() < 1e-9);
        let draws = BASE.session_min * BASE.calls_per_min - 1.0 - 6.0;
        let view_node = draws * weight(Action::ViewNode) / f64::from(total(WORKER, &[]));
        assert!((7.0..9.0).contains(&view_node), "{view_node}");
    }

    #[test]
    fn about_55_percent_of_calls_are_reads_with_waits_counted_as_reads() {
        // A worker session: one hello (a read) and about six forced checkpoints in 45 calls, the
        // rest drawn from WORKER.
        let t = f64::from(total(WORKER, &[]));
        let r: f64 = reads(WORKER, &[]).iter().map(|(_, w)| f64::from(*w)).sum();
        let worker_reads = (1.0 + 38.0 * r / t) / 45.0;
        // A third of the sessions park: a cycle of four waits of 50 s and a read after about 40 s
        // makes 1.25 calls a minute, every one a read; a worker makes 1.5.
        let parked_calls = (1.0 / 3.0) * 1.25;
        let worker_calls = (2.0 / 3.0) * 1.5;
        let share = (parked_calls + worker_calls * worker_reads) / (parked_calls + worker_calls);
        assert!((0.50..0.60).contains(&share), "{share}");
        assert_eq!(draw(WORKER, &[], 0), Action::Next);
        assert_eq!(draw(WORKER, PRINCIPAL, total(WORKER, &[])), Action::ToUser);
        assert_eq!(
            draw(WORKER, PRINCIPAL, total(WORKER, PRINCIPAL) - 1),
            Action::UserLog
        );
        assert!(
            reads(WORKER, PRINCIPAL)
                .iter()
                .all(|(a, _)| a.category().is_read())
        );
    }

    #[test]
    fn every_action_has_a_unique_name() {
        let all = [
            Action::Hello,
            Action::HelloSearch,
            Action::Next,
            Action::Take,
            Action::ViewNode,
            Action::FindText,
            Action::FindKey,
            Action::List,
            Action::Feed,
            Action::History,
            Action::Check,
            Action::Health,
            Action::Create,
            Action::CreateJoin,
            Action::Update,
            Action::Close,
            Action::Release,
            Action::Renew,
            Action::Checkpoint,
            Action::Say,
            Action::Decide,
            Action::Said,
            Action::Heard,
            Action::AskAgents,
            Action::Who,
            Action::Key,
            Action::Link,
            Action::Status,
            Action::FromUser,
            Action::ToUser,
            Action::AskUser,
            Action::Requests,
            Action::UserLog,
            Action::Wait,
            Action::Pause,
            Action::Unpause,
            Action::SayHolders,
        ];
        let names: std::collections::BTreeSet<_> = all.iter().map(|a| a.name()).collect();
        assert_eq!(names.len(), all.len());
    }
}
