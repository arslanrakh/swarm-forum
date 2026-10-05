//! The masked arguments of a call to its `Cmd` (D.11.5's table). The shape is checked before this
//! runs (`check_form`), so each reader here only parses values; it still answers a missing or
//! mistyped argument with `invalid`, so that it is total on its own. Each refusal names one value:
//! the decoder runs this again with that value replaced, to name every value of the call that does
//! not parse (1.6, `super::build_all`).
//!
//! Defaults that the specification states (`take: true`, `capacity: 1`, `priority: 0`, `view:
//! brief`) are left to the core, so that the logged command shows what the agent sent (D.5.2).

use std::collections::{BTreeMap, BTreeSet};

use forum_core::{
    AddresseeArg, AgentAddressee, AskTo, BasisArg, BoardState, Change, Cmd, ControlCmd, CreateArgs,
    Dur, Eid, EntryType, Evidence, Expects, FindBy, Flag, FlagSet, FromUserArg, HideArg, HoldCmd,
    KeyChange, Label, LinkRel, NextQuery, NodeCmd, NodeRef, Patch, ReleaseReason, RepeatArg,
    RequestArg, RequesterFilter, RetractArg, RouteTo, SaidKind, Seq, SessionCmd, TalkCmd, Text,
    ToUserKind, Token, UserAsk, Version, ViewCmd, ViewMode, WaitArg, When,
};
use serde_json::{Map, Value};

use super::spec::{self, Action, Form};
use super::values as val;
use super::{DecodeEnv, DecodeError, NodeArgText, Tool, UNKNOWN_NODE, fq};
use crate::digest::question_digest;
use crate::tz;

type R<T> = Result<T, DecodeError>;

fn bad(arg: impl Into<String>, why: impl Into<String>) -> DecodeError {
    DecodeError::new(arg, why)
}

/// An object of arguments and where it sits in the call.
#[derive(Clone, Copy)]
struct Obj<'m> {
    map: &'m Map<String, Value>,
    at: &'m str,
}

/// Reads values, records node arguments' raw texts, and runs the shell's lookups.
struct B<'e> {
    env: &'e DecodeEnv<'e>,
    nodes: Vec<NodeArgText>,
    /// `view feed {since}` given as an entry id (1.6).
    since_entry: Option<Eid>,
}

/// A built call: its command, the raw text of its node arguments (R5), and what the writer
/// resolves against its tables before `apply` (1.6: `feed`'s `since` given as an entry id, the
/// node's last seq at that entry, as it does for `wait`'s `since`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Built {
    pub(super) cmd: Cmd,
    pub(super) node_args: Vec<NodeArgText>,
    pub(super) since_entry: Option<Eid>,
}

