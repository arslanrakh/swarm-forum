//! The replay driver (design D.10.8): it reads each ITF state's `last_action` (the action name of
//! D.10.4, the session, the class, the `op`, `long`, the command, and the outcome), sends the
//! command to `forum-core` through the shell's pipeline (`settle`, `apply_with`, `settle`), and
//! compares the outcome and the projected state with the model's state after the step.
//!
//! Quint 0.33's `quint run --out-itf` and `quint test --out-itf` write state variables only, and
//! Apalache and TLC counterexamples carry no action names, so the driver dispatches on the model's
//! own `last_action` variable, which every backend writes (D.14.5).
//!
//! Beside the comparison, the core's own executable invariants run on every replayed step:
//! `check_step` after each command, `check_fire` after each firing, and `check_invariants` after
//! each step (D.9.1). A violation is reported with the divergences.
//!
//! **Gaps.** A command that a later package owns is answered by the core's placeholder,
//! `invalid(BadParams)` (D.13.1), a helper of a later package emits nothing (WP-D5a's
//! `create_uq` and `add_digest`), and a merge of a series into its own open occurrence, which the
//! model refuses with `invalid(MergeIntoOwnOccurrence)` (1.5), is merged by a core without WP-D6.
//! A step that reaches any of them is a *gap*, not a divergence: the replay of the trace stops
//! there and says why, unless `Options::gaps_diverge` is set. Everything else that differs is a
//! divergence.

use std::collections::{BTreeMap, BTreeSet};

use forum_core::{
    AskTo, ClassName, Cmd, ControlCmd, CreateArgs, Ctx, Done, Eid, Error, ErrorCode, HideArg, Hint,
    HoldCmd, Key, KeyChange, KeyStr, Label, LinkRel, Name, NextQuery, NextReply, NodeCmd, NodeId,
    NodeRef, NormVer, OpCtx, OpId, Outcome, Params, Patch, Path, PrincipalKey, ReleaseReason,
    Reply, ReqDigest, SaidKind, SessionCmd, SessionCtx, SessionId, Settled, State, TalkCmd, Text,
    TieBreak, Time, Token, ViewCmd, WaitArg, When, apply_with, check_fire, check_invariants,
    check_step, evolve_mut, genesis, settle,
};

use crate::itf::{R, Trace, V, diff, var};
use crate::project::{T0, at_tick, model_view, project, ticks, time};

/// What a trace replays against: the class configuration and the tie-break of `next`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Config {
    /// The classes in the order the seed opens one session for each (D.10.7).
    pub classes: Vec<ClassName>,
    /// `refusal.quorum`: 2 with two classes, 1 with one (D.10.5).
    pub quorum: u32,
    /// The seed creates the series `/s` (the `recurring` seed and the instances).
    pub series: bool,
    /// `TieBreak::Id` for the slices (`USE_FNV = false`), `TieBreak::Fnv` for the instances.
    pub tie: TieBreak,
}

impl Config {
    /// The configuration of a trace: the classes from the seed's sessions, the quorum from the
    /// module name (`…_two` and `two_class`: 2; otherwise 1), the series from the seed's nodes, and
    /// FNV for the instances of `model/instances` (D.10.6).
    ///
    /// # Errors
    ///
    /// When the first state is not a seed.
    pub fn infer(t: &Trace) -> R<Config> {
        let st = t.states.first().ok_or("an empty trace")?;
        let s = var(st, "s")?;
        let mut classes = Vec::new();
        for x in s.get("sessions")?.as_map()?.values() {
            let c = x.get("class")?.as_str()?;
            classes.push(ClassName::new(c).ok_or_else(|| format!("class {c}"))?);
        }
        let module = t.module.as_str();
        let quorum = if module.ends_with("_two") || module == "two_class" {
            2
        } else {
            1
        };
        let series = s.get("nodes")?.as_map()?.values().any(|n| {
            n.get("role")
                .and_then(V::as_tag)
                .is_ok_and(|(r, _)| r == "Series")
        });
        let tie = if ["two_class", "one_class", "no_user"].contains(&module) {
            TieBreak::Fnv
        } else {
            TieBreak::Id
        };
        Ok(Config {
            classes,
            quorum,
            series,
            tie,
        })
    }
}

/// The replay's parameters: §22's durations scaled to ticks of one minute, with their order kept
/// (D.10.5), and the scaled counts.
#[must_use]
pub fn scaled_params(quorum: u32) -> Params {
    Params {
        lease_first: ticks(3),
        lease_default: ticks(4),
        lease_max: ticks(7),
        lease_ceiling: ticks(9),
        renew_max: 2,
        hold_per_session: 2,
        presence_silence: ticks(3),
        presence_resolution: ticks(1),
        session_retain: ticks(10),
        op_retain: ticks(10),
        commitment_followup: ticks(6),
        question_recheck: ticks(8),
        question_max_rechecks: 2,
        user_question_due: ticks(11),
        user_question_due_absent: ticks(5),
        user_question_max_pending: 20,
        user_gap_window: ticks(13),
        deliver_window: ticks(10),
        leave_better_max: 3,
        refusal_ttl: ticks(12),
        refusal_window: ticks(12),
        refusal_backoff: ticks(5),
        refusal_quorum: quorum,
        too_big_attempts: 2,
        pause_ttl: ticks(7),
        archive_after: ticks(12),
        coverage_window: ticks(13),
        capacity_window: ticks(12),
        offer_stale: Some(ticks(14)),
        text_max: 4_096,
        tree_max_depth: 4,
        tree_max_children: 3,
        ..Params::default()
    }
}

/// How a replay treats gaps.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Options {
    /// Report a gap as a divergence instead of stopping the trace there.
    pub gaps_diverge: bool,
}

/// How a trace's replay ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// Every step replayed with no divergence.
    Pass,
    /// The replay stopped at a step that needs a later package (module documentation).
    Gap {
        /// The state index of the step.
        step: usize,
        /// The model's action.
        action: String,
        /// Which package.
        reason: String,
    },
    /// The core and the model disagree.
    Divergence {
        /// The state index of the step (0: the seed).
        step: usize,
        /// The model's action, with its command.
        action: String,
        /// What differs.
        details: Vec<String>,
    },
}

