//! The forum in process: the core's state, the simulated clock, and the shell's pipeline for one
//! call (D.8.1) without the shell: settle to `now`, `apply`, settle again. Every event goes to the
//! observer; every call's next calls (`query::guide`, D.8.26) are kept for the guided agents.

use std::collections::BTreeMap;

use forum_core::query::guide::{Guidance, Moment};
use forum_core::query::{self, NoMeter};
use forum_core::{
    ClassName, Cmd, Ctx, Done, Error, Params, SessionCtx, SessionId, Settled, State, Time, apply,
    evolve_mut, settle,
};

use crate::observe::Observer;

/// The forum and what the run counted about its calls.
#[derive(Clone, Debug)]
pub struct Forum {
    /// The core's state.
    pub s: State,
    /// The simulated clock.
    pub now: Time,
    /// What the events told.
    pub obs: Observer,
    /// Calls per command.
    pub calls: BTreeMap<String, u64>,
    /// Errors per command and code.
    pub errors: BTreeMap<String, u64>,
    /// Warnings per code.
    pub warnings: BTreeMap<String, u64>,
    /// Keys still queued after their own firing (`Settled::Stuck`): a core bug.
    pub stuck: Vec<String>,
    /// Responses computed with next calls.
    pub guided_responses: u64,
    /// Responses with at least one next call.
    pub guided_with_items: u64,
    /// Responses whose first item is a `now` call.
    pub guided_with_now: u64,
}

/// The name of a command for the counters: `tool action`.
#[must_use]
pub fn cmd_name(cmd: &Cmd) -> &'static str {
    use forum_core::{ControlCmd, HoldCmd, NodeCmd, SessionCmd, TalkCmd, ViewCmd};
    match cmd {
        Cmd::Session(SessionCmd::Hello { .. }) => "session hello",
        Cmd::Session(SessionCmd::Status { .. }) => "session status",
        Cmd::View(ViewCmd::Node { .. }) => "view node",
        Cmd::View(_) => "view other",
        Cmd::Node(NodeCmd::Create(_)) => "node create",
        Cmd::Node(NodeCmd::Update { .. }) => "node update",
        Cmd::Node(NodeCmd::Close { .. }) => "node close",
        Cmd::Node(NodeCmd::Merge { .. }) => "node merge",
        Cmd::Node(_) => "node other",
        Cmd::Hold(HoldCmd::Take { .. }) => "hold take",
        Cmd::Hold(HoldCmd::Next { .. }) => "hold next",
        Cmd::Hold(HoldCmd::Checkpoint { .. }) => "hold checkpoint",
        Cmd::Hold(HoldCmd::Renew { .. }) => "hold renew",
        Cmd::Hold(HoldCmd::Release { .. }) => "hold release",
        Cmd::Hold(HoldCmd::Check { .. }) => "hold check",
        Cmd::Talk(TalkCmd::FromUser { .. }) => "talk from_user",
        Cmd::Talk(TalkCmd::ToUser { .. }) => "talk to_user",
        Cmd::Talk(TalkCmd::Route { .. }) => "talk route",
        Cmd::Talk(TalkCmd::Said { .. }) => "talk said",
        Cmd::Talk(TalkCmd::Heard { .. }) => "talk heard",
        Cmd::Talk(TalkCmd::Ask { .. }) => "talk ask",
        Cmd::Talk(TalkCmd::Decide { .. }) => "talk decide",
        Cmd::Talk(TalkCmd::Say { .. }) => "talk say",
        Cmd::Talk(_) => "talk other",
        Cmd::Control(ControlCmd::Health) => "control health",
        Cmd::Control(_) => "control other",
        Cmd::Admin(_) => "admin",
    }
}

impl Forum {
    /// A forum whose log starts with Genesis at `start`.
    #[must_use]
    pub fn new(params: Params, start: Time, obs: Observer) -> Forum {
        let mut s = State::default();
        let g = forum_core::genesis(params, start);
        evolve_mut(&mut s, &g);
        Forum {
            s,
            now: start,
            obs,
            calls: BTreeMap::new(),
            errors: BTreeMap::new(),
            warnings: BTreeMap::new(),
            stuck: Vec::new(),
            guided_responses: 0,
            guided_with_items: 0,
            guided_with_now: 0,
        }
    }

    /// Moves the clock to `t` (never backwards) and fires every deadline due, as the shell's
    /// pre-settle or the idle writer would (D.8.2, FQ-6: the events are the same).
    pub fn advance(&mut self, t: Time) {
        self.now = self.now.max(t);
        let batch = self.s.params.settle_batch.max(1);
        loop {
            let (evs, st) = settle(&mut self.s, self.now, batch);
            self.obs.events(&evs);
            match st {
                Settled::Done => break,
                Settled::More => {}
                Settled::Stuck(k) => {
                    self.stuck.push(format!("{k:?}"));
                    break;
                }
            }
        }
    }

    /// One call at the clock, through the shell's pipeline: pre-settle, `apply`, post-settle. The
    /// next calls of the response are returned with it, for the guided agents.
    pub fn call(
        &mut self,
        class: &ClassName,
        session: SessionCtx,
        cmd: &Cmd,
    ) -> (Result<Done, Error>, Guidance) {
        self.advance(self.now);
        let ctx = Ctx {
            now: self.now,
            class: class.clone(),
            caps: None,
            session,
            op: None,
        };
        let (evs, out) = apply(&mut self.s, &ctx, cmd);
        self.obs.events(&evs);
        let name = cmd_name(cmd);
        *self.calls.entry(name.to_string()).or_default() += 1;
        match &out {
            Ok(d) => {
                for w in &d.warnings {
                    *self.warnings.entry(format!("{:?}", w.code)).or_default() += 1;
                }
            }
            Err(e) => {
                *self
                    .errors
                    .entry(format!("{name}: {:?} {:?}", e.code, e.hint))
                    .or_default() += 1;
            }
        }
        self.advance(self.now);
        let sess = match (&out, session) {
            (Ok(d), _) if d.opened.is_some() => d.opened,
            (_, SessionCtx::Existing(x)) => Some(x),
            _ => None,
        };
        let g = query::guide(&self.s, sess, self.now, cmd, &out, &mut NoMeter);
        self.guided_responses += 1;
        if !g.items.is_empty() {
            self.guided_with_items += 1;
        }
        if g.items.first().is_some_and(|i| i.when == Moment::Now) {
            self.guided_with_now += 1;
        }
        (out, g)
    }

    /// `call` in an existing session, without the next calls.
    pub fn call_in(
        &mut self,
        class: &ClassName,
        sess: SessionId,
        cmd: &Cmd,
    ) -> Result<Done, Error> {
        self.call(class, SessionCtx::Existing(sess), cmd).0
    }
}