impl<'m> Obj<'m> {
    fn get(&self, name: &str) -> Option<&'m Value> {
        self.map.get(name).filter(|v| !v.is_null())
    }

    fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    fn is_null(&self, name: &str) -> bool {
        self.map.get(name).is_some_and(Value::is_null)
    }

    fn fq(&self, name: &str) -> String {
        fq(self.at, name)
    }

    fn str(&self, name: &str) -> R<Option<&'m str>> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(bad(self.fq(name), "expected a string")),
        }
    }

    fn strs(&self, name: &str) -> R<Option<Vec<&'m str>>> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    v.as_str()
                        .ok_or_else(|| bad(format!("{}[{i}]", self.fq(name)), "expected a string"))
                })
                .collect::<R<Vec<_>>>()
                .map(Some),
            Some(_) => Err(bad(self.fq(name), "expected a list of strings")),
        }
    }

    fn obj(&self, name: &str) -> R<Option<(&'m Map<String, Value>, String)>> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Object(m)) => Ok(Some((m, self.fq(name)))),
            Some(_) => Err(bad(self.fq(name), "expected an object")),
        }
    }

    fn text(&self, name: &str) -> R<Option<Text>> {
        Ok(self.str(name)?.map(|s| Text(s.to_string())))
    }

    fn rtext(&self, name: &str) -> R<Text> {
        self.text(name)?.ok_or_else(|| missing(&self.fq(name)))
    }

    fn texts(&self, name: &str) -> R<Vec<Text>> {
        Ok(self
            .strs(name)?
            .unwrap_or_default()
            .into_iter()
            .map(|s| Text(s.to_string()))
            .collect())
    }

    fn text_map(&self, name: &str) -> R<Option<BTreeMap<Text, Text>>> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Object(m)) => m
                .iter()
                .map(|(k, v)| match v {
                    Value::String(s) => Ok((Text(k.clone()), Text(s.clone()))),
                    _ => Err(bad(format!("{}.{k}", self.fq(name)), "expected a string")),
                })
                .collect::<R<BTreeMap<_, _>>>()
                .map(Some),
            Some(_) => Err(bad(self.fq(name), "expected an object of strings")),
        }
    }

    fn parse<T>(&self, name: &str, f: impl Fn(&str) -> Result<T, String>) -> R<Option<T>> {
        match self.str(name)? {
            None => Ok(None),
            Some(s) => f(s).map(Some).map_err(|why| bad(self.fq(name), why)),
        }
    }

    fn rparse<T>(&self, name: &str, f: impl Fn(&str) -> Result<T, String>) -> R<T> {
        self.parse(name, f)?.ok_or_else(|| missing(&self.fq(name)))
    }

    fn parse_all<T>(&self, name: &str, f: impl Fn(&str) -> Result<T, String>) -> R<Option<Vec<T>>> {
        match self.strs(name)? {
            None => Ok(None),
            Some(items) => items
                .into_iter()
                .enumerate()
                .map(|(i, s)| f(s).map_err(|why| bad(format!("{}[{i}]", self.fq(name)), why)))
                .collect::<R<Vec<T>>>()
                .map(Some),
        }
    }

    fn u64(&self, name: &str) -> R<Option<u64>> {
        match self.get(name) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| bad(self.fq(name), "expected a non-negative integer")),
        }
    }

    fn u32(&self, name: &str) -> R<Option<u32>> {
        match self.u64(name)? {
            None => Ok(None),
            Some(n) => u32::try_from(n)
                .map(Some)
                .map_err(|_| bad(self.fq(name), "expected an integer from 0 to 4294967295")),
        }
    }

    fn i32(&self, name: &str) -> R<Option<i32>> {
        match self.get(name) {
            None => Ok(None),
            Some(v) => v
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| bad(self.fq(name), "expected a 32-bit integer")),
        }
    }

    fn bool(&self, name: &str) -> R<Option<bool>> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(bad(self.fq(name), "expected true or false")),
        }
    }

    fn eid(&self, name: &str) -> R<Option<forum_core::Eid>> {
        self.parse(name, |s| {
            forum_core::Eid::parse(s).ok_or_else(|| format!("{s:?} is not an entry id such as e57"))
        })
    }

    fn reid(&self, name: &str) -> R<forum_core::Eid> {
        self.eid(name)?.ok_or_else(|| missing(&self.fq(name)))
    }

    fn eids(&self, name: &str) -> R<BTreeSet<forum_core::Eid>> {
        Ok(self
            .parse_all(name, |s| {
                forum_core::Eid::parse(s)
                    .ok_or_else(|| format!("{s:?} is not an entry id such as e57"))
            })?
            .unwrap_or_default()
            .into_iter()
            .collect())
    }

    fn token(&self, name: &str) -> R<Option<Token>> {
        self.parse(name, |s| {
            Token::parse(s).ok_or_else(|| format!("{s:?} is not a token such as 12.3"))
        })
    }

    fn rtoken(&self, name: &str) -> R<Token> {
        self.token(name)?.ok_or_else(|| missing(&self.fq(name)))
    }

    fn label(&self, name: &str) -> R<Option<Label>> {
        self.parse(name, label)
    }

    fn labels(&self, name: &str) -> R<Option<BTreeSet<Label>>> {
        Ok(self
            .parse_all(name, label)?
            .map(|v| v.into_iter().collect()))
    }

    /// `needs`, `needs_add`, or `needs_remove`: cap labels, whose refusal says what needs name and
    /// where a blocker goes (1.10, §20.5, §21): in the lab, agents wrote what the work lacks.
    fn needs(&self, name: &str) -> R<Option<BTreeSet<Label>>> {
        Ok(self.parse_all(name, need)?.map(|v| v.into_iter().collect()))
    }

    fn enum_of(&self, name: &str, values: &[&str]) -> R<Option<usize>> {
        self.parse(name, |s| {
            values
                .iter()
                .position(|v| *v == s)
                .ok_or_else(|| format!("{s:?} is not one of {}", values.join(", ")))
        })
    }

    fn renum_of(&self, name: &str, values: &[&str]) -> R<usize> {
        self.enum_of(name, values)?
            .ok_or_else(|| missing(&self.fq(name)))
    }

    fn dur(&self, name: &str) -> R<Option<forum_core::Dur>> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::String(s)) => val::dur_text(s)
                .map(Some)
                .map_err(|why| bad(self.fq(name), why)),
            Some(v) => v
                .as_u64()
                .map(|ms| Some(forum_core::Dur(ms)))
                .ok_or_else(|| {
                    bad(
                        self.fq(name),
                        "expected a duration such as 15m, or an integer of milliseconds",
                    )
                }),
        }
    }

    fn when(&self, name: &str) -> R<Option<forum_core::When>> {
        self.parse(name, val::when)
    }

    fn key(&self, name: &str) -> R<Option<forum_core::Key>> {
        self.parse(name, val::key)
    }

    /// The `principal` of a human the caller names, refused with its own hint (1.5, R3).
    fn principal(&self, name: &str) -> R<Option<forum_core::PrincipalKey>> {
        self.parse(name, val::principal_arg)
    }
}

fn label(s: &str) -> Result<Label, String> {
    Label::new(s).ok_or_else(|| format!("{s:?} is not a label: [a-z0-9][a-z0-9_.:-]{{0,63}}"))
}

/// A label of `needs` (1.10, §20.5, §21): its refusal names the place for a blocker. In the lab's
/// fifth round (it4 l4 C) a sentence about missing access, refused with the pattern alone, became
/// two slugs and then a release's wait: three calls where one would do.
fn need(s: &str) -> Result<Label, String> {
    Label::new(s)
        .ok_or_else(|| format!("{s:?} is not a label ([a-z0-9][a-z0-9_.:-]{{0,63}}): {NEED_HINT}"))
}

/// What a refused label of `needs` adds (1.10, §21).
pub const NEED_HINT: &str = "needs name caps, such as python or sql; a blocker goes in a \
                             checkpoint's next_step or a release's waits_for";

fn missing(arg: &str) -> DecodeError {
    bad(arg, "missing")
}