/// The replay of one trace.
#[derive(Clone, Debug)]
pub struct Report {
    /// The trace's name.
    pub name: String,
    /// Its states, the seed included.
    pub states: usize,
    /// The steps replayed and compared, the seed included.
    pub replayed: usize,
    /// The replayed actions by name and result (`act_take ok`, `act_take err`, …).
    pub actions: BTreeMap<String, usize>,
    /// How it ended.
    pub verdict: Verdict,
    /// The core's state after the last step replayed; None when the seed failed.
    pub state: Option<State>,
}

impl Report {
    /// The verdict, rendered for a test failure.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.verdict {
            Verdict::Pass => format!(
                "{}: {} of {} states replayed",
                self.name, self.replayed, self.states
            ),
            Verdict::Gap {
                step,
                action,
                reason,
            } => format!(
                "{}: stopped at state {step} of {} ({action}): {reason}",
                self.name, self.states
            ),
            Verdict::Divergence {
                step,
                action,
                details,
            } => format!(
                "{}: DIVERGENCE at state {step} of {} after {action}:\n  {}",
                self.name,
                self.states,
                details.join("\n  ")
            ),
        }
    }
}

/// One step's result, before the state comparison.
enum Sent {
    /// Compare the state, and the outcome when the model gives one.
    Compare(Option<String>),
    /// A gap: stop.
    Gap(String),
}

/// The core's side of a replay: the state, the clock in ticks, and the request digests it gave
/// the model's commands.
pub struct Driver {
    /// The core's state.
    pub s: State,
    /// The shell's clock, in ticks since `T0` (D.10.2).
    pub clock: u64,
    /// The configuration.
    pub cfg: Config,
    digests: BTreeMap<V, ReqDigest>,
    cmds: BTreeMap<ReqDigest, V>,
    /// Problems the core's own checks found (`check_step`, `check_fire`, `check_invariants`).
    checks: Vec<String>,
}

impl Driver {
    /// Genesis at `T0` with the scaled params, then the seed of D.10.7: one `session hello` per
    /// class, `/w1` (key `Conv("c1")`) and `/w2` (key `Conv("c2")`) by session 1, capacity 1, not
    /// taken, tagged `offer`.
    ///
    /// # Errors
    ///
    /// When a seed command fails.
    pub fn new(cfg: Config) -> R<Driver> {
        let mut s = State::default();
        evolve_mut(&mut s, &genesis(scaled_params(cfg.quorum), T0));
        let mut d = Driver {
            s,
            clock: 0,
            cfg,
            digests: BTreeMap::new(),
            cmds: BTreeMap::new(),
            checks: Vec::new(),
        };
        let classes = d.cfg.classes.clone();
        for c in &classes {
            let cmd = V::tag(
                "CHello",
                V::rec([("caps", V::Set(BTreeSet::new())), ("principal", V::str(""))]),
            );
            d.call(SessionCtx::New, c, "", &cmd, false)?;
        }
        let first = classes.first().ok_or("no class")?.clone();
        if d.cfg.series {
            // The `recurring` seed's `/s` (D.10.7) needs WP-D6's `repeat`, and recurrence is
            // outside D.0.3's key parts.
            return Err("the seed series `/s` is not replayed (WP-D6)".to_owned());
        }
        for (name, conv) in [("w1", "c1"), ("w2", "c2")] {
            let cmd = V::tag(
                "CCreate",
                V::rec([
                    ("parent", V::Int(0)),
                    ("name", V::tag("Lit", V::str(name))),
                    (
                        "keys",
                        V::Set(BTreeSet::from([V::tag("Conv", V::str(conv))])),
                    ),
                    ("take", V::Bool(false)),
                    ("capacity", V::Int(1)),
                    ("owners", V::Set(BTreeSet::new())),
                    ("repeat", V::unit("RepKeep")),
                    ("tags", V::Set(BTreeSet::from([V::str("offer")]))),
                ]),
            );
            d.call(SessionCtx::Existing(SessionId(1)), &first, "", &cmd, false)?;
        }
        Ok(d)
    }

    fn now(&self) -> Time {
        at_tick(self.clock)
    }

