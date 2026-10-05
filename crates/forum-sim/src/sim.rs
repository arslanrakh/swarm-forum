//! The discrete-event simulation: a queue of timed actions over one forum. Simulated humans write
//! to their agents, answer the questions they are shown, are absent, or leave; simulated agents
//! open sessions, take work with `next`, checkpoint, ask, release, report, close, and vanish
//! mid-task. Between two actions the clock jumps; every action settles the forum to its time
//! first, as the shell's pre-settle (or the idle writer) would.

use std::collections::{BTreeMap, BTreeSet};

use forum_core::health::Level;
use forum_core::query::guide::Guidance;
use forum_core::query::{self, HelloData};
use forum_core::{
    Cand, ClassName, DAY, Dur, HOUR, Label, MINUTE, NextReply, NodeId, NodeRef, PrincipalKey,
    QuestionId, ReleaseReason, Reply, Requester, SessionCtx, SessionId, Time, ToUserKind, Token,
    USER_PRINCIPAL, UqState, UserEntryId, WaitArg,
};

use crate::START;
use crate::cmds::{self, label, path};
use crate::forum::Forum;
use crate::guided;
use crate::observe::Observer;
use crate::profile::{Kind, Profile, Rate};
use crate::rng::{PPM, Rng};

/// A human the swarm serves (company profile).
#[derive(Clone, Debug)]
pub struct Person {
    /// Their principal keys: one, or two aliases of one person.
    pub keys: Vec<PrincipalKey>,
    /// Their team.
    pub team: usize,
    /// How often they write.
    pub rate: Rate,
    /// They never answer a question.
    pub never_answers: bool,
    /// Their node under `/@people`.
    pub node: Option<NodeId>,
    /// When they joined.
    pub joined: Time,
    /// When they left.
    pub left: Option<Time>,
    /// When an agent marked them `gone`.
    pub gone_marked: Option<Time>,
    /// Visits to their agent.
    pub visits: u64,
}

/// A team: its node under `/@teams`, its work node, and its members.
#[derive(Clone, Debug)]
pub struct Team {
    /// `team-NN`.
    pub name: String,
    /// The needs label of its work.
    pub label: Label,
    /// Its node under `/@teams`.
    pub node: NodeId,
    /// `/work/team-NN`.
    pub work: NodeId,
    /// Its members, in joining order.
    pub members: Vec<usize>,
}

/// A piece of work, as the agents know it (the forum knows the rest).
#[derive(Clone, Debug)]
pub struct Task {
    /// Its node.
    pub node: NodeId,
    /// Its team.
    pub team: Option<usize>,
    /// The person who asked for it.
    pub requester: Option<usize>,
    /// Minutes of work it takes.
    pub est: u64,
    /// Minutes done, as of the last checkpoint.
    pub done: u64,
    /// At this progress, a question for a human comes up.
    pub ask_at: Option<u64>,
    /// That question carries a default.
    pub ask_default: bool,
    /// The question for the user, once asked.
    pub asked: Option<QuestionId>,
    /// At this progress, a question for a person outside the swarm comes up.
    pub person_at: Option<u64>,
    /// That question is blocking.
    pub person_blocking: bool,
    /// The people question, once asked.
    pub pq: Option<QuestionId>,
    /// At this progress, the task splits off an offered part.
    pub split_at: Option<u64>,
    /// When it was created.
    pub created: Time,
}

/// A simulated agent session that works.
#[derive(Clone, Debug)]
pub struct Worker {
    /// Its session.
    pub sess: Option<SessionId>,
    /// Its class.
    pub class: ClassName,
    /// Its team (its caps), if any.
    pub team: Option<usize>,
    /// Its hold.
    pub token: Option<Token>,
    /// When it vanishes mid-task, if it does.
    pub drop_at: Option<Time>,
    /// It will vanish during its first task.
    pub drops: bool,
    /// Tasks it finished or released.
    pub tasks: u32,
    /// The time of its last step.
    pub last_step: Time,
    /// The next calls of its last response.
    pub guidance: Option<Guidance>,
    /// When its own next action is due: a step of work after its last own action.
    pub own_at: Time,
    /// Its next step follows the first `now` next call of its last response.
    pub follow: bool,
    /// Still calling.
    pub alive: bool,
    /// What it starts with.
    pub start: Start,
}

/// How a worker session starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Start {
    /// `hello`, then `next`.
    Next,
    /// `hello`, then a scheduler's job of its own (no user): `create {request, take}`.
    Job,
}

/// A timed action.
#[derive(Clone, Debug)]
enum Act {
    /// A person writes to their agent.
    Message(usize),
    /// A worker's next step.
    Step(usize),
    /// The spawner looks for unattended work.
    Spawn,
    /// A scheduler hands the swarm a job (no user).
    Intake,
    /// A session wakes up and asks for work (no user).
    Poll,
    /// A person outside the swarm answers a people question.
    PersonAnswer {
        /// The question.
        q: QuestionId,
        /// Its node.
        node: NodeId,
    },
    /// The daily draw of leavers.
    LeaveDraw,
    /// A person leaves.
    Leave(usize),
    /// An agent marks a leaver `gone`.
    MarkGone(usize),
    /// A person joins a team.
    Join(usize),
    /// The hourly sample.
    Sample,
    /// The invariant check.
    Check,
}

/// The size of one `hello` (D.15.4): the structured data the shell renders within
/// `hello.budget`, and its user sections.
#[derive(Clone, Debug, Default)]
pub struct HelloSize {
    /// The `Debug` form of the hello data, in bytes: a proxy of the rendered size before the
    /// shell's budget cuts it.
    pub bytes: usize,
    /// Pending questions for the session's principals.
    pub decisions: u32,
    /// Of them shown.
    pub decision_items: usize,
    /// Pending digest entries for them.
    pub digest: u32,
    /// Of them shown.
    pub digest_items: usize,
    /// Older entries, in `@user/digest`.
    pub older: u64,
    /// Offers.
    pub offers: usize,
    /// Present sessions shown.
    pub present: usize,
    /// Notes delivered.
    pub inbox: usize,
}

impl HelloSize {
    fn of(h: &HelloData) -> HelloSize {
        HelloSize {
            bytes: format!("{h:?}").len(),
            decisions: h.user_decisions.count,
            decision_items: h.user_decisions.items.len(),
            digest: h.user_digest.count,
            digest_items: h.user_digest.items.len(),
            older: h.user_digest.older.unwrap_or(0),
            offers: h.offers.len(),
            present: h.present.items.len(),
            inbox: h.inbox.len(),
        }
    }
}