impl B<'_> {
    /// A node argument, its raw text recorded (R5).
    fn node(&mut self, o: Obj<'_>, name: &str) -> R<Option<NodeRef>> {
        let Some(raw) = o.str(name)? else {
            return Ok(None);
        };
        self.node_text(&o.fq(name), raw, val::node).map(Some)
    }

    fn rnode(&mut self, o: Obj<'_>, name: &str) -> R<NodeRef> {
        self.node(o, name)?.ok_or_else(|| missing(&o.fq(name)))
    }

    fn node_text<T>(
        &mut self,
        arg: &str,
        raw: &str,
        f: impl Fn(&str) -> Result<T, String>,
    ) -> R<T> {
        self.nodes.push(NodeArgText {
            arg: arg.to_string(),
            raw: raw.to_string(),
        });
        f(raw).map_err(|why| DecodeError {
            raw_node: Some(raw.to_string()),
            ..DecodeError::new(arg, why)
        })
    }

    fn nodes(&mut self, o: Obj<'_>, name: &str) -> R<Option<Vec<NodeRef>>> {
        let Some(items) = o.strs(name)? else {
            return Ok(None);
        };
        let at = o.fq(name);
        items
            .into_iter()
            .enumerate()
            .map(|(i, raw)| self.node_text(&format!("{at}[{i}]"), raw, val::node))
            .collect::<R<Vec<_>>>()
            .map(Some)
    }

    fn place(&mut self, o: Obj<'_>, name: &str) -> R<forum_core::Path> {
        let raw = o.str(name)?.ok_or_else(|| missing(&o.fq(name)))?;
        self.node_text(&o.fq(name), raw, val::place)
    }

    /// The node of an entry, from the shell's table; [`UNKNOWN_NODE`] without it, which the core
    /// refuses with `not_found` in §21's order (review E2-spec-3).
    fn entry_node(&self, o: Obj<'_>, name: &str) -> R<(forum_core::Eid, forum_core::NodeId)> {
        let eid = o.reid(name)?;
        let node = self.env.entries.entry_node(eid).unwrap_or(UNKNOWN_NODE);
        Ok((eid, node))
    }

    fn repeat(&self, m: &Map<String, Value>, at: &str) -> R<RepeatArg> {
        let o = Obj { map: m, at };
        let cron_text = o.str("cron")?.ok_or_else(|| missing(&o.fq("cron")))?;
        let cron = forum_cron::parse_cron(cron_text)
            .map_err(|e| bad(o.fq("cron"), val::cron_error(&e)))?;
        let zone = o.str("tz")?.unwrap_or(tz::DEFAULT_ZONE);
        let now = self.env.now.0;
        let from = now.saturating_sub(self.env.coverage_window.0);
        let to = now.saturating_add(tz::AHEAD_MS);
        let table = tz::table(zone, from, to).map_err(|e| {
            bad(
                o.fq("tz"),
                match e {
                    forum_cron::TzError::UnknownZone => {
                        format!("{zone:?} is not an IANA time zone such as Europe/Berlin")
                    }
                    forum_cron::TzError::TooManyTransitions => format!(
                        "{zone:?} changes its offset more than {} times within 50 years",
                        forum_cron::MAX_TRANSITIONS
                    ),
                },
            )
        })?;
        let run_within = o
            .dur("run_within")?
            .ok_or_else(|| missing(&o.fq("run_within")))?;
        Ok(RepeatArg {
            cron_text: Text(cron_text.to_string()),
            cron,
            tz: table,
            run_within,
        })
    }
}

/// Builds the command of a call whose shape `check_form` accepted, from its masked arguments.
pub(super) fn build(
    t: Tool,
    action: &Action,
    form: &Form,
    args: &Map<String, Value>,
    env: &DecodeEnv<'_>,
) -> Result<Built, DecodeError> {
    let mut b = B {
        env,
        nodes: Vec::new(),
        since_entry: None,
    };
    let o = Obj { map: args, at: "" };
    let cmd = match t {
        Tool::Session => Cmd::Session(session(&mut b, o, action.name)?),
        Tool::View => Cmd::View(view(&mut b, o, action.name)?),
        Tool::Node => Cmd::Node(node(&mut b, o, action.name)?),
        Tool::Hold => Cmd::Hold(hold(&mut b, o, action.name)?),
        Tool::Talk => Cmd::Talk(talk(&mut b, o, action.name, form.name)?),
        Tool::Control => Cmd::Control(control(&mut b, o, action.name)?),
    };
    Ok(Built {
        cmd,
        node_args: b.nodes,
        since_entry: b.since_entry,
    })
}

fn unknown_action(name: &str) -> DecodeError {
    bad("action", format!("{name:?} is not an action"))
}

// ---------------------------------------------------------------- session (§20.3)

/// The `to` of `from_user` and `route` (1.7, §8.2): a node, or `none`; `from_user` reads `node` as
/// `to`, and refuses the two together.
fn route_to(b: &mut B<'_>, o: Obj<'_>, with_node: bool) -> R<Option<RouteTo>> {
    let to = o.str("to")?;
    let node = if with_node { o.str("node")? } else { None };
    let (arg, raw) = match (to, node) {
        (Some(_), Some(_)) => {
            return Err(bad(o.fq("node"), "give the target once, as to"));
        }
        (Some(r), None) => ("to", r),
        (None, Some(r)) => ("node", r),
        (None, None) => return Ok(None),
    };
    if raw == "none" {
        return Ok(Some(RouteTo::Nowhere));
    }
    b.node_text(&o.fq(arg), raw, val::node)
        .map(|n| Some(RouteTo::Node(n)))
}

fn session(b: &mut B<'_>, o: Obj<'_>, action: &str) -> R<SessionCmd> {
    Ok(match action {
        "hello" => SessionCmd::Hello {
            intent: o.text("intent")?,
            caps: o.labels("caps")?,
            horizon: o.u32("horizon")?,
            principal: o.principal("principal")?,
            from_user: match o.obj("from_user")? {
                None => None,
                Some((m, at)) => {
                    let f = Obj { map: m, at: &at };
                    Some(FromUserArg {
                        text: f.rtext("text")?,
                        client: f.rtext("client")?,
                        principal: f.principal("principal")?,
                        answers: f.eid("answers")?,
                        to: route_to(b, f, true)?,
                        clarifies: f.eid("clarifies")?,
                        reason: f.text("reason")?,
                    })
                }
            },
        },
        "status" => SessionCmd::Status {
            text: o.rtext("text")?,
        },
        other => return Err(unknown_action(other)),
    })
}