    /// The request digest of a model command: the model's digest is the command itself (D.10.3),
    /// so each distinct command gets its own.
    fn digest_of(&mut self, cmd: &V) -> ReqDigest {
        if let Some(d) = self.digests.get(cmd) {
            return *d;
        }
        let n = u64::try_from(self.digests.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut b = [0u8; 32];
        if let Some(head) = b.get_mut(..8) {
            head.copy_from_slice(&n.to_be_bytes());
        }
        let d = ReqDigest(b);
        self.digests.insert(cmd.clone(), d);
        self.cmds.insert(d, cmd.clone());
        d
    }

    /// The path of an existing node, from the root.
    fn path_of(&self, n: NodeId) -> R<Vec<Name>> {
        let mut out = Vec::new();
        let mut at = n;
        for _ in 0..64 {
            let node = self
                .s
                .nodes
                .get(&at)
                .ok_or_else(|| format!("no node #{}", at.0))?;
            match &node.place {
                None => {
                    out.reverse();
                    return Ok(out);
                }
                Some(p) => {
                    out.push(p.name.clone());
                    at = p.parent;
                }
            }
        }
        Err(format!("the path of #{} is too deep", n.0))
    }

    /// Settles to the clock, one firing at a time with `check_fire` after each (D.8.2).
    fn settle_to_clock(&mut self) -> R<()> {
        let now = self.now();
        loop {
            let Some(key) = self.s.ix.deadlines.first().cloned() else {
                return Ok(());
            };
            if key.at > now {
                return Ok(());
            }
            let before = self.s.clone();
            let (evs, st) = settle(&mut self.s, now, 1);
            if let Settled::Stuck(k) = st {
                return Err(format!("settle is stuck at {k:?}"));
            }
            if let Err(v) = check_fire(&before, &key, &evs, &self.s) {
                self.checks.push(format!("check_fire of {key:?}: {v:?}"));
            }
        }
    }

    /// One call through the shell's pipeline (D.10.8): settle, `apply_with` with `check_step`,
    /// settle. Returns the outcome as `outcome_string` renders it.
    fn call(
        &mut self,
        session: SessionCtx,
        class: &ClassName,
        op: &str,
        cmd: &V,
        long: bool,
    ) -> R<String> {
        let rust = self.to_rust(cmd, long)?;
        let op = if op.is_empty() {
            None
        } else {
            Some(OpCtx {
                id: OpId::new(op).ok_or_else(|| format!("op {op}"))?,
                digest: self.digest_of(cmd),
            })
        };
        let ctx = Ctx {
            now: self.now(),
            class: class.clone(),
            caps: None,
            session,
            op,
        };
        self.settle_to_clock()?;
        let before = self.s.clone();
        let (evs, out) = apply_with(&mut self.s, &ctx, &rust, self.cfg.tie);
        if let Err(v) = check_step(&before, &ctx, &rust, &out, &evs, &self.s) {
            self.checks.push(format!("check_step: {v:?}"));
        }
        self.settle_to_clock()?;
        Ok(outcome_string(&out))
    }

    /// The model command as a `forum_core::Cmd` (D.10.8): node references as `#id`, content `v` as
    /// the title `c<v>`, a status as `s<v>`, a digest as `d<v>`, principals `p` as `test:p`, keys as
    /// `project::key` reads them, and with `long` the command's first free text at `text.max + 1`
    /// bytes.
    ///
    /// # Errors
    ///
    /// For a command the driver does not map (the recurring slice's `repeat`, the user's commands,
    /// and of the review's all but `object` and `resolve`, which 1.10's `review_two` replays): the
    /// replay of D.0.3 covers the key parts only.
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per command of D.10.4 that the key slices send"
    )]
    pub fn to_rust(&self, cmd: &V, long: bool) -> R<Cmd> {
        let (tag, a) = cmd.as_tag()?;
        let long_text = Text("x".repeat(usize::try_from(self.s.params.text_max).unwrap_or(0) + 1));
        let text = |plain: &str| {
            if long {
                long_text.clone()
            } else {
                Text(plain.to_owned())
            }
        };
        let id = |v: &V| -> R<NodeRef> { Ok(NodeRef::Id(NodeId(v.as_u64()?))) };
        let token = |v: &V| -> R<Token> {
            let (n, f) = v.as_int_pair()?;
            Ok(Token {
                node: NodeId(u64::try_from(n).map_err(|e| e.to_string())?),
                fence: forum_core::Fence(u64::try_from(f).map_err(|e| e.to_string())?),
            })
        };
        let labels = |v: &V| -> R<BTreeSet<Label>> {
            v.as_set()?
                .iter()
                .map(|x| {
                    let s = x.as_str()?;
                    Label::new(s).ok_or_else(|| format!("label {s}"))
                })
                .collect()
        };
        let node_refs = |v: &V| -> R<Vec<NodeRef>> { v.as_set()?.iter().map(id).collect() };
        let opt_key = |v: &V| -> R<Option<Key>> {
            match v.as_tag()? {
                ("NoKey", _) => Ok(None),
                ("SomeKey", k) => Ok(Some(key_of(k)?)),
                (t, _) => Err(format!("OptKey {t}")),
            }
        };
        Ok(match tag {
            "CHello" => {
                let caps = labels(a.get("caps")?)?;
                let p = a.get("principal")?.as_str()?;
                Cmd::Session(SessionCmd::Hello {
                    intent: None,
                    caps: (!caps.is_empty()).then_some(caps),
                    horizon: None,
                    principal: if p.is_empty() {
                        None
                    } else {
                        Some(principal_of(p)?)
                    },
                    from_user: None,
                })
            }
            "CCreate" => {
                if a.get("repeat")?.as_tag()?.0 != "RepKeep" {
                    return Err(
                        "`repeat` is not replayed (WP-D6, outside D.0.3's key parts)".to_owned(),
                    );
                }
                let parent = NodeId(a.get("parent")?.as_u64()?);
                let mut path = self.path_of(parent)?;
                path.push(name_of(a.get("name")?)?);
                let mut keys: Vec<Key> = a
                    .get("keys")?
                    .as_set()?
                    .iter()
                    .map(key_of)
                    .collect::<R<_>>()?;
                keys.sort_by(|x, y| x.key.cmp(&y.key));
                Cmd::Node(NodeCmd::Create(CreateArgs {
                    path: Path(path),
                    keys,
                    kind: None,
                    title: text("c1"),
                    body: None,
                    digest: None,
                    fields: BTreeMap::new(),
                    tags: labels(a.get("tags")?)?,
                    capacity: Some(
                        u32::try_from(a.get("capacity")?.as_u64()?).map_err(|e| e.to_string())?,
                    ),
                    needs: None,
                    est: None,
                    priority: None,
                    due: None,
                    depends_on: Vec::new(),
                    owners: node_refs(a.get("owners")?)?,
                    repeat: None,
                    agreements: None,
                    request: None,
                    take: Some(a.get("take")?.as_bool()?),
                }))
            }
            "CUpdate" => {
                if a.get("repeat")?.as_tag()?.0 != "RepKeep" {
                    return Err(
                        "`repeat` is not replayed (WP-D6, outside D.0.3's key parts)".to_owned(),
                    );
                }
                let int = |f: &str| -> R<i64> { a.get(f)?.as_int() };
                let coded = |f: &str, prefix: char| -> R<Option<Text>> {
                    let v = int(f)?;
                    Ok((v >= 0).then(|| {
                        if v == 0 {
                            Text::default()
                        } else {
                            Text(format!("{prefix}{v}"))
                        }
                    }))
                };
                let content = int("content")?;
                let title = if long {
                    Some(long_text.clone())
                } else {
                    (content >= 0).then(|| Text(format!("c{content}")))
                };
                let ids_change = |f: &str| -> R<Option<Vec<NodeRef>>> {
                    match a.get(f)?.as_tag()? {
                        ("IdsKeep", _) => Ok(None),
                        ("IdsSet", xs) => Ok(Some(node_refs(xs)?)),
                        (t, _) => Err(format!("IdsChange {t}")),
                    }
                };
                let due = match a.get("due")?.as_tag()? {
                    ("DueKeep", _) => forum_core::Change::Keep,
                    ("DueClear", _) => forum_core::Change::Clear,
                    ("DueSet", t) => forum_core::Change::Set(When::At(at_tick(t.as_u64()?))),
                    (t, _) => return Err(format!("DueChange {t}")),
                };
                let cap = int("capacity")?;
                let dver = int("dver")?;
                Cmd::Node(NodeCmd::Update {
                    at: id(a.get("node")?)?,
                    version: forum_core::Version(a.get("ver")?.as_u64()?),
                    digest_version: (dver >= 0)
                        .then(|| u64::try_from(dver).map(forum_core::Version))
                        .transpose()
                        .map_err(|e| e.to_string())?,
                    token: None,
                    patch: Patch {
                        title,
                        status: coded("status", 's')?,
                        digest: coded("digest", 'd')?,
                        capacity: (cap >= 0)
                            .then(|| u32::try_from(cap))
                            .transpose()
                            .map_err(|e| e.to_string())?,
                        needs_add: labels(a.get("needs_add")?)?,
                        needs_remove: labels(a.get("needs_remove")?)?,
                        tags_add: labels(a.get("tags_add")?)?,
                        tags_remove: labels(a.get("tags_remove")?)?,
                        due,
                        depends_on: ids_change("depends")?,
                        owners: ids_change("owners")?,
                        ..Patch::default()
                    },
                })
            }
            "CLink" => Cmd::Node(NodeCmd::Link {
                at: id(a.get("node")?)?,
                rel: link_rel_of(a.get("rel")?)?,
                to: id(a.get("to")?)?,
            }),
            "CKeyAdd" => Cmd::Node(NodeCmd::Key {
                at: id(a.get("node")?)?,
                change: KeyChange::Add(key_of(a.get("key")?)?),
            }),
            "CKeyRemove" => Cmd::Node(NodeCmd::Key {
                at: id(a.get("node")?)?,
                change: KeyChange::Remove(key_of(a.get("key")?)?.key),
            }),
            "CClose" => Cmd::Node(NodeCmd::Close {
                at: id(a.get("node")?)?,
                outcome: outcome_of(a.get("outcome")?)?,
                summary: text("x"),
                verified: None,
                // The model's close always names its `settles` (an empty set settles none); the
                // default of §5 (1.7) is not modelled.
                settles: Some(
                    a.get("settles")?
                        .as_set()?
                        .iter()
                        .map(|r| r.as_u64().map(Eid))
                        .collect::<R<_>>()?,
                ),
                learnings: None,
                token: match a.get("token")?.as_tag()? {
                    ("NoToken", _) => None,
                    ("SomeToken", t) => Some(token(t)?),
                    (t, _) => return Err(format!("OptToken {t}")),
                },
                unsent: BTreeSet::new(),
            }),
            // 1.12 (§9.3): a retract, of a node or an entry; the shell adds an entry's node from
            // its `entries` table, as for `hide`; the retract of an open question to a person
            // withdraws it.
            "CRetract" => match a.as_tag()? {
                ("TNode", n) => Cmd::Talk(TalkCmd::Retract {
                    target: forum_core::RetractArg::Node(id(n)?),
                    text: text("x"),
                    reason: text("x"),
                }),
                ("TEntry", e) => {
                    let eid = Eid(e.as_u64()?);
                    let node = self
                        .s
                        .entries
                        .get(&eid)
                        .map_or(forum_core::ROOT, |m| m.node);
                    Cmd::Talk(TalkCmd::Retract {
                        target: forum_core::RetractArg::Entry { eid, node },
                        text: text("x"),
                        reason: text("x"),
                    })
                }
                (t, _) => return Err(format!("BasisTarget {t}")),
            },
            "CReopen" => Cmd::Node(NodeCmd::Reopen {
                at: id(a)?,
                reason: text("x"),
            }),
            "CMove" => {
                let mut path = self.path_of(NodeId(a.get("parent")?.as_u64()?))?;
                path.push(name_of(a.get("name")?)?);
                Cmd::Node(NodeCmd::Move {
                    at: id(a.get("node")?)?,
                    to: Path(path),
                })
            }
            "CMerge" => Cmd::Node(NodeCmd::Merge {
                from: id(a.get("from")?)?,
                into: id(a.get("into")?)?,
            }),
            "CHide" => {
                let eid = Eid(a.as_u64()?);
                // The shell adds the entry's node from its `entries` table (D.5.2); the model names
                // only kept entries, or an eid that names none (`v_hide`).
                let node = self
                    .s
                    .entries
                    .get(&eid)
                    .map_or(forum_core::ROOT, |m| m.node);
                Cmd::Node(NodeCmd::Hide {
                    target: HideArg::Entry { eid, node },
                    reason: text("x"),
                })
            }
            "CTake" => {
                let lease = a.get("lease")?.as_int()?;
                Cmd::Hold(HoldCmd::Take {
                    target: id(a.get("node")?)?,
                    lease: (lease >= 0)
                        .then(|| u64::try_from(lease).map(ticks))
                        .transpose()
                        .map_err(|e| e.to_string())?,
                })
            }
            "CNext" => Cmd::Hold(HoldCmd::Next {
                query: NextQuery::default(),
                lease: None,
            }),
            "CCheckpoint" => Cmd::Hold(HoldCmd::Checkpoint {
                token: token(a.get("token")?)?,
                done: text("x"),
                next_step: Text("x".to_owned()),
                intent: a.get("intent")?.as_bool()?.then(|| Text("i".to_owned())),
                intent_done: a
                    .get("intent_done")?
                    .as_bool()?
                    .then(|| Text("d".to_owned())),
                files: Vec::new(),
            }),
            "CCheckIntent" => Cmd::Hold(HoldCmd::Check {
                target: id(a)?,
                token: None,
                intent_done: Some(text("d")),
            }),
            "CRenew" => Cmd::Hold(HoldCmd::Renew {
                token: token(a)?,
                why: text("x"),
            }),
            "CRelease" => Cmd::Hold(HoldCmd::Release {
                token: token(a.get("token")?)?,
                done: text("x"),
                next_step: Text("x".to_owned()),
                reason: match a.get("reason")?.as_tag()?.0 {
                    "Handoff" => ReleaseReason::Handoff,
                    "Blocked" => ReleaseReason::Blocked,
                    "Cannot" => ReleaseReason::Cannot,
                    r => return Err(format!("ReleaseReason {r}")),
                },
                waits_for: match a.get("wait")?.as_tag()? {
                    ("WaNone", _) => None,
                    ("WaQuestion", q) => Some(WaitArg::Question(Eid(q.as_u64()?))),
                    ("WaNode", m) => Some(WaitArg::Node(id(m)?)),
                    ("WaTime", t) => Some(WaitArg::Time(When::At(at_tick(t.as_u64()?)))),
                    ("WaExternal", t) => Some(WaitArg::External {
                        what: Text("x".to_owned()),
                        recheck_after: When::At(at_tick(t.as_u64()?)),
                    }),
                    (t, _) => return Err(format!("WaitArg {t}")),
                },
                // 1.4 (R15): the model's `CRelease` may carry `intent_done` (WP-A2's follow-up);
                // without it, the release closes no intent.
                intent_done: match a.get("intent_done") {
                    Ok(v) => v.as_bool()?.then(|| Text("d".to_owned())),
                    Err(_) => None,
                },
                learnings: None,
                unsent: BTreeSet::new(),
                // 1.12 (§10.3): the requests its decision lists, after the release; a trace of a
                // model before 1.12 has none.
                settles: match a.get("settles") {
                    Ok(v) => v
                        .as_set()?
                        .iter()
                        .map(|r| r.as_u64().map(Eid))
                        .collect::<R<_>>()?,
                    Err(_) => BTreeSet::new(),
                },
            }),
            "CRequest" => Cmd::Talk(TalkCmd::Request {
                at: id(a.get("at")?)?,
                text: text("x"),
                origin: key_of(a.get("origin")?)?,
                where_: opt_key(a.get("where_key")?)?,
                requester: Text("x".to_owned()),
            }),
            "CSaid" => Cmd::Talk(TalkCmd::Said {
                at: id(a.get("at")?)?,
                where_: opt_key(a.get("where_key")?)?,
                text: text("x"),
                kind: match a.get("kind")?.as_tag()?.0 {
                    "SAck" => SaidKind::Ack,
                    "SReply" => SaidKind::Reply,
                    "SResult" => SaidKind::Result,
                    "SCorrection" => SaidKind::Correction,
                    "SQuestion" => SaidKind::Question,
                    k => return Err(format!("SaidKind {k}")),
                },
                corrects: None,
                seen: None,
            }),
            "CHeard" => {
                let q = a.get("answers")?.as_int()?;
                Cmd::Talk(TalkCmd::Heard {
                    at: id(a.get("at")?)?,
                    where_: None,
                    from: Text("x".to_owned()),
                    text: text("x"),
                    answers: (q >= 0)
                        .then(|| u64::try_from(q).map(Eid))
                        .transpose()
                        .map_err(|e| e.to_string())?,
                })
            }
            "CAskPerson" => Cmd::Talk(TalkCmd::Ask {
                at: id(a.get("at")?)?,
                text: text("x"),
                to: AskTo::Person {
                    where_: None,
                    blocking: a.get("blocking")?.as_bool()?,
                },
                again: a.get("again")?.as_bool()?,
            }),
            // 1.10: the review and note commands of `review_two` (§11.3, §13).
            "CObject" => Cmd::Talk(TalkCmd::Object {
                on: id(a)?,
                note: text("x"),
            }),
            // 1.11: `defer` (§11.3), a deferral, which decides nothing.
            "CResolve" => Cmd::Talk(TalkCmd::Resolve {
                objection: Eid(a.get("objection")?.as_u64()?),
                note: text("x"),
                upheld: a.get("upheld")?.as_bool()?,
                defer: a.get("defer")?.as_bool()?,
            }),
            "CSay" => Cmd::Talk(TalkCmd::Say {
                at: id(a.get("at")?)?,
                text: text("x"),
                to: match a.get("to")?.as_tag()? {
                    ("SayNone", _) => None,
                    ("SaySession", x) => {
                        Some(forum_core::AddresseeArg::Session(SessionId(x.as_u64()?)))
                    }
                    ("SayHolder", n) => Some(forum_core::AddresseeArg::Holder(id(n)?)),
                    ("SayHolders", n) => Some(forum_core::AddresseeArg::Holders(id(n)?)),
                    (t, _) => return Err(format!("SayTo {t} (not replayed)")),
                },
                expects: None,
                reply_to: None,
                until: None,
            }),
            "CPause" => Cmd::Control(ControlCmd::Pause {
                scope: id(a)?,
                reason: text("x"),
            }),
            "CUnpause" => Cmd::Control(ControlCmd::Unpause { scope: id(a)? }),
            "CView" => Cmd::View(ViewCmd::Node {
                targets: vec![id(a)?],
                view: None,
                page: None,
            }),
            t => {
                return Err(format!(
                    "the driver does not map {t} (outside D.0.3's key parts)"
                ));
            }
        })
    }

    /// Replays one step: the action recorded in `st`'s `last_action`.
    fn send(&mut self, la: &V) -> R<Sent> {
        let name = la.get("name")?.as_str()?;
        let cmd = la.get("cmd")?;
        match name {
            "act_tick" => {
                let (_, dt) = cmd.as_tag()?;
                self.clock = self.clock.saturating_add(dt.as_u64()?);
                self.settle_to_clock()?;
                Ok(Sent::Compare(None))
            }
            "act_tick_next" => {
                // The clock jumps to the first queued key (D.10.4), read from the core's queue.
                let Some(k) = self.s.ix.deadlines.first() else {
                    return Err("act_tick_next with no queued deadline in the core".to_owned());
                };
                let t = time(k.at)
                    .as_u64()
                    .map_err(|e| format!("first deadline {k:?}: {e}"))?;
                if t <= self.clock {
                    return Err(format!("the first deadline {k:?} is not after the clock"));
                }
                self.clock = t;
                self.settle_to_clock()?;
                Ok(Sent::Compare(None))
            }
            _ => {
                let class_s = la.get("class")?.as_str()?;
                let class = ClassName::new(class_s).ok_or_else(|| format!("class {class_s}"))?;
                let session = if name == "act_hello" {
                    SessionCtx::New
                } else {
                    SessionCtx::Existing(SessionId(la.get("sess")?.as_u64()?))
                };
                let op = la.get("op")?.as_str()?.to_owned();
                let long = la.get("long")?.as_bool()?;
                let out = self.call(session, &class, &op, cmd, long)?;
                if out == "Err(Invalid(BadParams))" {
                    return Ok(Sent::Gap(
                        "the core answers invalid(BadParams), the placeholder of a later package \
                         (D.13.1): WP-D4 (keys, link, move, merge, reopen, a joining create) or \
                         WP-D5 (talk)"
                            .to_owned(),
                    ));
                }
                if out.starts_with("Ok")
                    && model_outcome(la.get("outcome")?)?.as_deref()
                        == Some("Err(Invalid(MergeIntoOwnOccurrence))")
                {
                    return Ok(Sent::Gap(
                        "the model refuses a merge of a series into its own open occurrence \
                         (invalid(MergeIntoOwnOccurrence), 1.5) and the core merges it: WP-D6 \
                         (the occurrence arms of node merge)"
                            .to_owned(),
                    ));
                }
                Ok(Sent::Compare(Some(out)))
            }
        }
    }
}