/// What the run measured, apart from the observer's events.
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    /// Hellos of the agents that serve principals (company), or of the workers (no user), within
    /// the measurement.
    pub hellos: Vec<HelloSize>,
    /// `health` samples within the measurement: green, yellow, red.
    pub health: [u64; 3],
    /// Sessions opened within the measurement, by role.
    pub sessions: BTreeMap<&'static str, u64>,
    /// Worker sessions that vanished mid-task.
    pub drops: u64,
    /// Spurious `cannot`s.
    pub cannots: u64,
    /// Empty `next` calls.
    pub next_empty: u64,
    /// Next calls followed (guided agents).
    pub follows: u64,
    /// Them by rule, call, and outcome (`made`, or `refused: <code>`).
    pub followed: BTreeMap<(&'static str, &'static str, String), u64>,
    /// Holds a followed call issued to a worker that held one already, which it gave back.
    pub second_holds: u64,
    /// Invariant checks run.
    pub checks: u64,
    /// Their violations: each distinct one with when it was first seen and how many checks saw it.
    pub violations: BTreeMap<String, (Time, forum_core::Violation, u64)>,
    /// Nodes that waited longer than `capacity.window`, with the longest wait seen.
    pub waited_long: BTreeMap<NodeId, Dur>,
    /// The longest wait seen.
    pub max_wait: Dur,
    /// Candidates offered for longer than `capacity.window` without anyone taking them.
    pub long_candidates: BTreeMap<NodeId, Dur>,
    /// The oldest candidate seen.
    pub max_candidate_age: Dur,
    /// The longest a question for the user was pending.
    pub max_uq_pending: Dur,
    /// Open work nodes with no mover (no hold, not a candidate, not waiting), with when first seen.
    pub parked: BTreeMap<NodeId, Time>,
    /// The most such nodes at one sample.
    pub parked_peak: u64,
    /// Per day of the measurement: open tasks, candidates, waiting nodes, pending questions,
    /// pending digest entries (all principals), and pending digest entries for `user`.
    pub daily: Vec<[u64; 6]>,
    /// Tasks created within the measurement.
    pub tasks_created: u64,
    /// Requests whose report an agent misaddressed on purpose (mistake injection).
    pub mistakes_reports: u64,
    /// Digest showings an agent misaddressed on purpose.
    pub mistakes_digest: u64,
    /// Leavers, gone marks, joiners.
    pub leavers: u64,
    /// Gone marks.
    pub gone_marks: u64,
    /// Joiners.
    pub joiners: u64,
    /// Answers given by principals.
    pub answers_given: u64,
    /// People answers recorded with `heard`.
    pub heard: u64,
    /// The most questions pending for one person at a sample.
    pub max_pending_person: u64,
    /// Person-samples above `user_question.max_pending` (20).
    pub saturated_samples: u64,
}

/// One run.
pub struct Sim {
    /// The profile.
    pub p: Profile,
    /// The forum.
    pub f: Forum,
    /// The generator.
    pub rng: Rng,
    q: BTreeMap<(u64, u64), Act>,
    seq: u64,
    /// The people.
    pub people: Vec<Person>,
    /// The teams.
    pub teams: Vec<Team>,
    /// The work.
    pub tasks: BTreeMap<NodeId, Task>,
    /// The tasks whose nodes may be open (the hourly sample visits these only).
    live: BTreeSet<NodeId>,
    /// The workers.
    pub workers: Vec<Worker>,
    /// The agent classes.
    pub classes: Vec<ClassName>,
    /// The class of the directory agent.
    pub directory: ClassName,
    /// `/work`, or `/work/jobs` with no user.
    pub jobs: Option<NodeId>,
    /// The measurement starts here.
    pub measure_from: Time,
    /// The measurement ends here.
    pub measure_to: Time,
    /// The run ends here, after the cool-down.
    pub end: Time,
    /// What was measured.
    pub m: Metrics,
    next_name: u64,
    next_digest: u64,
}

/// How long a guided agent takes to make a `now` next call: it reads the response, fills the
/// item's texts, and calls, well within the checkpoint's 5 minutes (§20.9 rule 3).
pub const FOLLOW_DELAY: Dur = MINUTE;

fn class(s: &str) -> ClassName {
    ClassName::new(s).unwrap_or_else(|| panic!("invalid class {s:?}"))
}

impl Sim {
    /// A run of `p`, set up and ready.
    #[must_use]
    pub fn new(p: Profile) -> Sim {
        let measure_from = START.plus(Dur(p.warmup_days * DAY.0));
        let measure_to = measure_from.plus(Dur(p.days * DAY.0));
        let end = measure_to.plus(Dur(p.cooldown_days * DAY.0));
        let obs = Observer {
            measure_from,
            ..Observer::default()
        };
        let f = Forum::new(forum_core::Params::default(), START, obs);
        let classes = match p.kind {
            Kind::Company => (0..p.classes)
                .map(|i| class(&format!("class-{i:02}")))
                .collect(),
            Kind::NoUser => vec![class("worker")],
        };
        let mut sim = Sim {
            rng: Rng::new(p.seed, 0),
            f,
            q: BTreeMap::new(),
            seq: 0,
            people: Vec::new(),
            teams: Vec::new(),
            tasks: BTreeMap::new(),
            live: BTreeSet::new(),
            workers: Vec::new(),
            classes,
            directory: class("directory"),
            jobs: None,
            measure_from,
            measure_to,
            end,
            m: Metrics::default(),
            next_name: 0,
            next_digest: 0,
            p,
        };
        sim.setup();
        sim
    }

    fn at(&mut self, t: Time, a: Act) {
        self.seq += 1;
        self.q.insert((t.0, self.seq), a);
    }

    /// Whether the clock is within the measured days.
    #[must_use]
    pub fn in_window(&self) -> bool {
        self.f.now >= self.measure_from && self.f.now <= self.measure_to
    }

    fn name(&mut self, prefix: &str) -> String {
        self.next_name += 1;
        format!("{prefix}-{}", self.next_name)
    }

    // --------------------------------------------------------------------------------------------
    // Setup

    /// Opens a session of `class` and returns it.
    fn open(
        &mut self,
        class: &ClassName,
        caps: Option<BTreeSet<Label>>,
        principal: Option<PrincipalKey>,
        role: &'static str,
    ) -> Option<SessionId> {
        let (out, g) = self
            .f
            .call(class, SessionCtx::New, &cmds::hello(caps, principal));
        let sess = out.ok().and_then(|d| d.opened);
        if sess.is_some() && self.in_window() {
            *self.m.sessions.entry(role).or_default() += 1;
        }
        let _ = g;
        sess
    }

    fn created(
        out: Result<forum_core::Done, forum_core::Error>,
    ) -> Option<(NodeId, Option<Token>)> {
        match out.ok()?.reply {
            Reply::Created { id, token, .. } => Some((id, token)),
            _ => None,
        }
    }

    fn setup(&mut self) {
        let dir = self.directory.clone();
        let Some(s) = self.open(&dir, None, None, "directory") else {
            panic!("the directory agent could not open a session");
        };
        let work = Self::created(self.f.call_in(
            &dir,
            s,
            &cmds::create(path(&["work"]), "work", |_| {}),
        ))
        .map(|x| x.0);
        match self.p.kind {
            Kind::Company => self.setup_company(s, work),
            Kind::NoUser => {
                let jobs = Self::created(self.f.call_in(
                    &dir,
                    s,
                    &cmds::create(path(&["work", "jobs"]), "jobs", |_| {}),
                ))
                .map(|x| x.0);
                // A permanent open child keeps the container from `ready_to_close`.
                let _ = self.f.call_in(
                    &dir,
                    s,
                    &cmds::create(path(&["work", "jobs", "readme"]), "how jobs work", |_| {}),
                );
                self.jobs = jobs;
                let first = self.rng.exp_ms(DAY.0 / self.p.jobs_per_day.max(1));
                self.at(START.plus(Dur(first)), Act::Intake);
                if self.p.polls_per_day > 0 {
                    let first = self.rng.exp_ms(DAY.0 / self.p.polls_per_day);
                    self.at(START.plus(Dur(first)), Act::Poll);
                }
            }
        }
        self.at(START.plus(self.p.spawn_every), Act::Spawn);
        self.at(START.plus(HOUR), Act::Sample);
        if self.p.check_every_hours > 0 {
            self.at(
                START.plus(Dur(self.p.check_every_hours * HOUR.0)),
                Act::Check,
            );
        }
    }