// ---------------------------------------------------------------- view (§20.4)

fn view(b: &mut B<'_>, o: Obj<'_>, action: &str) -> R<ViewCmd> {
    Ok(match action {
        "node" => {
            let targets = match b.node(o, "path")? {
                Some(n) => vec![n],
                None => b.nodes(o, "paths")?.unwrap_or_default(),
            };
            ViewCmd::Node {
                targets,
                view: o.enum_of("view", spec::VIEW_MODES)?.map(|i| match i {
                    0 => ViewMode::Brief,
                    1 => ViewMode::Full,
                    _ => ViewMode::Review,
                }),
                page: o.u32("page")?,
            }
        }
        "find" => ViewCmd::Find {
            by: match o.parse("key", val::key)? {
                Some(k) => FindBy::Key(k.key),
                None => FindBy::Text(o.rtext("text")?),
            },
        },
        "feed" => ViewCmd::Feed {
            at: b.rnode(o, "at")?,
            since: feed_since(b, o)?,
            types: o
                .parse_all("types", entry_type)?
                .unwrap_or_default()
                .into_iter()
                .collect(),
            limit: o.u32("limit")?,
        },
        "list" => {
            let (status, closed) = list_state(o)?;
            ViewCmd::List {
                under: b.node(o, "under")?,
                kind: o.label("kind")?,
                tags: o.labels("tags")?.unwrap_or_default(),
                status,
                fields: o.text_map("fields")?.unwrap_or_default(),
                flags: o
                    .parse_all("flags", flag)?
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<FlagSet>(),
                closed,
                limit: o.u32("limit")?,
            }
        }
        "history" => ViewCmd::History {
            at: b.rnode(o, "path")?,
            before: o.u64("before")?,
            limit: o.u32("limit")?,
        },
        "requests" => ViewCmd::Requests {
            status: o.enum_of("status", spec::BOARD_STATES)?.map(|i| match i {
                0 => BoardState::Unrouted,
                1 => BoardState::Answered,
                2 => BoardState::ClosedUnanswered,
                3 => BoardState::Decided,
                4 => BoardState::Waiting,
                _ => BoardState::InProgress,
            }),
            requester: o.parse("requester", |s| match s {
                "me" => Ok(RequesterFilter::Mine),
                "all" => Ok(RequesterFilter::All),
                other => val::principal(other).map(RequesterFilter::Principal),
            })?,
        },
        "user_log" => ViewCmd::UserLog {
            since: o.eid("since")?,
            limit: o.u32("limit")?,
        },
        "agreements" => ViewCmd::Agreements {
            under: b.node(o, "under")?,
            limit: o.u32("limit")?,
        },
        "constitution" => ViewCmd::Constitution,
        other => return Err(unknown_action(other)),
    })
}

/// `view feed {since}`: a seq, as an integer or its digits, or an entry id (1.6, §20.4), which
/// the writer turns into the node's last seq at that entry; the command's `since` stays empty
/// until then.
fn feed_since(b: &mut B<'_>, o: Obj<'_>) -> R<Option<Seq>> {
    match o.get("since") {
        None => Ok(None),
        Some(Value::String(s)) => {
            if let Some(e) = Eid::parse(s.trim()) {
                b.since_entry = Some(e);
                return Ok(None);
            }
            val::canonical_decimal(s.trim())
                .map(|n| Some(Seq(n)))
                .ok_or_else(|| {
                    bad(
                        o.fq("since"),
                        format!(
                            "{s:?} is neither a seq nor an entry id: pass the eid of the last \
                             entry you saw, such as e57, or a seq"
                        ),
                    )
                })
        }
        Some(v) => v
            .as_u64()
            .map(|n| Some(Seq(n)))
            .ok_or_else(|| bad(o.fq("since"), "expected a seq, or an entry id such as e57")),
    }
}

/// `view list {status, closed}`: `status: open` and `status: closed` filter the node's state, as
/// `closed: false` and `closed: true` do (1.6, §20.4: agents meant the state, l2, l4); any other
/// status is the free-text status. A `closed` that says the opposite is refused, rather than
/// listing nothing.
fn list_state(o: Obj<'_>) -> R<(Option<Text>, Option<bool>)> {
    let status = o.text("status")?;
    let closed = o.bool("closed")?;
    let state = match status.as_ref().map(|t| t.0.as_str()) {
        Some("open") => Some(false),
        Some("closed") => Some(true),
        _ => None,
    };
    match (state, closed) {
        (None, _) => Ok((status, closed)),
        (Some(a), Some(b)) if a != b => {
            let word = if a { "closed" } else { "open" };
            Err(bad(
                o.fq("status"),
                format!(
                    "status {word} asks for {word} nodes, and closed: {b} for the others: pass \
                     one of them"
                ),
            ))
        }
        (Some(a), _) => Ok((None, Some(a))),
    }
}

fn entry_type(s: &str) -> Result<EntryType, String> {
    const ALL: [EntryType; 16] = [
        EntryType::Note,
        EntryType::Request,
        EntryType::Question,
        EntryType::Answer,
        EntryType::Decision,
        EntryType::Checkpoint,
        EntryType::Said,
        EntryType::Heard,
        EntryType::FromUser,
        EntryType::ToUser,
        EntryType::Approve,
        EntryType::Object,
        EntryType::Resolve,
        EntryType::Retract,
        EntryType::System,
        EntryType::Late,
    ];
    spec::ENTRY_TYPES
        .iter()
        .position(|t| *t == s)
        .and_then(|i| ALL.get(i).copied())
        .ok_or_else(|| {
            format!(
                "{s:?} is not an entry type: {}",
                spec::ENTRY_TYPES.join(", ")
            )
        })
}