/// A model key as the replay sends it (`project::key` reads it back).
fn key_of(k: &V) -> R<Key> {
    let s = match k.as_tag()? {
        ("Conv", c) => format!("chat:t/{}", c.as_str()?),
        ("Msg", m) => match m.as_seq()? {
            [c, n] => format!("chat:t/{}/{}", c.as_str()?, n.as_int()?),
            _ => return Err(format!("Msg {m}")),
        },
        ("Obj", o) => format!("lean:{}", o.as_str()?),
        ("User", e) => format!("user:e{}", e.as_int()?),
        ("Person", p) => {
            let p = p.as_str()?;
            if p == "user" {
                "person:user".to_owned()
            } else {
                format!("person:test:{p}")
            }
        }
        (t, _) => return Err(format!("the driver does not send a {t} key")),
    };
    let key = KeyStr::new(&s).ok_or_else(|| format!("key {s}"))?;
    Ok(Key {
        key,
        nv: NormVer(1),
    })
}

fn name_of(n: &V) -> R<Name> {
    match n.as_tag()? {
        ("Lit", s) => {
            let s = s.as_str()?;
            Name::new(s).ok_or_else(|| format!("name {s}"))
        }
        (t, _) => Err(format!("the driver does not send a {t} name")),
    }
}

fn principal_of(p: &str) -> R<PrincipalKey> {
    let s = if p == "user" {
        p.to_owned()
    } else {
        format!("test:{p}")
    };
    PrincipalKey::new(&s).ok_or_else(|| format!("principal {s}"))
}