    fn setup_company(&mut self, s: SessionId, _work: Option<NodeId>) {
        let dir = self.directory.clone();
        // The rates, in exact proportions, shuffled over the principals.
        let n = self.p.principals;
        let mut rates = Vec::with_capacity(n);
        for (r, permille) in Rate::ALL.iter().zip(self.p.rate_mix) {
            let k = (n * permille as usize + 500) / 1_000;
            rates.extend(std::iter::repeat_n(*r, k));
        }
        while rates.len() < n {
            rates.push(Rate::Daily);
        }
        rates.truncate(n);
        self.rng.shuffle(&mut rates);
        let mut never: Vec<bool> = (0..n)
            .map(|i| i < (n * self.p.never_answer as usize + 500) / 1_000)
            .collect();
        self.rng.shuffle(&mut never);
        for t in 0..self.p.teams() {
            let name = format!("team-{t:02}");
            let lbl = label(&name);
            let mut members = Vec::new();
            let mut nodes = Vec::new();
            for k in 0..self.p.team_size {
                let i = t * self.p.team_size + k;
                if i >= n {
                    break;
                }
                let idx = self.add_person(s, t, rates[i], never[i], START);
                members.push(idx);
                if let Some(x) = self.people[idx].node {
                    nodes.push(x);
                }
            }
            let team_node = Self::created(self.f.call_in(
                &dir,
                s,
                &cmds::create(path(&["@teams", &name]), &name, |a| {
                    a.owners = nodes.iter().map(|&x| NodeRef::Id(x)).collect();
                }),
            ))
            .map(|x| x.0)
            .unwrap_or_else(|| panic!("team {name} not created"));
            let work = Self::created(self.f.call_in(
                &dir,
                s,
                &cmds::create(path(&["work", &name]), &name, |a| {
                    a.needs = Some([lbl.clone()].into());
                    a.owners = vec![NodeRef::Id(team_node)];
                }),
            ))
            .map(|x| x.0)
            .unwrap_or_else(|| panic!("work node of {name} not created"));
            let _ = self.f.call_in(
                &dir,
                s,
                &cmds::create(path(&["work", &name, "readme"]), "how we work", |_| {}),
            );
            self.f
                .obs
                .team_members
                .push(members.iter().copied().collect());
            self.teams.push(Team {
                name,
                label: lbl,
                node: team_node,
                work,
                members,
            });
        }
        for i in 0..self.people.len() {
            let gap = self.people[i].rate.mean_gap().0;
            let first = self.rng.exp_ms(gap);
            self.at(START.plus(Dur(first)), Act::Message(i));
        }
        self.at(START.plus(DAY), Act::LeaveDraw);
    }

    /// A new person in team `t`, with their node under `/@people` created in session `s`.
    fn add_person(
        &mut self,
        s: SessionId,
        t: usize,
        rate: Rate,
        never_answers: bool,
        now: Time,
    ) -> usize {
        let idx = self.people.len();
        let mut keys = vec![cmds::principal(&format!("sim:p{idx:04}"))];
        if self.rng.chance(self.p.alias) {
            keys.push(cmds::principal(&format!("chat:p{idx:04}")));
        }
        for k in &keys {
            self.f.obs.person_of_key.insert(k.as_str().to_string(), idx);
        }
        let dir = self.directory.clone();
        let person_keys: Vec<forum_core::Key> = keys
            .iter()
            .map(|k| cmds::key(&format!("person:{k}")))
            .collect();
        let node = Self::created(self.f.call_in(
            &dir,
            s,
            &cmds::create(
                path(&["@people", &format!("p-{idx:04}")]),
                "a person",
                |a| {
                    a.keys = person_keys;
                },
            ),
        ))
        .map(|x| x.0);
        self.people.push(Person {
            keys,
            team: t,
            rate,
            never_answers,
            node,
            joined: now,
            left: None,
            gone_marked: None,
            visits: 0,
        });
        idx
    }

    // --------------------------------------------------------------------------------------------
    // The run

    /// Runs to the end and takes the final sample.
    pub fn run(&mut self) {
        while let Some(((t, _), a)) = self.q.pop_first() {
            let t = Time(t);
            if t > self.end {
                break;
            }
            self.f.advance(t);
            self.act(a);
        }
        self.f.advance(self.end);
        self.sample();
        if self.p.check_every_hours > 0 {
            self.check();
        }
    }

    fn act(&mut self, a: Act) {
        match a {
            Act::Message(i) => self.message(i),
            Act::Step(w) => self.step(w),
            Act::Spawn => self.spawn(),
            Act::Intake => self.intake(),
            Act::Poll => self.poll(),
            Act::PersonAnswer { q, node } => self.person_answer(q, node),
            Act::LeaveDraw => self.leave_draw(),
            Act::Leave(i) => self.leave(i),
            Act::MarkGone(i) => self.mark_gone(i),
            Act::Join(t) => self.join(t),
            Act::Sample => {
                self.sample();
                let next = self.f.now.plus(HOUR);
                self.at(next, Act::Sample);
            }
            Act::Check => {
                self.check();
                let next = self.f.now.plus(Dur(self.p.check_every_hours * HOUR.0));
                self.at(next, Act::Check);
            }
        }
    }

    // --------------------------------------------------------------------------------------------
    // Humans (company)

    /// Person `i` writes to their agent: a new session that logs the message first, answers the
    /// questions shown to them, routes a new request to a node of their team, and shows them their
    /// digest.
    fn message(&mut self, i: usize) {
        if self.people[i].left.is_some() {
            return;
        }
        let gap = self.people[i].rate.mean_gap().0;
        let next = self.f.now.plus(Dur(self.rng.exp_ms(gap)));
        self.at(next, Act::Message(i));
        self.people[i].visits += 1;
        let c = self.classes[self.rng.index(self.classes.len())].clone();
        let k = {
            let keys = &self.people[i].keys;
            keys[self.rng.index(keys.len())].clone()
        };
        let Some(sess) = self.open(&c, None, Some(k), "assistant") else {
            return;
        };
        let h = query::hello(&self.f.s, sess, self.f.now);
        if self.in_window() {
            self.m.hellos.push(HelloSize::of(&h));
        }
        let mut answered = 0;
        if !self.people[i].never_answers {
            for &q in &h.user_decisions.items {
                if self
                    .f
                    .call_in(&c, sess, &cmds::from_user("my answer", Some(q)))
                    .is_ok()
                {
                    answered += 1;
                    self.m.answers_given += 1;
                }
            }
        }
        if self.rng.chance(self.p.request) {
            self.new_request(i, &c, sess);
        } else if answered == 0
            && let Some(e) = self.log(&c, sess, "just checking in")
        {
            let _ = self.f.call_in(&c, sess, &cmds::route_none(e));
        }
        if !h.user_digest.items.is_empty() && self.rng.chance(self.p.show_digest) {
            let mut re: BTreeSet<forum_core::Eid> = h.user_digest.items.iter().copied().collect();
            if self.rng.chance(self.p.mistake)
                && let Some(d) = self.someone_elses_digest(i)
            {
                re.insert(d);
                self.m.mistakes_digest += 1;
            }
            let _ = self
                .f
                .call_in(&c, sess, &cmds::to_user(ToUserKind::Ack, None, re));
        }
    }