fn flag(s: &str) -> Result<Flag, String> {
    spec::FLAGS
        .iter()
        .position(|f| *f == s)
        .and_then(|i| Flag::ALL.get(i).copied())
        .ok_or_else(|| format!("{s:?} is not a flag: {}", spec::FLAGS.join(", ")))
}

// ---------------------------------------------------------------- node (§20.5)

fn node(b: &mut B<'_>, o: Obj<'_>, action: &str) -> R<NodeCmd> {
    Ok(match action {
        "create" => NodeCmd::Create(create(b, o)?),
        "update" => update(b, o)?,
        "link" => NodeCmd::Link {
            at: b.rnode(o, "from")?,
            rel: match o.renum_of("rel", spec::LINK_RELS)? {
                0 => LinkRel::Continues,
                1 => LinkRel::DerivedFrom,
                2 => LinkRel::ConflictsWith,
                3 => LinkRel::Duplicate,
                _ => LinkRel::SupersededBy,
            },
            to: b.rnode(o, "to")?,
        },
        "key" => NodeCmd::Key {
            at: b.rnode(o, "path")?,
            change: match o.key("add")? {
                Some(k) => KeyChange::Add(k),
                None => KeyChange::Remove(
                    o.parse("remove", |s| val::key(s).map(|k| k.key))?
                        .ok_or_else(|| missing("add|remove"))?,
                ),
            },
        },
        "close" => NodeCmd::Close {
            at: b.rnode(o, "path")?,
            outcome: o.rparse("outcome", val::outcome)?,
            summary: o.rtext("summary")?,
            verified: o.text("verified")?,
            // 1.7 (§5): without `settles`, the close settles what a result answered; `[]` none.
            settles: if o.get("settles").is_some() {
                Some(o.eids("settles")?)
            } else {
                None
            },
            learnings: o.text("learnings")?,
            token: o.token("token")?,
            unsent: o.eids("unsent")?,
        },
        "reopen" => NodeCmd::Reopen {
            at: b.rnode(o, "path")?,
            reason: o.rtext("reason")?,
        },
        "move" => NodeCmd::Move {
            at: b.rnode(o, "path")?,
            to: b.place(o, "to")?,
        },
        "merge" => NodeCmd::Merge {
            from: b.rnode(o, "from")?,
            into: b.rnode(o, "into")?,
        },
        "hide" => NodeCmd::Hide {
            target: if o.has("entry") {
                let (eid, node) = b.entry_node(o, "entry")?;
                HideArg::Entry { eid, node }
            } else {
                let (m, at) = o.obj("file")?.ok_or_else(|| missing("entry|file"))?;
                let f = Obj { map: m, at: &at };
                HideArg::File {
                    at: b.rnode(f, "path")?,
                    name: f.rtext("name")?,
                }
            },
            reason: o.rtext("reason")?,
        },
        other => return Err(unknown_action(other)),
    })
}

fn create(b: &mut B<'_>, o: Obj<'_>) -> R<CreateArgs> {
    Ok(CreateArgs {
        path: b.place(o, "path")?,
        keys: o.parse_all("keys", val::key)?.unwrap_or_default(),
        kind: o.label("kind")?,
        title: o.rtext("title")?,
        body: o.text("body")?,
        digest: o.text("digest")?,
        fields: o.text_map("fields")?.unwrap_or_default(),
        tags: o.labels("tags")?.unwrap_or_default(),
        capacity: o.u32("capacity")?,
        needs: o.needs("needs")?,
        est: o.u32("est")?,
        priority: o.i32("priority")?,
        due: o.when("due")?,
        depends_on: b.nodes(o, "depends_on")?.unwrap_or_default(),
        owners: b.nodes(o, "owners")?.unwrap_or_default(),
        repeat: match o.obj("repeat")? {
            None => None,
            Some((m, at)) => Some(b.repeat(m, &at)?),
        },
        agreements: o.text("agreements")?,
        request: match o.obj("request")? {
            None => None,
            Some((m, at)) => {
                let r = Obj { map: m, at: &at };
                Some(RequestArg {
                    text: r.rtext("text")?,
                    origin: r.key("origin")?,
                    where_: r.key("where")?,
                    requester: r.rtext("requester")?,
                })
            }
        },
        take: o.bool("take")?,
    })
}

/// `null` clears a `Change` field; absence keeps it.
fn change<T>(o: Obj<'_>, name: &str, value: Option<T>) -> Change<T> {
    match value {
        Some(v) => Change::Set(v),
        None if o.is_null(name) => Change::Clear,
        None => Change::Keep,
    }
}

/// `null` on a field that `Patch` clears with its empty value: that value (§20.5: a field set to
/// null is cleared, review E2-spec-8); absence keeps the field.
fn cleared<T>(o: Obj<'_>, name: &str, value: Option<T>, empty: impl FnOnce() -> T) -> Option<T> {
    match value {
        Some(v) => Some(v),
        None if o.is_null(name) => Some(empty()),
        None => None,
    }
}