fn outcome_of(o: &V) -> R<Outcome> {
    Ok(match o.as_tag()?.0 {
        "Done" => Outcome::Done,
        "Failed" => Outcome::Failed,
        "Dropped" => Outcome::Dropped,
        "Superseded" => Outcome::Superseded,
        "Missed" => Outcome::Missed,
        t => return Err(format!("Outcome {t}")),
    })
}

fn link_rel_of(r: &V) -> R<LinkRel> {
    Ok(match r.as_tag()?.0 {
        "Continues" => LinkRel::Continues,
        "DerivedFrom" => LinkRel::DerivedFrom,
        "ConflictsWith" => LinkRel::ConflictsWith,
        "Duplicate" => LinkRel::Duplicate,
        "SupersededBy" => LinkRel::SupersededBy,
        t => return Err(format!("LinkRel {t}")),
    })
}

/// The variant name of a `Debug` rendering: `EmptyText(Title)` is `EmptyText`.
fn variant<T: std::fmt::Debug>(x: &T) -> String {
    let s = format!("{x:?}");
    s.split(['(', ' ', '{']).next().unwrap_or("").to_owned()
}

/// An integer of a reply as the model lists it.
fn n(x: u64) -> i64 {
    i64::try_from(x).unwrap_or(i64::MAX)
}