    /// Logs a message of the session's principal, unrouted; returns its entry.
    fn log(&mut self, c: &ClassName, sess: SessionId, t: &str) -> Option<UserEntryId> {
        match self
            .f
            .call_in(c, sess, &cmds::from_user(t, None))
            .ok()?
            .reply
        {
            Reply::FromUser { entry, .. } => Some(entry),
            _ => None,
        }
    }

    /// A new request of person `i`: logged, then a node of their team's work carrying its
    /// `user:e<n>` key, which routes it there (D.8.11).
    fn new_request(&mut self, i: usize, c: &ClassName, sess: SessionId) {
        let Some(e) = self.log(c, sess, "please do this") else {
            return;
        };
        let t = self.people[i].team;
        let team = self.teams[t].name.clone();
        let name = self.name("r");
        let est = self.rng.range(self.p.est_min.0, self.p.est_min.1);
        let out = self.f.call_in(
            c,
            sess,
            &cmds::create(path(&["work", &team, &name]), "a request", |a| {
                a.keys = vec![cmds::user_entry_key(e)];
                a.est = u32::try_from(est).ok();
            }),
        );
        if let Some((node, _)) = Self::created(out) {
            self.add_task(node, Some(t), Some(i), est, false);
        } else {
            // The entry stays unrouted; route it nowhere rather than leave it to `next`.
            let _ = self.f.call_in(c, sess, &cmds::route_none(e));
        }
    }

    /// Records a task; `conv`: its node carries a conversation key, so it can ask a person.
    fn add_task(
        &mut self,
        node: NodeId,
        team: Option<usize>,
        requester: Option<usize>,
        est: u64,
        conv: bool,
    ) {
        let ask_at = self
            .rng
            .chance(self.p.ask_user)
            .then(|| self.rng.range(0, est.saturating_sub(1)));
        let ask_default = self.rng.chance(self.p.ask_default);
        let person_at = (conv && self.rng.chance(self.p.ask_person))
            .then(|| self.rng.range(0, est.saturating_sub(1)));
        let person_blocking = self.rng.chance(self.p.person_blocking);
        let split_at = self
            .rng
            .chance(self.p.split)
            .then(|| self.rng.range(0, est.saturating_sub(1)));
        if let Some(t) = team {
            self.f.obs.node_team.insert(node, t);
        }
        if self.in_window() {
            self.m.tasks_created += 1;
        }
        self.live.insert(node);
        self.tasks.insert(
            node,
            Task {
                node,
                team,
                requester,
                est,
                done: 0,
                ask_at,
                ask_default,
                asked: None,
                person_at,
                person_blocking,
                pq: None,
                split_at,
                created: self.f.now,
            },
        );
    }

    /// A pending digest entry of another person, for the mistake of showing it to `i`.
    fn someone_elses_digest(&self, i: usize) -> Option<forum_core::Eid> {
        for (p, set) in &self.f.s.ix.pending_digest_by {
            if self.f.obs.person_of(*p) != Some(i)
                && let Some(d) = set.iter().next()
            {
                return Some(*d);
            }
        }
        None
    }

    fn leave_draw(&mut self) {
        let h = self.p.leave_per_day();
        for i in 0..self.people.len() {
            if self.people[i].left.is_none() && self.rng.chance(h) {
                let t = self.f.now.plus(Dur(self.rng.below(DAY.0)));
                self.at(t, Act::Leave(i));
            }
        }
        let next = self.f.now.plus(DAY);
        self.at(next, Act::LeaveDraw);
    }

    fn leave(&mut self, i: usize) {
        if self.people[i].left.is_some() {
            return;
        }
        self.people[i].left = Some(self.f.now);
        if self.in_window() {
            self.m.leavers += 1;
        }
        if self.rng.chance(self.p.gone_mark) {
            let t = self.f.now.plus(Dur(self.rng.range(DAY.0, 7 * DAY.0)));
            self.at(t, Act::MarkGone(i));
        }
        if self.p.replace_leavers {
            let t = self.f.now.plus(Dur(self.rng.range(0, 14 * DAY.0)));
            let team = self.people[i].team;
            self.at(t, Act::Join(team));
        }
    }

    fn mark_gone(&mut self, i: usize) {
        let Some(n) = self.people[i].node else {
            return;
        };
        let dir = self.directory.clone();
        let Some(s) = self.open(&dir, None, None, "directory") else {
            return;
        };
        let Some(version) = self.f.s.nodes.get(&n).map(|x| x.version) else {
            return;
        };
        let out = self.f.call_in(
            &dir,
            s,
            &cmds::update(n, version, |p| {
                p.tags_add = [label(forum_core::GONE)].into();
            }),
        );
        if out.is_ok() {
            self.people[i].gone_marked = Some(self.f.now);
            if self.in_window() {
                self.m.gone_marks += 1;
            }
        }
    }

    fn join(&mut self, t: usize) {
        let dir = self.directory.clone();
        let Some(s) = self.open(&dir, None, None, "directory") else {
            return;
        };
        let rate = {
            let x = self.rng.below(1_000) as u32;
            let mut acc = 0;
            let mut out = Rate::Daily;
            for (r, w) in Rate::ALL.iter().zip(self.p.rate_mix) {
                acc += w;
                if x < acc {
                    out = *r;
                    break;
                }
            }
            out
        };
        let never = self.rng.chance(self.p.never_answer * 1_000);
        let now = self.f.now;
        let idx = self.add_person(s, t, rate, never, now);
        if self.in_window() {
            self.m.joiners += 1;
        }
        self.teams[t].members.push(idx);
        if let Some(m) = self.f.obs.team_members.get_mut(t) {
            m.insert(idx);
        }
        let team_node = self.teams[t].node;
        let owners: Vec<NodeRef> = self
            .f
            .s
            .nodes
            .get(&team_node)
            .map(|x| x.owners.iter().map(|&o| NodeRef::Id(o)).collect())
            .unwrap_or_default();
        if let (Some(version), Some(pn)) = (
            self.f.s.nodes.get(&team_node).map(|x| x.version),
            self.people[idx].node,
        ) {
            let mut owners = owners;
            owners.push(NodeRef::Id(pn));
            let _ = self.f.call_in(
                &dir,
                s,
                &cmds::update(team_node, version, |p| p.owners = Some(owners)),
            );
        }
        let gap = self.people[idx].rate.mean_gap().0;
        let first = self.f.now.plus(Dur(self.rng.exp_ms(gap)));
        self.at(first, Act::Message(idx));
    }

    // --------------------------------------------------------------------------------------------
    // Work arriving with no user

    fn intake(&mut self) {
        let next = self
            .f
            .now
            .plus(Dur(self.rng.exp_ms(DAY.0 / self.p.jobs_per_day.max(1))));
        self.at(next, Act::Intake);
        self.new_worker(None, Start::Job, self.f.now);
    }

    fn poll(&mut self) {
        let next = self
            .f
            .now
            .plus(Dur(self.rng.exp_ms(DAY.0 / self.p.polls_per_day.max(1))));
        self.at(next, Act::Poll);
        self.new_worker(None, Start::Next, self.f.now);
    }

    fn person_answer(&mut self, q: QuestionId, node: NodeId) {
        let open = self.f.s.pquestions.get(&q).is_some_and(|x| {
            matches!(
                x.state,
                forum_core::PqState::Open
                    | forum_core::PqState::Expired
                    | forum_core::PqState::Escalated { .. }
            )
        });
        if !open {
            return;
        }
        let c = self.classes[0].clone();
        let Some(sess) = self.open(&c, None, None, "listener") else {
            return;
        };
        if self.f.call_in(&c, sess, &cmds::heard(node, q)).is_ok() {
            self.m.heard += 1;
        }
    }