fn update(b: &mut B<'_>, o: Obj<'_>) -> R<NodeCmd> {
    let repeat = match o.obj("repeat")? {
        None => None,
        Some((m, at)) => Some(b.repeat(m, &at)?),
    };
    let empty = || Text(String::new());
    Ok(NodeCmd::Update {
        at: b.rnode(o, "path")?,
        // Left out, the writer fills it with the version last shown to the session (1.7, §20.5);
        // 0 is no node's version, so one left unfilled is stale (O2).
        version: Version(o.u64("version")?.unwrap_or(0)),
        digest_version: o.u64("digest_version")?.map(Version),
        token: o.token("token")?,
        patch: Patch {
            title: o.text("title")?,
            body: cleared(o, "body", o.text("body")?, empty),
            digest: o.text("digest")?,
            fields: cleared(o, "fields", o.text_map("fields")?, BTreeMap::new),
            files: cleared(o, "files", o.text_map("files")?, BTreeMap::new),
            status: cleared(o, "status", o.text("status")?, empty),
            agreements: cleared(o, "agreements", o.text("agreements")?, empty),
            kind: change(o, "kind", o.label("kind")?),
            tags_add: o.labels("tags_add")?.unwrap_or_default(),
            tags_remove: o.labels("tags_remove")?.unwrap_or_default(),
            capacity: o.u32("capacity")?,
            priority: o.i32("priority")?,
            est: change(o, "est", o.u32("est")?),
            needs_add: o.needs("needs_add")?.unwrap_or_default(),
            needs_remove: o.needs("needs_remove")?.unwrap_or_default(),
            due: change(o, "due", o.when("due")?),
            depends_on: cleared(o, "depends_on", b.nodes(o, "depends_on")?, Vec::new),
            owners: cleared(o, "owners", b.nodes(o, "owners")?, Vec::new),
            repeat: change(o, "repeat", repeat),
        },
    })
}

// ---------------------------------------------------------------- hold (§20.6)

fn hold(b: &mut B<'_>, o: Obj<'_>, action: &str) -> R<HoldCmd> {
    Ok(match action {
        "take" => HoldCmd::Take {
            target: b.rnode(o, "target")?,
            lease: o.dur("lease")?,
        },
        "next" => HoldCmd::Next {
            query: match o.obj("query")? {
                None => NextQuery::default(),
                Some((m, at)) => {
                    let q = Obj { map: m, at: &at };
                    NextQuery {
                        under: b.node(q, "under")?,
                        kind: q.label("kind")?,
                        tags: q.labels("tags")?.unwrap_or_default(),
                    }
                }
            },
            lease: o.dur("lease")?,
        },
        "checkpoint" => HoldCmd::Checkpoint {
            token: o.rtoken("token")?,
            done: o.rtext("done")?,
            next_step: o.rtext("next_step")?,
            intent: o.text("intent")?,
            intent_done: o.text("intent_done")?,
            files: o.texts("files")?,
        },
        "renew" => HoldCmd::Renew {
            token: o.rtoken("token")?,
            why: o.rtext("why")?,
        },
        "release" => HoldCmd::Release {
            token: o.rtoken("token")?,
            done: o.rtext("done")?,
            next_step: o.rtext("next_step")?,
            reason: match o.renum_of("reason", spec::RELEASE_REASONS)? {
                0 => ReleaseReason::Handoff,
                1 => ReleaseReason::Blocked,
                _ => ReleaseReason::Cannot,
            },
            waits_for: match o.obj("waits_for")? {
                None => None,
                Some((m, at)) => Some(waits_for(b, Obj { map: m, at: &at })?),
            },
            intent_done: o.text("intent_done")?,
            learnings: o.text("learnings")?,
            unsent: o.eids("unsent")?,
            // 1.12 (§10.3): the requests the release's decision lists.
            settles: o.eids("settles")?,
        },
        "check" => HoldCmd::Check {
            target: b.rnode(o, "target")?,
            token: o.token("token")?,
            intent_done: o.text("intent_done")?,
        },
        other => return Err(unknown_action(other)),
    })
}

fn waits_for(b: &mut B<'_>, w: Obj<'_>) -> R<WaitArg> {
    if w.has("question") {
        // §21's hint for text in `waits_for.question` (1.4, review E2-spec-7).
        return w
            .rparse("question", |s| {
                forum_core::Eid::parse(s).ok_or_else(|| {
                    format!(
                        "{s:?} is not an entry id such as e57: pass the eN of a question you \
                         asked with talk ask, such as e9"
                    )
                })
            })
            .map(WaitArg::Question);
    }
    if w.has("node") {
        return Ok(WaitArg::Node(b.rnode(w, "node")?));
    }
    if w.has("time") {
        return w
            .when("time")?
            .map(WaitArg::Time)
            .ok_or_else(|| missing(&w.fq("time")));
    }
    Ok(WaitArg::External {
        what: w.rtext("external")?,
        recheck_after: w
            .when("recheck_after")?
            .ok_or_else(|| missing(&w.fq("recheck_after")))?,
    })
}

// ---------------------------------------------------------------- talk (§20.7)