fn tick_of(t: Time) -> i64 {
    time(t).as_int().unwrap_or(i64::MIN)
}

/// The ids of a reply as the model's `OutOk` lists them (`commands.qnt`, at each execution).
fn reply_ids(r: &Reply) -> Option<Vec<i64>> {
    Some(match r {
        Reply::Hello { session, .. } => vec![n(session.0)],
        Reply::Created { id, token, .. } => vec![n(id.0), token.map_or(-1, |t| n(t.fence.0))],
        Reply::Joined { existing, .. } => vec![n(existing.0)],
        Reply::Updated {
            version,
            digest_version,
        } => vec![n(version.0), n(digest_version.0)],
        Reply::Linked { signal, .. } | Reply::KeyExisting { signal, .. } => {
            vec![signal.map_or(-1, |x| n(x.0))]
        }
        Reply::KeyAdded { .. } => vec![-1],
        Reply::Closed { decision, .. } => vec![decision.map_or(-1, |x| n(x.0))],
        // 1.12 (§10.3): a release with `settles` names its decision, as the model's does.
        Reply::Released {
            decision: Some(d), ..
        } => vec![n(d.0)],
        // 1.12 (§9.3): a retract names its entry.
        Reply::Retracted { entry, .. } => vec![n(entry.0)],
        Reply::KeyRemoved
        | Reply::Reopened { .. }
        | Reply::Moved { .. }
        | Reply::Merged { .. }
        | Reply::Hidden
        | Reply::Released { .. }
        | Reply::Unpaused { .. }
        | Reply::ViewNodes { .. } => Vec::new(),
        Reply::Taken { node, token, .. } => vec![n(node.0), n(token.fence.0)],
        Reply::Next(NextReply::Hold { node, token, .. }) => vec![0, n(node.0), n(token.fence.0)],
        // 1.9 (§10.4): a review item is the model's kind 1 too, an item with no hold.
        Reply::Next(NextReply::Status { node, .. } | NextReply::Review { node, .. }) => {
            vec![1, n(node.0)]
        }
        Reply::Next(NextReply::Route { entry, .. }) => vec![2, n(entry.0)],
        Reply::Next(NextReply::Empty { .. }) => Vec::new(),
        Reply::Checkpointed { lease_until, .. } => vec![tick_of(*lease_until)],
        // 1.8 (§10.2): `CCheckIntent`'s `[entry, intent]`.
        Reply::HoldCheck {
            entry: Some(e),
            intent_done: Some(i),
            ..
        } => vec![n(e.0), n(i.0)],
        Reply::Renewed {
            lease_until,
            renews_left,
        } => vec![tick_of(*lease_until), i64::from(*renews_left)],
        Reply::Paused { until, .. } => vec![tick_of(*until)],
        // 1.10 (`review_two`): a note, an objection, and a resolve: its entry, the new objection
        // of a second verdict, or -1 when it records nothing.
        Reply::Say { entry } => vec![n(entry.0)],
        Reply::Objected { objection } => vec![n(objection.0)],
        Reply::ObjectionResolved { entry, .. } => vec![entry.map_or(-1, |e| n(e.0))],
        Reply::Request { request, node, .. } => vec![n(request.0), n(node.0)],
        Reply::Said {
            said,
            node,
            settled,
            ..
        } => {
            let mut v = vec![n(said.0), n(node.0)];
            v.extend(settled.iter().map(|r| n(r.0)));
            v
        }
        Reply::Heard {
            entry,
            node,
            answered,
            ..
        } => vec![n(entry.0), n(node.0), answered.map_or(-1, |q| n(q.0))],
        Reply::Asked {
            question,
            said,
            node,
            ..
        } => vec![n(question.0), said.map_or(-1, |x| n(x.0)), n(node.0)],
        Reply::Status
        | Reply::ViewFind { .. }
        | Reply::ViewFeed { .. }
        | Reply::ViewList { .. }
        | Reply::ViewHistory { .. }
        | Reply::ViewRequests { .. }
        | Reply::ViewUserLog { .. }
        | Reply::ViewConstitution
        | Reply::Agreements { .. }
        | Reply::HoldCheck { .. }
        | Reply::FromUser { .. }
        | Reply::ToUser { .. }
        | Reply::Routed { .. }
        | Reply::SaidConfirmed { .. }
        | Reply::Answered { .. }
        | Reply::Decided { .. }
        | Reply::Approved { .. }
        | Reply::Waited { .. }
        | Reply::Who { .. }
        | Reply::Health
        | Reply::Constitution
        | Reply::Export { .. }
        | Reply::ParamsSet
        | Reply::Replay => return None,
    })
}