    // --------------------------------------------------------------------------------------------
    // Workers

    /// The spawner: for each team (or the forum), as many new workers as there are candidates
    /// nobody holds, at most `spawn_max` a tick, each starting within the tick.
    fn spawn(&mut self) {
        let next = self.f.now.plus(self.p.spawn_every);
        self.at(next, Act::Spawn);
        let mut per_team: BTreeMap<Option<usize>, u64> = BTreeMap::new();
        for c in self.f.s.ix.next.of.keys() {
            let Cand::Node(n) = c else {
                continue;
            };
            if self.f.s.ix.next.held.contains_key(c) {
                continue;
            }
            let team = match self.p.kind {
                Kind::NoUser => None,
                Kind::Company => {
                    let needs = self
                        .f
                        .s
                        .nodes
                        .get(n)
                        .map(|x| x.needs.clone())
                        .unwrap_or_default();
                    match self.teams.iter().position(|t| needs.contains(&t.label)) {
                        Some(t) => Some(t),
                        None => continue,
                    }
                }
            };
            *per_team.entry(team).or_default() += 1;
        }
        // Workers already on their way (spawned, not begun) count against the candidates.
        for w in &self.workers {
            if w.alive
                && w.sess.is_none()
                && w.start == Start::Next
                && let Some(k) = per_team.get_mut(&w.team)
            {
                *k = k.saturating_sub(1);
            }
        }
        for (team, k) in per_team {
            for _ in 0..k.min(self.p.spawn_max) {
                let t = self.f.now.plus(Dur(self.rng.below(self.p.spawn_every.0)));
                self.new_worker(team, Start::Next, t);
            }
        }
    }

    fn new_worker(&mut self, team: Option<usize>, start: Start, t: Time) {
        let c = self.classes[self.rng.index(self.classes.len())].clone();
        let drops = self.rng.chance(self.p.drop);
        self.workers.push(Worker {
            sess: None,
            class: c,
            team,
            token: None,
            drop_at: None,
            drops,
            tasks: 0,
            last_step: t,
            guidance: None,
            own_at: t,
            follow: false,
            alive: true,
            start,
        });
        let w = self.workers.len() - 1;
        self.at(t, Act::Step(w));
    }

    fn step_len(&mut self) -> Dur {
        Dur(self.rng.range(self.p.step_min.0, self.p.step_min.1) * MINUTE.0)
    }

    /// The worker's next step, chosen when a response comes: a guided agent makes the response's
    /// first `now` next call at once (`FOLLOW_DELAY`), with probability `p_follow`, instead of
    /// working on; otherwise its own next action comes when it is due (`own_at`), which a
    /// followed call does not move.
    fn schedule(&mut self, w: usize) {
        let follow = self.p.p_follow > 0
            && self.workers[w]
                .guidance
                .as_ref()
                .is_some_and(|g| guided::first_now(g, &self.f.s).is_some())
            && self.rng.chance(self.p.p_follow);
        self.workers[w].follow = follow;
        let t = if follow {
            self.f.now.plus(FOLLOW_DELAY)
        } else {
            self.workers[w].own_at.max(self.f.now)
        };
        self.at(t, Act::Step(w));
    }

    /// A followed next call, and the worker's bookkeeping after it: the hold the call issued is
    /// its task, a node it created is work, and a hold that is gone is no longer its own.
    fn follow(&mut self, w: usize, g: &Guidance, cmd: &forum_core::Cmd) {
        self.m.follows += 1;
        // Expired holds have left the state: the run settles to the step's time first.
        let had = self.workers[w]
            .token
            .filter(|t| self.f.s.holds.contains_key(t));
        let out = self.wcall(w, cmd);
        let rule = g.items.first().map_or("", |i| i.rule.name());
        let outcome = match &out {
            Ok(_) => String::from("made"),
            Err(e) => format!("refused: {:?}", e.code),
        };
        *self
            .m
            .followed
            .entry((rule, crate::forum::cmd_name(cmd), outcome))
            .or_default() += 1;
        let reply = out.as_ref().ok().map(|d| &d.reply);
        // A node the call created (a split's part) is work, as the worker's own split is.
        if let Some(Reply::Created { id, .. }) = reply
            && !self.tasks.contains_key(id)
        {
            let team = had
                .and_then(|t| self.tasks.get(&t.node))
                .and_then(|t| t.team)
                .or(self.workers[w].team);
            let est = self.rng.range(self.p.est_min.0, self.p.est_min.1);
            self.add_task(*id, team, None, est, false);
        }
        let issued = match reply {
            Some(
                Reply::Next(NextReply::Hold { token, .. })
                | Reply::Taken { token, .. }
                | Reply::Created {
                    token: Some(token), ..
                },
            ) => Some(*token),
            _ => None,
        };
        // The worker's hold is gone: the call ended it (a release, a close, a merge), or its
        // lease ran out before the call (it then answers `stale_token`).
        let ended = had.is_some_and(|t| !self.f.s.holds.contains_key(&t));
        if self.workers[w]
            .token
            .is_some_and(|t| !self.f.s.holds.contains_key(&t))
        {
            self.workers[w].token = None;
        }
        match issued {
            Some(t) if self.workers[w].token == Some(t) => {}
            Some(t) if self.workers[w].token.is_none() && !ended => {
                // A take or `next` after the hold was lost: the new hold is the worker's task.
                self.hold(w, t);
                return;
            }
            Some(t) => {
                // One task at a time: the worker gives a second hold back at once.
                self.m.second_holds += 1;
                let _ = self.wcall(w, &cmds::release(t, ReleaseReason::Handoff, None));
            }
            None => {}
        }
        if ended {
            self.after_task(w);
        } else {
            self.schedule(w);
        }
    }

    /// A worker's call, keeping the response's next calls.
    fn wcall(
        &mut self,
        w: usize,
        cmd: &forum_core::Cmd,
    ) -> Result<forum_core::Done, forum_core::Error> {
        let c = self.workers[w].class.clone();
        let Some(sess) = self.workers[w].sess else {
            return Err(forum_core::Error::not_found(forum_core::What::Session));
        };
        let (out, g) = self.f.call(&c, SessionCtx::Existing(sess), cmd);
        self.workers[w].guidance = Some(g);
        out
    }