fn talk(b: &mut B<'_>, o: Obj<'_>, action: &str, form: &str) -> R<TalkCmd> {
    Ok(match action {
        "from_user" => TalkCmd::FromUser {
            text: o.rtext("text")?,
            client: o.rtext("client")?,
            principal: o.principal("principal")?,
            answers: o.eid("answers")?,
            to: route_to(b, o, true)?,
            clarifies: o.eid("clarifies")?,
            reason: o.text("reason")?,
        },
        "to_user" => TalkCmd::ToUser {
            text: o.rtext("text")?,
            // 1.11 (§21): a refused kind names to_user's kinds and what each means.
            kind: o.rparse("kind", to_user_kind)?,
            // Optional since 1.6 (§8.1): without it, the entry is marked as a paraphrase.
            verbatim: o.bool("verbatim")?.unwrap_or(false),
            principal: o.principal("principal")?,
            re: re(o)?,
            // 1.8 (§8.1): the node whose work the report is about.
            at: b.node(o, "at")?,
        },
        "route" => {
            // 1.7 (§8.2): exactly one of `to`, `answers`, and `clarifies` (the shape check's
            // group); none is missing.
            let entry = o.reid("entry")?;
            let to = route_to(b, o, false)?;
            let answers = o.eid("answers")?;
            let clarifies = o.eid("clarifies")?;
            if to.is_none() && answers.is_none() && clarifies.is_none() {
                return Err(missing("to|answers|clarifies"));
            }
            TalkCmd::Route {
                entry,
                to,
                answers,
                clarifies,
                reason: o.text("reason")?,
            }
        }
        "request" => TalkCmd::Request {
            at: b.rnode(o, "at")?,
            text: o.rtext("text")?,
            origin: o.key("origin")?.ok_or_else(|| missing("origin"))?,
            where_: o.key("where")?,
            requester: o.rtext("requester")?,
        },
        "said" if form == "confirm" => TalkCmd::SaidConfirm {
            said: o.reid("said_id")?,
            confirmed: o.rtext("confirmed")?,
        },
        "said" => TalkCmd::Said {
            at: b.rnode(o, "at")?,
            where_: o.key("where")?,
            text: o.rtext("text")?,
            kind: o.rparse("kind", said_kind)?,
            corrects: o.eid("corrects")?,
            seen: o.u64("seen")?.map(Seq),
        },
        "heard" => TalkCmd::Heard {
            at: b.rnode(o, "at")?,
            where_: o.key("where")?,
            from: o.rtext("from")?,
            text: o.rtext("text")?,
            answers: o.eid("answers")?,
        },
        "ask" => ask(b, o, form)?,
        "answer" => {
            let question = o.reid("question")?;
            // Without the entry, the core refuses the answer in §21's order (review E2-spec-3).
            let (node, asker) = b
                .env
                .entries
                .question(question)
                .unwrap_or((UNKNOWN_NODE, None));
            TalkCmd::Answer {
                question,
                node,
                asker,
                text: o.rtext("text")?,
            }
        }
        "say" => {
            let at = b.rnode(o, "at")?;
            // 1.9 (§13): `holder:` with no path is the holder of `at` (l6 of the lab's fourth
            // round: the bare form was refused, and the note was then sent to nobody).
            let to = o.parse("to", |raw| {
                if raw.trim() == "holder:" {
                    Ok(AddresseeArg::Holder(at.clone()))
                } else {
                    val::addressee(raw)
                }
            })?;
            say(o, at, to)?
        }
        "decide" => TalkCmd::Decide {
            at: b.rnode(o, "at")?,
            text: o.rtext("text")?,
            settles: o.eids("settles")?,
            supersedes: o.eid("supersedes")?,
            basis: basis(b, o)?,
            evidence: o.enum_of("evidence", spec::EVIDENCE)?.map(|i| match i {
                0 => Evidence::Attempt,
                1 => Evidence::Observation,
                2 => Evidence::Inference,
                _ => Evidence::Reported,
            }),
        },
        "approve" => {
            let raw = o.str("on")?.ok_or_else(|| missing("on"))?;
            let (on, version) = b.node_text("on", raw, val::node_at)?;
            TalkCmd::Approve {
                on,
                version,
                checked: o.rtext("checked")?,
            }
        }
        // The argument is `text` since 1.6 (l6); the command keeps its field `note` (D.5.2).
        "object" => TalkCmd::Object {
            on: b.rnode(o, "on")?,
            note: o.rtext("text")?,
        },
        "resolve" => {
            let objection = o.reid("objection")?;
            let note = o.rtext("text")?;
            // 1.10 (§20.7): the argument is `stands`, and `upheld`, its name before, is read as it
            // (`spec::ARG_ALIASES`); the command keeps its field `upheld` (§25, the model).
            let stands = o.bool("stands")?;
            // 1.11 (§11.3): a deferral decides nothing, so it takes no verdict.
            let defer = o.bool("defer")?.unwrap_or(false);
            if defer && stands.is_some() {
                return Err(bad(
                    "defer|stands",
                    "defer: true says you cannot decide yet, and stands is a verdict: give one \
                     of them; to defer, say why in text",
                ));
            }
            TalkCmd::Resolve {
                objection,
                note,
                upheld: stands.unwrap_or(false),
                defer,
            }
        }
        "retract" => TalkCmd::Retract {
            target: if o.has("entry") {
                let (eid, node) = b.entry_node(o, "entry")?;
                RetractArg::Entry { eid, node }
            } else {
                RetractArg::Node(b.rnode(o, "path")?)
            },
            text: o.rtext("text")?,
            reason: o.rtext("reason")?,
        },
        "wait" => TalkCmd::Wait {
            timeout: o.dur("timeout")?.ok_or_else(|| missing("timeout"))?,
            for_: b.node(o, "for")?,
            since: None,
        },
        "who" => TalkCmd::Who {
            at: b.node(o, "at")?,
        },
        other => return Err(unknown_action(other)),
    })
}

/// `to_user {re}`: the eNs of the requests a report answers. A node given there is refused with
/// its raw text in the error, so that §21's hint can name its unsettled requests (1.4, review
/// E2-spec-7). Only a text that is plainly a node counts: `#12`, `/a/b`, or `a/b`, but not a key
/// such as `chat:c1/t1`, whose first step has a scheme, nor a bare word, which may be anything.
fn re(o: Obj<'_>) -> R<BTreeSet<forum_core::Eid>> {
    let mut out = BTreeSet::new();
    for (i, s) in o.strs("re")?.unwrap_or_default().into_iter().enumerate() {
        if let Some(e) = forum_core::Eid::parse(s) {
            out.insert(e);
            continue;
        }
        let plainly = s.starts_with('#')
            || s.starts_with('/')
            || s.split_once('/')
                .is_some_and(|(first, _)| !first.contains(':'));
        let node = plainly.then(|| val::node(s).ok()).flatten();
        let why = match &node {
            Some(_) => format!(
                "{s:?} is a node, not an entry id: re takes the eNs of the requests this report \
                 answers, such as e57; a node's unsettled requests are in its brief (view node)"
            ),
            None => format!("{s:?} is not an entry id such as e57"),
        };
        return Err(DecodeError {
            raw_node: node.map(|_| s.to_string()),
            ..DecodeError::new(format!("re[{i}]"), why)
        });
    }
    Ok(out)
}