/// A Rust outcome as the replay compares it: `Ok[ids]`, `Replay`, or `Err(code)` with the subject
/// of `limit`, `invalid`, and `not_found` (warnings are dropped, D.10.8).
#[must_use]
pub fn outcome_string(out: &Result<Done, Error>) -> String {
    match out {
        Ok(d) if d.reply == Reply::Replay => "Replay".to_owned(),
        Ok(d) => match reply_ids(&d.reply) {
            Some(ids) => format!("Ok{ids:?}"),
            None => format!("Ok({})", variant(&d.reply)),
        },
        Err(e) => match &e.hint {
            Hint::Limit(k) => format!("Err(Limit({}))", variant(k)),
            Hint::Invalid(w) => format!("Err(Invalid({}))", variant(w)),
            Hint::NotFound(w) => format!("Err(NotFound({}))", variant(w)),
            Hint::Full { .. }
            | Hint::HoldsLimit { .. }
            | Hint::Paused { .. }
            | Hint::StaleToken { .. }
            | Hint::StaleVersion { .. }
            | Hint::Cycle { .. }
            | Hint::Closed { .. }
            | Hint::OpIdConflict => format!("Err({})", variant::<ErrorCode>(&e.code)),
        },
    }
}

/// The model's outcome in `outcome_string`'s form; `None` for `init` and the time actions.
///
/// # Errors
///
/// When it is not a `CmdOutcome`.
pub fn model_outcome(o: &V) -> R<Option<String>> {
    let strip = |s: &str, p: &str| s.strip_prefix(p).unwrap_or(s).to_owned();
    Ok(match o.as_tag()? {
        ("OutNone", _) => None,
        ("OutReplay", _) => Some("Replay".to_owned()),
        ("OutOk", ids) => {
            let ids: Vec<i64> = ids.as_seq()?.iter().map(V::as_int).collect::<R<_>>()?;
            Some(format!("Ok{ids:?}"))
        }
        ("OutErr", e) => {
            let (code, sub) = e.as_tag()?;
            let code = strip(code, "Err");
            Some(match code.as_str() {
                "Limit" => format!("Err(Limit({}))", strip(sub.as_tag()?.0, "Lk")),
                "Invalid" => format!("Err(Invalid({}))", strip(sub.as_tag()?.0, "Why")),
                "NotFound" => format!("Err(NotFound({}))", strip(sub.as_tag()?.0, "What")),
                _ => format!("Err({code})"),
            })
        }
        (t, _) => return Err(format!("CmdOutcome {t}")),
    })
}