    fn step(&mut self, w: usize) {
        if !self.workers[w].alive {
            return;
        }
        if self.workers[w].sess.is_none() {
            self.begin(w);
            return;
        }
        if let Some(d) = self.workers[w].drop_at
            && d <= self.f.now
        {
            // The session vanishes: no release, no checkpoint; its lease runs out.
            self.workers[w].alive = false;
            self.m.drops += 1;
            return;
        }
        // A guided agent makes the first `now` next call of its last response, chosen when the
        // response came (`schedule`).
        if std::mem::take(&mut self.workers[w].follow) {
            if let Some(g) = self.workers[w].guidance.clone()
                && let Some(cmd) = guided::first_now(&g, &self.f.s)
            {
                self.follow(w, &g, &cmd);
                return;
            }
            // The item can no longer be made: the agent works on to its own next action.
            if self.workers[w].own_at > self.f.now {
                let t = self.workers[w].own_at;
                self.at(t, Act::Step(w));
                return;
            }
        }
        let Some(token) = self.workers[w].token else {
            self.workers[w].alive = false;
            return;
        };
        let n = token.node;
        let elapsed = self.f.now.since(self.workers[w].last_step).0 / MINUTE.0;
        self.workers[w].last_step = self.f.now;
        let Some(task) = self.tasks.get(&n).cloned() else {
            self.end_task(w, token, ReleaseReason::Handoff, None);
            return;
        };
        let done = task.done + elapsed;
        // A question for the user comes up.
        if let Some(at) = task.ask_at
            && task.asked.is_none()
            && done >= at
        {
            self.next_digest += 1;
            let out = self.wcall(w, &cmds::ask_user(n, task.ask_default, self.next_digest));
            if let Ok(forum_core::Done {
                reply: Reply::Asked { question, .. },
                ..
            }) = out
            {
                if let Some(t) = self.tasks.get_mut(&n) {
                    t.asked = Some(question);
                    t.done = done;
                }
                if !task.ask_default {
                    self.end_task(
                        w,
                        token,
                        ReleaseReason::Blocked,
                        Some(WaitArg::Question(question)),
                    );
                    return;
                }
            }
        }
        // A question for a person outside the swarm comes up.
        if let Some(at) = task.person_at
            && task.pq.is_none()
            && done >= at
        {
            let out = self.wcall(w, &cmds::ask_person(n, task.person_blocking));
            if let Ok(forum_core::Done {
                reply: Reply::Asked { question, .. },
                ..
            }) = out
            {
                if let Some(t) = self.tasks.get_mut(&n) {
                    t.pq = Some(question);
                    t.done = done;
                }
                if self.rng.chance(self.p.person_answers) {
                    let t = self.f.now.plus(Dur(self.rng.exp_ms(self.p.person_delay.0)));
                    self.at(
                        t,
                        Act::PersonAnswer {
                            q: question,
                            node: n,
                        },
                    );
                }
                self.end_task(
                    w,
                    token,
                    ReleaseReason::Blocked,
                    Some(WaitArg::Question(question)),
                );
                return;
            }
        }
        // The task splits off an offered part.
        if let Some(at) = task.split_at
            && done >= at
        {
            if let Some(t) = self.tasks.get_mut(&n) {
                t.split_at = None;
            }
            self.split(w);
        }
        if done >= task.est {
            self.finish(w, token);
            return;
        }
        match self.wcall(w, &cmds::checkpoint(token)) {
            Ok(_) => {
                if let Some(t) = self.tasks.get_mut(&n) {
                    t.done = done;
                }
                self.workers[w].own_at = self.f.now.plus(self.step_len());
                self.schedule(w);
            }
            Err(_) => {
                // The hold is gone (an overrun lease): the session gives up.
                self.workers[w].alive = false;
            }
        }
    }

    /// A worker's first step: `hello`, then `next`, or a job of its own.
    fn begin(&mut self, w: usize) {
        let c = self.workers[w].class.clone();
        let caps = self.workers[w]
            .team
            .map(|t| [self.teams[t].label.clone()].into());
        let role = match self.workers[w].start {
            Start::Next => "worker",
            Start::Job => "worker (scheduled job)",
        };
        let Some(sess) = self.open(&c, caps, None, role) else {
            self.workers[w].alive = false;
            return;
        };
        self.workers[w].sess = Some(sess);
        if self.p.kind == Kind::NoUser && self.in_window() {
            let h = query::hello(&self.f.s, sess, self.f.now);
            self.m.hellos.push(HelloSize::of(&h));
        }
        match self.workers[w].start {
            Start::Next => self.next(w),
            Start::Job => self.job(w),
        }
    }

    /// A scheduler's job: a node with a conversation key and an external request, taken at once.
    fn job(&mut self, w: usize) {
        let Some(jobs) = self.jobs else {
            return;
        };
        let name = self.name("j");
        let est = self.rng.range(self.p.est_min.0, self.p.est_min.1);
        let mut p = self.path_of(jobs);
        p.0.push(forum_core::Name::new(&name).unwrap_or_else(|| panic!("name {name}")));
        let conv = format!("issue:sim/{name}");
        let out = self.wcall(
            w,
            &cmds::create(p, "a scheduled job", |a| {
                a.keys = vec![cmds::key(&conv)];
                a.request = Some(cmds::external_request("run the job", "scheduler"));
                a.est = u32::try_from(est).ok();
                a.take = Some(true);
            }),
        );
        match Self::created(out) {
            Some((node, Some(token))) => {
                self.add_task(node, None, None, est, true);
                self.hold(w, token);
            }
            Some((node, None)) => {
                self.add_task(node, None, None, est, true);
                self.workers[w].alive = false;
            }
            None => self.workers[w].alive = false,
        }
    }

    fn path_of(&self, n: NodeId) -> forum_core::Path {
        let mut names = Vec::new();
        let mut at = n;
        while let Some(p) = self.f.s.nodes.get(&at).and_then(|x| x.place.clone()) {
            names.push(p.name);
            at = p.parent;
        }
        names.reverse();
        forum_core::Path(names)
    }

    /// An offered part of the worker's task, beside it.
    fn split(&mut self, w: usize) {
        let Some(jobs) = self.jobs else {
            return;
        };
        let name = self.name("part");
        let est = self.rng.range(self.p.est_min.0, self.p.est_min.1);
        let mut p = self.path_of(jobs);
        p.0.push(forum_core::Name::new(&name).unwrap_or_else(|| panic!("name {name}")));
        let out = self.wcall(
            w,
            &cmds::create(p, "a part of a job", |a| {
                a.tags = [label(forum_core::OFFER)].into();
                a.est = u32::try_from(est).ok();
            }),
        );
        if let Some((node, _)) = Self::created(out) {
            self.add_task(node, None, None, est, false);
        }
    }

    /// `next`, and what the worker does with its answer.
    fn next(&mut self, w: usize) {
        let out = self.wcall(w, &cmds::next());
        match out.map(|d| d.reply) {
            Ok(Reply::Next(NextReply::Hold { token, .. })) => self.hold(w, token),
            Ok(Reply::Next(NextReply::Route { entry, .. })) => {
                let _ = self.wcall(w, &cmds::route_none(entry));
                self.workers[w].alive = false;
            }
            Ok(Reply::Next(NextReply::Empty { .. })) => {
                self.m.next_empty += 1;
                self.workers[w].alive = false;
            }
            _ => self.workers[w].alive = false,
        }
    }

    /// The worker holds `token`: a spurious refusal, a question to re-check, a closed node to
    /// finish, or work.
    fn hold(&mut self, w: usize, token: Token) {
        self.workers[w].token = Some(token);
        self.workers[w].last_step = self.f.now;
        let n = token.node;
        if self.rng.chance(self.p.cannot) {
            self.m.cannots += 1;
            self.end_task(w, token, ReleaseReason::Cannot, None);
            return;
        }
        let Some(task) = self.tasks.get(&n).cloned() else {
            // Not work the agents made (a person, a team, a signal): leave it to others.
            self.end_task(w, token, ReleaseReason::Handoff, None);
            return;
        };
        if self.workers[w].drops && self.workers[w].drop_at.is_none() {
            let left = task.est.saturating_sub(task.done).max(1);
            let at = self.f.now.plus(Dur(self.rng.range(1, left) * MINUTE.0));
            self.workers[w].drop_at = Some(at);
        }
        let node = self.f.s.nodes.get(&n);
        let reopened = self
            .f
            .s
            .holds
            .get(&token)
            .and_then(|h| h.reopened.clone())
            .is_some();
        let recheck = node.is_some_and(|x| !x.recheck_marks.is_empty());
        if reopened {
            // A closed node with a request nobody was told about: tell them, and close it again.
            self.finish(w, token);
            return;
        }
        if recheck
            && let Some(q) = task.pq
            && self
                .f
                .s
                .pquestions
                .get(&q)
                .is_some_and(|x| x.state == forum_core::PqState::Open)
        {
            // The person has not answered yet: wait for the next recheck.
            self.end_task(w, token, ReleaseReason::Blocked, Some(WaitArg::Question(q)));
            return;
        }
        self.workers[w].own_at = self.f.now.plus(self.step_len());
        self.schedule(w);
    }