/// `talk said {kind}`: the four kinds of §9.2, and `question`, which the core refuses with its
/// own hint (`invalid(SaidKindQuestion)`: questions go through `talk ask`). Another value's
/// refusal says what each kind means (1.9).
fn said_kind(s: &str) -> Result<SaidKind, String> {
    match s {
        "ack" => Ok(SaidKind::Ack),
        "reply" => Ok(SaidKind::Reply),
        "result" => Ok(SaidKind::Result),
        "correction" => Ok(SaidKind::Correction),
        "question" => Ok(SaidKind::Question),
        other => Err(format!(
            "{other:?} is not a kind of said: {}",
            crate::mcp::schema::d_said_kinds!()
        )),
    }
}

/// `talk to_user {kind}`: the three kinds of §8.1. Another value's refusal says what each means,
/// and a said's kind that it is one (1.11, §20.7, §21): in the lab's sixth round (it5 l2, l4), two
/// reports were refused for `reply`, with to_user's kinds and no meaning, and one was resent as a
/// result although it was partial.
fn to_user_kind(s: &str) -> Result<ToUserKind, String> {
    match s {
        "ack" => Ok(ToUserKind::Ack),
        "question" => Ok(ToUserKind::Question),
        "result" => Ok(ToUserKind::Result),
        "reply" | "correction" => Err(format!(
            "{s:?} is a kind of said, not of to_user: {}",
            crate::mcp::schema::d_to_user_kinds!()
        )),
        other => Err(format!(
            "{other:?} is not a kind of to_user: {}",
            crate::mcp::schema::d_to_user_kinds!()
        )),
    }
}

/// `talk say` with its `at` and `to` decoded.
fn say(o: Obj<'_>, at: NodeRef, to: Option<AddresseeArg>) -> R<TalkCmd> {
    Ok(TalkCmd::Say {
        at,
        text: o.rtext("text")?,
        to,
        expects: o.enum_of("expects", spec::EXPECTS)?.map(|i| match i {
            0 => Expects::None,
            1 => Expects::Ack,
            _ => Expects::Reply,
        }),
        reply_to: o.eid("reply_to")?,
        until: o.when("until")?,
    })
}

/// The three forms of `ask` (§20.7, R27): the form was picked by `to`, and `check_form` refused
/// any argument of another form.
fn ask(b: &mut B<'_>, o: Obj<'_>, form: &str) -> R<TalkCmd> {
    let at = b.rnode(o, "at")?;
    let text = o.rtext("text")?;
    let again = o.bool("again")?.unwrap_or(false);
    let to = match form {
        "human" => {
            let options = o.texts("options")?;
            let digest = question_digest(
                &text.0,
                &options.iter().map(|t| t.0.clone()).collect::<Vec<_>>(),
            );
            AskTo::User(UserAsk {
                to: o.rparse("to", val::ask_human)?,
                options,
                recommendation: o.text("recommendation")?,
                default: o.text("default")?,
                due: o.when("due")?,
                digest,
            })
        }
        // 1.9 (§13): `holder:` with no path is the holder of `at`.
        "agent" => AskTo::Agent(o.rparse("to", |raw| {
            if raw.trim() == "holder:" {
                Ok(AgentAddressee::Holder(at.clone()))
            } else {
                val::agent(raw)
            }
        })?),
        _ => AskTo::Person {
            where_: o.key("where")?,
            blocking: o.bool("blocking")?.unwrap_or(false),
        },
    };
    Ok(TalkCmd::Ask {
        at,
        text,
        to,
        again,
    })
}

/// `decide {basis}` (§15, CS-22). Since a bare path is a node reference (1.5, R3), the text of
/// each node reference is kept for R5's hint, as a node argument's is: a `not_found(Node)` then
/// says what was given. A string not written as a reference is something outside the forum,
/// checked now, and `checked_at` is optional, now by default (1.6).
fn basis(b: &mut B<'_>, o: Obj<'_>) -> R<Vec<BasisArg>> {
    let Some(v) = o.get("basis") else {
        return Ok(Vec::new());
    };
    let Value::Array(items) = v else {
        return Err(bad("basis", "expected a list of references"));
    };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let at = format!("basis[{i}]");
        out.push(match item {
            Value::String(s) => {
                let r = val::basis_text(s).map_err(|why| bad(&at, why))?;
                if matches!(r, BasisArg::Node { .. }) {
                    b.nodes.push(NodeArgText {
                        arg: at.clone(),
                        raw: s.clone(),
                    });
                }
                r
            }
            Value::Object(m) => {
                let e = Obj { map: m, at: &at };
                BasisArg::External {
                    what: e.rtext("external")?,
                    checked_at: e
                        .parse("checked_at", val::checked_at)?
                        .unwrap_or(When::In(Dur(0))),
                    recheck: e.when("recheck")?,
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {
                return Err(bad(
                    at,
                    "expected a reference: a string, or {external, checked_at?, recheck?}",
                ));
            }
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- control (§20.8)

fn control(b: &mut B<'_>, o: Obj<'_>, action: &str) -> R<ControlCmd> {
    Ok(match action {
        "pause" => ControlCmd::Pause {
            scope: b.rnode(o, "scope")?,
            reason: o.rtext("reason")?,
        },
        "unpause" => ControlCmd::Unpause {
            scope: b.rnode(o, "scope")?,
        },
        "health" => ControlCmd::Health,
        "constitution" => ControlCmd::Constitution,
        "export" => ControlCmd::Export {
            after: o.u64("after")?,
            limit: o.u32("limit")?,
        },
        other => return Err(unknown_action(other)),
    })
}