/// A step's action, short: `act_take(s2 a) CTake(…) op=o1 long`.
fn describe_action(la: &V) -> String {
    let get = |f: &str| la.get(f).map(ToString::to_string).unwrap_or_default();
    let name = la.get("name").and_then(V::as_str).unwrap_or("?");
    let mut s = format!(
        "{name} sess={} class={} {}",
        get("sess"),
        get("class"),
        get("cmd")
    );
    if la
        .get("op")
        .and_then(V::as_str)
        .is_ok_and(|o| !o.is_empty())
    {
        s.push_str(&format!(" op={}", get("op")));
    }
    if la.get("long").and_then(V::as_bool).unwrap_or(false) {
        s.push_str(" long");
    }
    s
}

/// The ids the model's step added to an entity map that Rust has not: the trace of a WP-D5a
/// helper (`create_uq`, `add_digest`) that the core does not run yet.
fn d5a_gap(prev: &V, post: &V, rust: &V) -> R<Option<String>> {
    for field in ["uquestions", "digest"] {
        let before: BTreeSet<&V> = prev.get(field)?.as_map()?.keys().collect();
        let added: Vec<&V> = post
            .get(field)?
            .as_map()?
            .keys()
            .filter(|k| !before.contains(k))
            .collect();
        let have = rust.get(field)?.as_set()?;
        if added.iter().any(|k| !have.contains(*k)) {
            return Ok(Some(format!(
                "the model's step adds {field} {}, which WP-D5a's helpers (create_uq, add_digest, \
                 e1_check) make and the core's stubs do not yet",
                added
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    Ok(None)
}

/// Compares the core with the model's state `st`: what differs, empty when they agree.
fn compare(d: &Driver, st: &BTreeMap<String, V>, out: Option<&str>) -> R<Vec<String>> {
    let la = var(st, "last_action")?;
    let mut details = Vec::new();
    if let Some(rust) = out {
        let model = model_outcome(la.get("outcome")?)?;
        if model.as_deref() != Some(rust) {
            details.push(format!(
                "outcome: model {} / rust {rust}",
                model.as_deref().unwrap_or("(none)")
            ));
        }
    }
    let clock = var(st, "clock")?.as_u64()?;
    if clock != d.clock {
        details.push(format!("clock: model {clock} / rust {}", d.clock));
    }
    let model = model_view(var(st, "s")?)?;
    let rust = project(&d.s, &d.cmds);
    details.extend(diff("s", &model, &rust).iter().map(ToString::to_string));
    Ok(details)
}

/// Replays a trace: the seed against state 0, then each later state's `last_action`.
#[must_use]
pub fn replay(t: &Trace, name: &str, opts: Options) -> Report {
    let mut rep = Report {
        name: name.to_owned(),
        states: t.states.len(),
        replayed: 0,
        actions: BTreeMap::new(),
        verdict: Verdict::Pass,
        state: None,
    };
    match Config::infer(t).and_then(Driver::new) {
        Ok(mut d) => {
            run(t, opts, &mut d, &mut rep);
            rep.state = Some(d.s);
        }
        Err(e) => rep.verdict = diverge(0, "init".to_owned(), vec![format!("seed: {e}")]),
    }
    rep
}

fn diverge(step: usize, action: String, details: Vec<String>) -> Verdict {
    Verdict::Divergence {
        step,
        action,
        details,
    }
}

/// The steps of `replay` from the seed on: it sets `rep.verdict` at the first gap or divergence.
fn run(t: &Trace, opts: Options, d: &mut Driver, rep: &mut Report) {
    for (i, st) in t.states.iter().enumerate() {
        let la = match var(st, "last_action") {
            Ok(la) => la.clone(),
            Err(e) => {
                rep.verdict = diverge(i, "?".to_owned(), vec![e]);
                return;
            }
        };
        let action = describe_action(&la);
        let sent = if i == 0 {
            Ok(Sent::Compare(None))
        } else {
            d.send(&la)
        };
        let out = match sent {
            Ok(Sent::Compare(out)) => out,
            Ok(Sent::Gap(reason)) if !opts.gaps_diverge => {
                rep.verdict = Verdict::Gap {
                    step: i,
                    action,
                    reason,
                };
                return;
            }
            Ok(Sent::Gap(reason)) => {
                rep.verdict = diverge(i, action, vec![format!("gap: {reason}")]);
                return;
            }
            Err(e) => {
                rep.verdict = diverge(i, action, vec![format!("driver: {e}")]);
                return;
            }
        };
        let mut details = match compare(d, st, out.as_deref()) {
            Ok(x) => x,
            Err(e) => vec![format!("driver: {e}")],
        };
        if !details.is_empty() && !opts.gaps_diverge && i > 0 {
            let prev = t.states.get(i - 1).and_then(|p| var(p, "s").ok());
            let gap = prev
                .zip(var(st, "s").ok())
                .and_then(|(p, q)| d5a_gap(p, q, &project(&d.s, &d.cmds)).ok().flatten());
            if let Some(reason) = gap {
                rep.verdict = Verdict::Gap {
                    step: i,
                    action,
                    reason,
                };
                return;
            }
        }
        let now = d.now().max(d.s.last_now);
        if let Err(v) = check_invariants(&d.s, now) {
            d.checks.push(format!("check_invariants: {v:?}"));
        }
        details.append(&mut d.checks);
        if !details.is_empty() {
            rep.verdict = diverge(i, action, details);
            return;
        }
        rep.replayed += 1;
        let ok = match out.as_deref() {
            None => "",
            Some(o) if o.starts_with("Ok") => " ok",
            Some("Replay") => " replay",
            Some(_) => " err",
        };
        let name = la.get("name").and_then(V::as_str).unwrap_or("?");
        *rep.actions.entry(format!("{name}{ok}")).or_default() += 1;
    }
}