    /// Reports to every requester of the node and closes it, settling what was answered.
    fn finish(&mut self, w: usize, token: Token) {
        let n = token.node;
        let unsettled: BTreeSet<forum_core::RequestId> = self
            .f
            .s
            .ix
            .items
            .get(&n)
            .map(|x| x.unsettled.clone())
            .unwrap_or_default();
        let mut said = false;
        for &r in &unsettled {
            let Some(req) = self.f.s.requests.get(&r) else {
                continue;
            };
            match req.requester {
                Requester::User(p) => {
                    let mut key = self.f.s.ix.principal_key.get(&p).cloned();
                    if self.rng.chance(self.p.mistake)
                        && let Some(other) = self.other_principal(p)
                    {
                        key = Some(other);
                        self.m.mistakes_reports += 1;
                    }
                    let _ = self.wcall(w, &cmds::to_user(ToUserKind::Result, key, [r].into()));
                }
                Requester::External => {
                    if !said {
                        said = true;
                        let _ = self.wcall(w, &cmds::said_result(n));
                    }
                }
            }
        }
        let _ = self.wcall(w, &cmds::close(n, Some(token), unsettled));
        self.after_task(w);
    }

    /// Another principal than `p`'s person: the mistake of reporting to the wrong human.
    fn other_principal(&mut self, p: forum_core::PrincipalId) -> Option<PrincipalKey> {
        let me = self.f.obs.person_of(p);
        let others: Vec<usize> = (0..self.people.len()).filter(|&i| Some(i) != me).collect();
        if others.is_empty() {
            return None;
        }
        let i = others[self.rng.index(others.len())];
        Some(self.people[i].keys[0].clone())
    }

    fn end_task(&mut self, w: usize, token: Token, reason: ReleaseReason, wait: Option<WaitArg>) {
        let _ = self.wcall(w, &cmds::release(token, reason, wait));
        self.after_task(w);
    }

    /// After a task: more work, or the session ends.
    fn after_task(&mut self, w: usize) {
        self.workers[w].token = None;
        self.workers[w].tasks += 1;
        self.workers[w].drop_at = None;
        self.workers[w].drops = false;
        if self.workers[w].tasks < 3 && self.rng.chance(self.p.continue_after) {
            self.next(w);
        } else {
            self.workers[w].alive = false;
        }
    }

    // --------------------------------------------------------------------------------------------
    // Measurement

    fn sample(&mut self) {
        let now = self.f.now;
        let s = &self.f.s;
        let window = s.params.capacity_window;
        let measuring = self.in_window();
        if measuring {
            let h = query::health(s, now);
            let k = match h.level {
                Level::Green => 0,
                Level::Yellow => 1,
                Level::Red => 2,
            };
            self.m.health[k] += 1;
        }
        // Waits.
        for n in &s.ix.next.waiting {
            if let Some(w) = s.nodes.get(n).and_then(|x| x.wait.as_ref()) {
                let age = now.since(w.set_at);
                self.m.max_wait = self.m.max_wait.max(age);
                if age > window {
                    let e = self.m.waited_long.entry(*n).or_default();
                    *e = (*e).max(age);
                }
            }
        }
        // Candidates nobody takes.
        for (c, at) in &s.ix.next.of {
            if let Cand::Node(n) | Cand::Status(n) = c {
                let age = now.since(at.since);
                self.m.max_candidate_age = self.m.max_candidate_age.max(age);
                if age > window {
                    let e = self.m.long_candidates.entry(*n).or_default();
                    *e = (*e).max(age);
                }
            }
        }
        // Pending questions per person (aliases merged), the most at once.
        let mut per_person: BTreeMap<Option<usize>, usize> = BTreeMap::new();
        for (p, set) in &s.ix.pending_uq_by {
            let who = self.f.obs.person_of(*p);
            *per_person.entry(who).or_default() += set.len();
        }
        if measuring {
            for (who, k) in &per_person {
                if who.is_some() {
                    self.m.max_pending_person = self.m.max_pending_person.max(*k as u64);
                    if *k as u32 > s.params.user_question_max_pending {
                        self.m.saturated_samples += 1;
                    }
                }
            }
        }
        // Pending questions for the user.
        let mut pending_uq = 0;
        for (q, u) in &s.uquestions {
            if u.state == UqState::Pending {
                pending_uq += 1;
                if let Some(x) = self.f.obs.uqs.get(q) {
                    self.m.max_uq_pending = self.m.max_uq_pending.max(now.since(x.created));
                }
            }
        }
        // Open work with no mover.
        let mut parked = 0;
        let mut open_tasks = 0;
        for n in self.f.obs.reopened_nodes.drain(..) {
            if self.tasks.contains_key(&n) {
                self.live.insert(n);
            }
        }
        let s = &self.f.s;
        let mut gone = Vec::new();
        for n in &self.live {
            let Some(node) = s.nodes.get(n) else {
                gone.push(*n);
                continue;
            };
            if !matches!(node.life, forum_core::Life::Open) {
                gone.push(*n);
                continue;
            }
            open_tasks += 1;
            let held = s.ix.node.get(n).is_some_and(|x| x.live > 0);
            let cand = s.ix.next.of.contains_key(&Cand::Node(*n))
                || s.ix.next.of.contains_key(&Cand::Status(*n));
            let waiting = s.ix.next.waiting.contains(n) || waits_unresolved(s, *n, now);
            if !held && !cand && !waiting {
                parked += 1;
                self.m.parked.entry(*n).or_insert(now);
            }
        }
        for n in gone {
            self.live.remove(&n);
        }
        self.m.parked_peak = self.m.parked_peak.max(parked);
        let digest_all: usize = s.ix.pending_digest_by.values().map(BTreeSet::len).sum();
        let digest_user =
            s.ix.pending_digest_by
                .get(&USER_PRINCIPAL)
                .map_or(0, BTreeSet::len);
        if measuring && (now.since(self.measure_from).0 % DAY.0 < HOUR.0 || now == self.measure_to)
        {
            self.m.daily.push([
                open_tasks,
                s.ix.next.nodes.into(),
                s.ix.next.waiting.len() as u64,
                pending_uq,
                digest_all as u64,
                digest_user as u64,
            ]);
        }
    }

    fn check(&mut self) {
        self.m.checks += 1;
        if let Err(v) = forum_core::check_invariants(&self.f.s, self.f.now) {
            for x in v {
                let k = format!("{x:?}");
                let now = self.f.now;
                self.m.violations.entry(k).or_insert((now, x, 0)).2 += 1;
            }
        }
    }

    /// The share of `ppm` as a fraction, for the report.
    #[must_use]
    pub fn share(ppm: u32) -> f64 {
        f64::from(ppm) / f64::from(PPM)
    }
}

/// A node waits on something whose resolution has a mover (a time, or a pending question).
fn waits_unresolved(s: &forum_core::State, n: NodeId, now: Time) -> bool {
    let Some(w) = s.nodes.get(&n).and_then(|x| x.wait.as_ref()) else {
        return false;
    };
    match w.on {
        forum_core::WaitOn::Time(t) | forum_core::WaitOn::External(t) => now < t,
        forum_core::WaitOn::UserQuestion(q) => s
            .uquestions
            .get(&q)
            .is_some_and(|u| u.state == UqState::Pending),
        forum_core::WaitOn::Question { q, rechecks_at_set } => s
            .pquestions
            .get(&q)
            .is_some_and(|x| x.state == forum_core::PqState::Open && x.rechecks <= rechecks_at_set),
        forum_core::WaitOn::Node(t) => s
            .nodes
            .get(&t)
            .is_some_and(|x| matches!(x.life, forum_core::Life::Open)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forum_core::query::guide::{Because, Moment, NextCall, Rule, Skeleton};
    use forum_core::{Cmd, Done, HoldCmd, Total};

    /// A company of one team whose workers always follow, never refuse, never vanish, and end
    /// their session after a task.
    fn sim() -> Sim {
        let mut p = Profile::company(1, 1);
        p.principals = 8;
        p.p_follow = PPM;
        p.cannot = 0;
        p.drop = 0;
        p.continue_after = 0;
        Sim::new(p)
    }

    /// A response whose only next call is `call`, now.
    fn now(call: Skeleton) -> Guidance {
        Guidance {
            items: vec![NextCall {
                rule: Rule::HeldFlag,
                when: Moment::Now,
                once: false,
                call,
                because: Because::RenewLimit,
            }],
            total: Total::Exact(1),
            end: None,
        }
    }

    /// A task of team 0.
    fn task(sim: &mut Sim, name: &str) -> NodeId {
        let dir = sim.directory.clone();
        let s = sim.open(&dir, None, None, "directory").expect("a session");
        let team = sim.teams[0].name.clone();
        let out = sim.f.call_in(
            &dir,
            s,
            &cmds::create(path(&["work", &team, name]), "a task", |_| {}),
        );
        let (n, _) = Sim::created(out).expect("created");
        sim.add_task(n, Some(0), None, 60, false);
        n
    }

    /// A worker of team 0 that holds `n`.
    fn worker(sim: &mut Sim, n: NodeId) -> usize {
        sim.new_worker(Some(0), Start::Next, sim.f.now);
        let w = sim.workers.len() - 1;
        let caps = Some([sim.teams[0].label.clone()].into());
        let c = sim.workers[w].class.clone();
        sim.workers[w].sess = sim.open(&c, caps, None, "worker");
        let take = Cmd::Hold(HoldCmd::Take {
            target: NodeRef::Id(n),
            lease: None,
        });
        let out = sim.wcall(w, &take);
        let Ok(Done {
            reply: Reply::Taken { token, .. },
            ..
        }) = out
        else {
            panic!("{out:?}");
        };
        sim.hold(w, token);
        w
    }

    /// The worker's next step follows `call`.
    fn follow(sim: &mut Sim, w: usize, call: Skeleton) {
        sim.workers[w].guidance = Some(now(call));
        sim.workers[w].follow = true;
        sim.step(w);
    }

    fn step_at(sim: &Sim, w: usize, t: Time) -> bool {
        sim.q
            .iter()
            .any(|((at, _), a)| *at == t.0 && matches!(a, Act::Step(v) if *v == w))
    }

    #[test]
    fn a_now_item_is_followed_at_once_and_the_own_action_keeps_its_time() {
        let mut sim = sim();
        let a = task(&mut sim, "a");
        let w = worker(&mut sim, a);
        let own = sim.workers[w].own_at;
        assert!(own > sim.f.now.plus(FOLLOW_DELAY));
        sim.q.clear();
        sim.workers[w].guidance = Some(now(Skeleton::Split { parent: a }));
        sim.schedule(w);
        assert!(sim.workers[w].follow);
        assert!(step_at(&sim, w, sim.f.now.plus(FOLLOW_DELAY)));
        // After the followed call, the own action is still due when it was.
        sim.q.clear();
        let t = sim.f.now.plus(FOLLOW_DELAY);
        sim.f.advance(t);
        sim.step(w);
        assert!(!sim.workers[w].follow);
        assert_eq!(sim.workers[w].own_at, own);
        assert!(step_at(&sim, w, own));
    }

    #[test]
    fn a_followed_call_keeps_the_workers_bookkeeping() {
        let mut sim = sim();
        let a = task(&mut sim, "a");
        let w = worker(&mut sim, a);
        let first = sim.workers[w].token.expect("a hold");
        // A split's part is work of the parent's team; the worker keeps its hold.
        follow(&mut sim, w, Skeleton::Split { parent: a });
        let part = *sim.tasks.keys().next_back().expect("a task");
        assert_ne!(part, a);
        assert_eq!(sim.tasks[&part].team, Some(0));
        assert_eq!(sim.workers[w].token, Some(first));
        // The lease runs out: a followed take of the node again is the worker's new hold.
        let later = sim.f.now.plus(HOUR);
        sim.f.advance(later);
        assert!(!sim.f.s.holds.contains_key(&first));
        follow(&mut sim, w, Skeleton::Take { target: a });
        let again = sim.workers[w].token.expect("the hold taken again");
        assert_eq!(again.node, a);
        assert!(again.fence > first.fence);
        assert!(sim.f.s.holds.contains_key(&again));
        // `next` while it holds `a`: the part's hold is given back, and `a` stays its task.
        follow(&mut sim, w, Skeleton::Next);
        assert_eq!(sim.m.second_holds, 1);
        assert_eq!(sim.workers[w].token, Some(again));
        assert!(sim.f.s.ix.node.get(&part).is_none_or(|i| i.live == 0));
        // A followed close of its own hold ends its task.
        follow(
            &mut sim,
            w,
            Skeleton::Close {
                node: a,
                token: again,
                settles: Vec::new(),
                outcome: None,
            },
        );
        assert_eq!(sim.workers[w].token, None);
        assert_eq!(sim.workers[w].tasks, 1);
        assert!(!sim.workers[w].alive);
        assert!(!matches!(sim.f.s.nodes[&a].life, forum_core::Life::Open));
        assert_eq!(sim.m.follows, 4);
        assert!(
            sim.m.followed.keys().all(|(_, _, o)| o == "made"),
            "{:?}",
            sim.m.followed
        );
    }

    #[test]
    fn a_hold_lost_before_a_followed_call_is_no_longer_the_workers() {
        let mut sim = sim();
        let a = task(&mut sim, "a");
        let w = worker(&mut sim, a);
        let token = sim.workers[w].token.expect("a hold");
        let later = sim.f.now.plus(HOUR);
        sim.f.advance(later);
        follow(
            &mut sim,
            w,
            Skeleton::Checkpoint {
                token,
                intent_done: false,
                intent: false,
            },
        );
        assert_eq!(sim.workers[w].token, None);
        assert_eq!(sim.workers[w].tasks, 0);
        let key = (
            "held_flag",
            "hold checkpoint",
            String::from("refused: StaleToken"),
        );
        assert_eq!(sim.m.followed.get(&key), Some(&1), "{:?}", sim.m.followed);
    }
}
