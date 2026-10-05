//! The input schemas of the six tools (design D.11.5, §20.2, 1.5, R20; 1.7), and the texts of the
//! argument descriptions.
//!
//! **Schemas** (1.5; 1.4's per-action `oneOf`, R21, is replaced). The Anthropic API refuses a tool
//! input schema whose top level is a `oneOf`, `anyOf`, or `allOf`, and its clients drop the tool,
//! so each tool's input schema is one object, built from the argument tables of `decode::spec`,
//! which the decoder reads too:
//! - `action` is an enum whose description lists, line by line, each action (and each form of
//!   `talk ask` and `talk said`) with the arguments it takes, `*` marking the required ones and
//!   `a|b` a group of which exactly one is given;
//! - the properties are `session`, `op_id`, and every argument of the tool's actions, each once,
//!   with what it is: the actions that take it are the `action` lines' (1.7: no `actions: …`
//!   prefix); an argument whose shape differs between actions has one schema that takes every
//!   shape (a type array, a union enum, or, where neither says it, an `anyOf` inside its own
//!   property), and a field `update` clears with null has `null` in its type array (1.7);
//! - `required` is `["action", "session"]` (1.7: `session` first came from `hello`'s reply), and
//!   `["action"]` for the `session` tool, whose `hello` opens one; no `additionalProperties:
//!   false`, since one object serves every action.
//!
//! The decoder checks what the schema can no longer say: a missing required argument is `invalid`
//! naming it, and an argument the action does not take is ignored with a warning, or, when its
//! loss would change what is recorded, `invalid` naming it and the actions that do (1.7). Nested
//! objects (`from_user`, `repeat`, `waits_for`, …) stay closed objects, the forms of `waits_for`
//! as an `anyOf`.
//!
//! **The tool list once** (1.7, §20.2: it was 57 KB, read on every model request). No tool has an
//! output schema: `structuredContent` is sent without one, which MCP allows, and neither client of
//! the lab gave the schema to its model; what it said of `next_calls` is in the `session`
//! description. The node, time, and duration rules are said once, in each tool's description, not in
//! each argument's. A test holds the list to [`TOOLS_MAX`] bytes, minified.
//!
//! **Descriptions.** The texts below are macros, so that the argument tables can join them with
//! `concat!` at compile time. The rows of §20.2's argument table are carried as the table states
//! them, without the Markdown backticks and the section and invariant references, which mean
//! nothing to an agent; where a row says what a description says ("That…", "Also that…", "One line
//! per value"), the description says it. A test reads the table from `docs/protocol.md` and checks
//! that every argument it names carries its row's text.

/// The most bytes the minified `tools/list` may take (1.7, §20.2), as `instructions.max` bounds
/// the instructions: a test holds the list to it. 1.8 raised it from 24 KiB for the descriptions
/// that 1.7's cut lost and agents needed (it2).
pub const TOOLS_MAX: usize = 25 * 1024;

/// `session` (1.7: copy it from `hello`'s reply; a call sent beside `hello` cannot have it).
macro_rules! d_session {
    () => {
        "From hello's reply, such as s-4: copy it, never guess; wait for hello first"
    };
}

/// `from_user` of `hello` (1.6, the lab's first round; 1.7: what the `session` description says
/// is not repeated; 1.9: nor a client's instruction files, it3 l1; 1.12: the request's signature
/// names its required `text`, as `hello`'s `end` does).
macro_rules! d_from_user_hello {
    () => {
        "Not your client's instruction files, nor a message in a shared thread, which is a \
         request: node create {keys: [<thread>], request: {origin, requester, text}}"
    };
}

/// What each kind of `said` means (1.9, §9.2): in the lab's fourth round, a progress post
/// recorded as a `reply` answered a request, and the close of unfinished work was offered on it.
/// The refusal of a `said` whose `kind` is missing or not one of them says it too. 1.11 (it5 l1, as
/// it3 l1): an "I'll compare …" post recorded as a `reply` answered its request at once, so `ack`
/// comes first, with the words of such a post.
macro_rules! d_said_kinds {
    () => {
        "ack: 'I'll look into it', on it, or progress so far: answers nothing; reply: the answer to \
         what they asked; result: the outcome of the work; correction: replaces an earlier said"
    };
}

/// What each kind of `to_user` means (1.11, §8.1, §20.7): in the lab's sixth round (it5 l2, l4),
/// two reports were refused for `kind: reply`, a said's kind, whose refusal listed to_user's kinds
/// without what they mean. The refusal of a `to_user` whose `kind` is missing or not one of them
/// says it.
macro_rules! d_to_user_kinds {
    () => {
        "result: the outcome, which answers their request; question: you ask them; ack: a promise, \
         or anything else you want on record"
    };
}

/// `kind` of `to_user` and `said` (1.9: what each kind of `said` means; 1.11: each action's
/// kinds, since the two share one enum, and what to_user's mean, it5 l2, l4).
macro_rules! d_kind_answers {
    () => {
        concat!(
            "to_user's kinds: ",
            d_to_user_kinds!(),
            ". said's kinds: ",
            d_said_kinds!()
        )
    };
}

/// `to` of `from_user` and `route` (1.9: shorter, for the tool list's bound; `node`, read as
/// `to`, needs no word; 1.11: a follow-up on the node you hold leaves `to` out, §8.2). 1.12 (it6
/// l4): words that need no work are more than a thanks: an ack such as "I'll review" became a
/// clarification, and a question answered in the reply a request; quiet in `hello`'s `from_user`,
/// whose `to` the `talk` tool's says. Leaving `to` out is `from_user`'s: a route takes exactly one
/// of `to`, `answers`, and `clarifies` (§8.2).
macro_rules! d_from_user_node {
    () => {
        "A node: new work, a request there; none: no work, such as a thanks, an ack, or a question \
         you answer in your reply; a follow-up to their open request on the node you hold: \
         from_user without to, or clarifies; an answer: answers"
    };
}

/// What `lease`'s description also says.
macro_rules! d_lease {
    () => {
        "At least your longest step between checkpoints; the first lease is always lease.first, 15 \
         min by default"
    };
}

/// `repeat` (1.7: shorter; nobody used it in the lab's two rounds).
macro_rules! d_repeat {
    () => {
        "Each occurrence copies the series when its slot comes: edit the series, not an occurrence"
    };
}

/// What `run_within`'s description also says.
macro_rules! d_run_within {
    () => {
        "At its end, an untouched occurrence closes as missed, and one still open is overdue"
    };
}

/// `since` (1.6: `feed` also takes an entry id, l6).
macro_rules! d_since {
    () => {
        "For feed, the last seq or eid you saw, such as e57; for user_log, the last eid"
    };
}

/// `status` of `view list` (1.6: `open` and `closed` filter the node's state, l2, l4).
macro_rules! d_list_status {
    () => {
        "The node's free-text status; open and closed filter the node's state instead, as closed \
         does"
    };
}

/// `verbatim` of `to_user` (1.6: optional, it0).
macro_rules! d_verbatim {
    () => {
        "True when text is exactly what the human sees; else it is marked as a paraphrase"
    };
}

/// `basis` of `decide` (1.6: any other text is something outside the forum, l1, l2, l6).
macro_rules! d_basis {
    () => {
        "What it rests on: #12@4 or a path with @4, a node at a version; #12 or a path, a node; \
         e57, an entry; any other text, or {external, checked_at?, recheck?}, something outside \
         the forum, checked now"
    };
}

/// `checked_at` of an outside basis reference (1.6: optional, `now` and a date, l2, l6).
macro_rules! d_checked_at {
    () => {
        "When it was checked: now by default, a date such as 2026-10-04 (00:00 UTC), or an RFC \
         3339 time; a time after now is read as now"
    };
}

/// A request's `origin`.
macro_rules! d_origin {
    () => {
        "The message's id in its conversation, written the same way every time, such as \
         chat:<channel>/<thread>/<message>: another form makes a second request"
    };
}

/// `re`.
macro_rules! d_re {
    () => {
        "The eids, such as e57, of requests, user entries, or digest entries"
    };
}

/// `intent` of `checkpoint` (1.8: what it is, and that a post in a thread needs none, it2 l1, l3:
/// agents recorded intents for posts and reads, and asked whether an intent is a plan or a record;
/// 1.9: something others will see, not local edits or tests, it3 l1, l3, l4).
macro_rules! d_intent {
    () => {
        "Before something others will see: a push, PR, deploy, job, or mail; local edits, tests, \
         reads, and thread posts need none. intent_done then says whether it happened. One open \
         intent per node"
    };
}

/// `verified` of `close` (1.8: 1.7 left it undescribed, it2 l1).
macro_rules! d_verified {
    () => {
        "How you checked the result, such as tests, CI, or a link: shown to whoever comes next"
    };
}

/// `needs` of `create`, and `needs_add` of `update` (1.8: agents wrote what the work lacks, and
/// were refused for a label, it2 l5, l6).
macro_rules! d_needs {
    () => {
        "Cap labels such as python or sql, [a-z0-9][a-z0-9_.:-]{0,63}: what a taker must be able to \
         do, not what the work lacks"
    };
}

/// `on` of `object` (1.8: agents objected to a decision's eN, it2 l3, l6). 1.11: shorter, for the
/// tool list's bound, with `defer` of `resolve`.
macro_rules! d_object_on {
    () => {
        "The node, never an entry: for a decision, object on its node, naming it in text"
    };
}

/// `evidence`: one line per value.
macro_rules! d_evidence {
    () => {
        "attempt, you tried it\nobservation, you saw it\ninference, you reasoned to it\nreported, \
         someone told you"
    };
}

/// `options`.
macro_rules! d_options {
    () => {
        "[] for an open question, answered in the human's own words"
    };
}

/// `principal` of `to_user` (1.5, the final usability round: a report to another principal's
/// request).
macro_rules! d_to_user_principal {
    () => {
        "To whom, as a principal key such as mail:alice@corp.example: your own human, or the \
         requester of a request you finished"
    };
}

/// `at` of `to_user` (1.8, §8.1): in the lab's third round, 3 of 10 reports passed it, and it was
/// ignored, so a worker's report was filed for `user`.
macro_rules! d_to_user_at {
    () => {
        "The node it reports on: by default to its requester, naming their requests there"
    };
}

/// `confirmed` of `said` (1.5, the final usability round: no client returned a link). 1.9 (the lab's
/// fourth round): only once it is out; a worker confirmed a reply it never sent.
macro_rules! d_confirmed {
    () => {
        "Once it is out: its link, or, with none, how you sent it"
    };
}

/// `stands` of `resolve` (1.10, §11.3): the verdict, named `upheld` before 1.10. In the lab's fifth
/// round (it4 l5), a reviewer who found a claim true but unproven passed `upheld: true`, which
/// marks the claim refuted: "upheld" read as "the claim is upheld". 1.11: the third answer, `defer`
/// (it5 l5: every reviewer found a review that neither held nor fell), in rule 9's words. 1.12
/// (it6 l5): "false: it holds" asserted a claim that the reviewer denied; `false` says only that
/// the objection fails.
macro_rules! d_stands {
    () => {
        "true: the claim is false (refuted or failed); false: the objection fails; cannot tell yet: \
         defer"
    };
}

/// `defer` of `resolve` (1.11, §11.3): in the lab's sixth round (it5 l5), every reviewer found a
/// review that neither held nor fell, and had no third answer. 1.11 left it quiet, for the tool
/// list's bound; 1.12 (it6 l5: no reviewer used or named it) describes it.
macro_rules! d_defer {
    () => {
        "Cannot decide yet: say in text what evidence would decide it"
    };
}

/// `unsent` of `close` and `release` (1.10, §9.2): in the lab's fifth round (it4 l1), an agent
/// listed a post it sent in the reply after its release, which made it unconfirmed.
macro_rules! d_unsent {
    () => {
        "Drafts you dropped and will never send; a said you post in this reply counts as sent: \
         leave it out"
    };
}

/// `op_id`.
macro_rules! d_op_id {
    () => {
        "A fresh UUID for each write, in any tool; repeat it only to retry that write"
    };
}

pub(crate) use {
    d_basis, d_checked_at, d_confirmed, d_defer, d_evidence, d_from_user_hello, d_from_user_node,
    d_intent, d_kind_answers, d_lease, d_list_status, d_needs, d_object_on, d_op_id, d_options,
    d_origin, d_re, d_repeat, d_run_within, d_said_kinds, d_session, d_since, d_stands,
    d_to_user_at, d_to_user_kinds, d_to_user_principal, d_unsent, d_verbatim, d_verified,
};

// ------------------------------------------------------------------------------------------------
// Input schemas (1.5, §20.2; 1.7)

use serde_json::{Map, Value, json};

use crate::decode::spec::{Action, Arg, BASIS_EXTERNAL_ARGS, COMMON, Form, Kind, ToolSpec};

/// The head of the description of `action`; one line per action, or per form of `talk ask` and
/// `talk said`, follows it.
pub const ACTION_HEAD: &str = "Arguments by action: * required, a|b exactly one of them.";

/// The JSON Schema of a value of `kind`, without its description.
fn kind_schema(kind: Kind) -> Value {
    match kind {
        Kind::Text
        | Kind::Cron
        | Kind::Zone
        | Kind::Node
        | Kind::Place
        | Kind::NodeAt
        | Kind::Key
        | Kind::Principal
        | Kind::Requester
        | Kind::Token
        | Kind::Eid
        | Kind::When
        | Kind::Checked
        | Kind::Label
        | Kind::Addressee
        | Kind::AskHuman
        | Kind::AskAgent
        | Kind::RouteTo
        | Kind::Outcome
        | Kind::SessionId
        | Kind::OpId => json!({"type": "string"}),
        Kind::Nodes | Kind::Keys | Kind::Eids | Kind::Labels | Kind::TextList => {
            json!({"type": "array", "items": {"type": "string"}})
        }
        // The decoder's own bounds of the integers are not stated (1.7: the tool list once): a
        // negative count or one past 32 bits is refused naming the argument.
        Kind::Seq | Kind::U64 | Kind::U32 | Kind::I32 => json!({"type": "integer"}),
        // An integer as `Seq` takes, or an entry id such as `e57` (1.6).
        Kind::SeqOrEid => json!({"type": ["integer", "string"]}),
        Kind::Dur => json!({"type": ["string", "integer"]}),
        Kind::Bool => json!({"type": "boolean"}),
        Kind::Enum(values) => json!({"type": "string", "enum": values}),
        Kind::Enums(values) => {
            json!({"type": "array", "items": {"type": "string", "enum": values}})
        }
        Kind::TextMap => json!({"type": "object", "additionalProperties": {"type": "string"}}),
        Kind::Object(args) => object_schema(args),
        // The variants are told apart by their first argument, and each is closed, so at most
        // one matches: `anyOf` says the same as `oneOf`, in the combinator more clients take.
        Kind::OneOf(variants) => json!({
            "type": "object",
            "anyOf": variants.iter().map(|args| object_schema(args)).collect::<Vec<_>>(),
        }),
        Kind::Basis => json!({
            "type": "array",
            "items": {"anyOf": [{"type": "string"}, object_schema(BASIS_EXTERNAL_ARGS)]},
        }),
    }
}

/// `schema` that also takes `null` (§20.5: `update` clears a field set to null): `null` joins its
/// type, as a type array (1.7).
fn with_null(schema: &mut Value) {
    let Some(m) = schema.as_object_mut() else {
        return;
    };
    let types = match m.remove("type") {
        Some(Value::String(t)) => vec![Value::String(t)],
        Some(Value::Array(ts)) => ts,
        _ => Vec::new(),
    };
    let mut types: Vec<Value> = types.into_iter().filter(|t| t != "null").collect();
    types.push(json!("null"));
    m.insert("type".into(), Value::Array(types));
}

/// The schema of one argument, with its description unless it is quiet (1.7).
fn arg_schema(a: &Arg) -> Value {
    let mut s = kind_schema(a.kind);
    if a.nullable {
        with_null(&mut s);
    }
    if a.described {
        describe(&mut s, a.desc);
    }
    s
}

fn describe(schema: &mut Value, desc: &str) {
    if let (Value::Object(m), false) = (schema, desc.is_empty()) {
        m.insert("description".into(), Value::String(desc.to_string()));
    }
}

fn properties(args: &[Arg]) -> Map<String, Value> {
    args.iter()
        .map(|a| (a.name.to_string(), arg_schema(a)))
        .collect()
}

fn required(args: &[Arg]) -> Vec<&'static str> {
    args.iter().filter(|a| a.required).map(|a| a.name).collect()
}

/// A closed object of `args`: a nested argument (`from_user`, `repeat`, `query`, …), whose
/// shape is the same for every action that takes it.
fn object_schema(args: &[Arg]) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!("object"));
    m.insert("properties".into(), Value::Object(properties(args)));
    let required = required(args);
    if !required.is_empty() {
        m.insert("required".into(), json!(required));
    }
    m.insert("additionalProperties".into(), json!(false));
    Value::Object(m)
}

/// The description of a tool's `action`: [`ACTION_HEAD`], then one line per action, or per form
/// of an action with several, `<action>: <arguments>` (1.5, §20.2), such as `release: token*,
/// done*, next_step*, reason*, waits_for, learnings, unsent`.
#[must_use]
pub fn action_description(tool: &ToolSpec) -> String {
    let mut out = String::from(ACTION_HEAD);
    for (action, form) in tool.forms() {
        out.push('\n');
        out.push_str(&action.form_title(form));
        out.push_str(": ");
        out.push_str(&form.arg_list());
    }
    out
}

type Taker = (&'static Action, &'static Form, &'static Arg);

/// The description of one argument of a tool: what it is, once when every action that takes it
/// says the same, else line by line, `<forms>: <description>` (1.5, §20.2); without the `actions:
/// …` prefix of 1.5, which the lines of `action` give, and without the descriptions of quiet
/// arguments (1.7).
fn arg_description(takers: &[Taker]) -> String {
    let mut descs: Vec<(&str, Vec<(&'static Action, &'static Form)>)> = Vec::new();
    for (a, f, x) in takers {
        if x.desc.is_empty() || !x.described {
            continue;
        }
        match descs.iter_mut().find(|(d, _)| *d == x.desc) {
            Some((_, v)) => v.push((*a, *f)),
            None => descs.push((x.desc, vec![(*a, *f)])),
        }
    }
    match descs.as_slice() {
        [] => String::new(),
        [(d, fs)] if fs.len() == takers.len() => (*d).to_string(),
        _ => descs
            .iter()
            .map(|(d, fs)| format!("{}: {d}", crate::decode::spec::name_forms(fs).join(", ")))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// The types a schema that says no more than its type takes; None for one that says more.
fn plain_types(s: &Value) -> Option<Vec<Value>> {
    let m = s.as_object().filter(|m| m.len() == 1)?;
    match m.get("type")? {
        Value::Array(ts) => Some(ts.clone()),
        t => Some(vec![t.clone()]),
    }
}

/// One schema that takes every shape in `alts` (1.7): the one shape; string enums joined into one
/// enum (`kind` of `to_user` and `said`); plain types joined into a type array; else an `anyOf`
/// of them (`status` of `view list` and `view requests`).
fn merge(alts: Vec<Value>) -> Value {
    if let [one] = alts.as_slice() {
        return one.clone();
    }
    let union = |lists: Vec<Vec<Value>>| {
        let mut out: Vec<Value> = Vec::new();
        for v in lists.into_iter().flatten() {
            if !out.contains(&v) {
                out.push(v);
            }
        }
        out
    };
    let enums: Option<Vec<Vec<Value>>> = alts
        .iter()
        .map(|s| {
            let m = s.as_object().filter(|m| m.len() == 2)?;
            (m.get("type")? == "string").then_some(())?;
            m.get("enum")?.as_array().cloned()
        })
        .collect();
    if let Some(values) = enums {
        return json!({"type": "string", "enum": union(values)});
    }
    let types: Option<Vec<Vec<Value>>> = alts.iter().map(plain_types).collect();
    if let Some(types) = types {
        return json!({"type": union(types)});
    }
    json!({"anyOf": alts})
}

/// The property of one argument of a tool: one schema that takes every shape its forms give it
/// (1.7), with its description.
fn arg_property(takers: &[Taker]) -> Value {
    let mut alts: Vec<Value> = Vec::new();
    for (_, _, a) in takers {
        let s = kind_schema(a.kind);
        if !alts.contains(&s) {
            alts.push(s);
        }
    }
    let mut s = merge(alts);
    if takers.iter().any(|(_, _, a)| a.nullable) {
        if s.get("anyOf").is_some() {
            if let Some(Value::Array(xs)) = s.get_mut("anyOf") {
                xs.push(json!({"type": "null"}));
            }
        } else {
            with_null(&mut s);
        }
    }
    describe(&mut s, &arg_description(takers));
    s
}

/// A tool's input schema: one object, with no combinator at the top (1.5, §20.2), that requires
/// `action`, and `session` but in the `session` tool (1.7).
#[must_use]
pub fn input_schema(tool: &ToolSpec) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert(
        "action".into(),
        json!({
            "type": "string",
            "enum": tool.action_names(),
            "description": action_description(tool),
        }),
    );
    // `op_id` is described once, in the `session` tool (1.7: the tool list once).
    for c in &COMMON {
        let mut p = arg_schema(c);
        if c.name == "op_id"
            && tool.name != "session"
            && let Value::Object(m) = &mut p
        {
            m.remove("description");
        }
        props.insert(c.name.into(), p);
    }
    for name in tool.arg_names() {
        props.insert(name.into(), arg_property(&tool.takers(name)));
    }
    let mut s = Map::new();
    s.insert("type".into(), json!("object"));
    s.insert("properties".into(), Value::Object(props));
    s.insert("required".into(), json!(required_of(tool)));
    s
}

/// What a tool's input schema requires (1.7, §20.2): `action`, and `session` in every tool but
/// `session`, whose `hello` opens one. The decoder still reads a missing session from the binding
/// of the MCP session (§3, §20.1).
#[must_use]
pub fn required_of(tool: &ToolSpec) -> Vec<&'static str> {
    if tool.name == "session" {
        vec!["action"]
    } else {
        vec!["action", "session"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::spec::{NODE_ALIASES, TOOLS, about, directing, name_forms};
    use crate::decode::{DecodeEnv, EntryLookup, Prepared, ShellAnswer, prepare};
    use forum_core::{Dur, Eid, NodeId, SessionId, Time};
    use regex::Regex;
    use std::collections::{BTreeMap, BTreeSet};

    const PROTOCOL: &str = include_str!("../../../../docs/protocol.md");

    // ------------------------------------------------------------ a JSON Schema subset

    fn type_ok(t: &Value, v: &Value) -> bool {
        match t {
            Value::Array(ts) => ts.iter().any(|t| type_ok(t, v)),
            Value::String(t) => match t.as_str() {
                "object" => v.is_object(),
                "array" => v.is_array(),
                "string" => v.is_string(),
                "integer" => v.is_i64() || v.is_u64(),
                "number" => v.is_number(),
                "boolean" => v.is_boolean(),
                "null" => v.is_null(),
                other => panic!("type {other}"),
            },
            other => panic!("type {other}"),
        }
    }

    /// Validates `v` against `schema`, for the keywords the generator emits; any other keyword
    /// fails the test, so the subset cannot silently grow.
    fn valid(schema: &Value, v: &Value) -> bool {
        let Value::Object(s) = schema else {
            panic!("a schema is an object: {schema}");
        };
        for k in s.keys() {
            assert!(
                [
                    "type",
                    "enum",
                    "properties",
                    "required",
                    "additionalProperties",
                    "items",
                    "anyOf",
                    "minimum",
                    "maximum",
                    "description",
                ]
                .contains(&k.as_str()),
                "keyword {k}"
            );
        }
        let get = |k: &str| s.get(k);
        let ty = get("type").is_none_or(|t| type_ok(t, v));
        let listed = match get("enum") {
            Some(Value::Array(e)) => e.contains(v),
            _ => true,
        };
        let int = v.as_i64();
        let above = match (get("minimum").and_then(Value::as_i64), int) {
            (Some(min), Some(n)) => n >= min,
            _ => true,
        };
        let below = match (get("maximum").and_then(Value::as_i64), int) {
            (Some(max), Some(n)) => n <= max,
            (Some(_), None) => v.as_u64().is_none(),
            _ => true,
        };
        let object = match v {
            Value::Object(o) => object_ok(s, o),
            _ => true,
        };
        let items = match (get("items"), v) {
            (Some(i), Value::Array(xs)) => xs.iter().all(|x| valid(i, x)),
            _ => true,
        };
        let any = match get("anyOf") {
            Some(Value::Array(b)) => b.iter().any(|b| valid(b, v)),
            _ => true,
        };
        ty && listed && above && below && object && items && any
    }

    /// `properties`, `required`, and `additionalProperties` of an object value.
    fn object_ok(s: &Map<String, Value>, o: &Map<String, Value>) -> bool {
        let props = s.get("properties").and_then(Value::as_object);
        let declared = |k: &String| props.is_some_and(|p| p.contains_key(k));
        let typed = o
            .iter()
            .all(|(k, x)| props.and_then(|p| p.get(k)).is_none_or(|ps| valid(ps, x)));
        let present = match s.get("required") {
            Some(Value::Array(req)) => req.iter().all(|r| o.contains_key(r.as_str().unwrap())),
            _ => true,
        };
        let closed = match s.get("additionalProperties") {
            Some(Value::Bool(false)) => o.keys().all(declared),
            Some(extra @ Value::Object(_)) => o
                .iter()
                .filter(|(k, _)| !declared(k))
                .all(|(_, x)| valid(extra, x)),
            _ => true,
        };
        typed && present && closed
    }

    // ------------------------------------------------------------ samples

    /// A valid value of each kind.
    fn sample(kind: Kind) -> Value {
        match kind {
            Kind::Text => json!("t"),
            Kind::Cron => json!("0 7 * * *"),
            Kind::Zone => json!("Europe/Berlin"),
            Kind::Node | Kind::RouteTo => json!("/a/b"),
            Kind::Place => json!("/a/c"),
            Kind::NodeAt => json!("#12@4"),
            Kind::Nodes => json!(["/a", "#3"]),
            Kind::Key => json!("chat:c1/t1"),
            Kind::Keys => json!(["issue:p#1"]),
            Kind::Principal => json!("mail:bob@x.example"),
            Kind::Requester => json!("me"),
            Kind::Token => json!("12.3"),
            Kind::Eid => json!("e5"),
            Kind::Eids => json!(["e5", "e6"]),
            Kind::Seq | Kind::U64 => json!(4),
            Kind::SeqOrEid => json!("e5"),
            Kind::Checked => json!("now"),
            Kind::U32 => json!(2),
            Kind::I32 => json!(-1),
            Kind::Dur => json!("15m"),
            Kind::When => json!("in 10m"),
            Kind::Label => json!("rust"),
            Kind::Labels => json!(["rust"]),
            Kind::Bool => json!(true),
            Kind::Enum(values) => json!(values[0]),
            Kind::Enums(values) => json!([values[0]]),
            Kind::TextMap => json!({"k": "v"}),
            Kind::TextList => json!(["a"]),
            Kind::Object(args) => Value::Object(
                args.iter()
                    .filter(|a| a.required)
                    .map(|a| (a.name.to_string(), sample(a.kind)))
                    .collect(),
            ),
            Kind::OneOf(variants) => sample(Kind::Object(variants[0])),
            Kind::Basis => {
                json!(["#12@4", "e57", {"external": "x", "checked_at": "2026-10-01T07:00:00Z"}])
            }
            Kind::Addressee | Kind::AskAgent => json!("session:s-4"),
            Kind::AskHuman => json!("user"),
            Kind::Outcome => json!("done"),
            Kind::SessionId => json!("s-4"),
            Kind::OpId => json!("op-1"),
        }
    }

    struct AllEntries;

    impl EntryLookup for AllEntries {
        fn entry_node(&self, _: Eid) -> Option<NodeId> {
            Some(NodeId(40))
        }
        fn question(&self, _: Eid) -> Option<(NodeId, Option<SessionId>)> {
            Some((NodeId(41), None))
        }
    }

    fn prepared(tool: &ToolSpec, call: &Map<String, Value>) -> Result<Prepared, ShellAnswer> {
        let env = DecodeEnv {
            now: Time(1_790_812_800_000),
            text_max: 65_536,
            coverage_window: Dur(28 * 86_400_000),
            entries: &AllEntries,
        };
        prepare(tool.name, call.clone(), &env)
    }

    fn decodes(tool: &ToolSpec, call: &Map<String, Value>) -> Result<(), ShellAnswer> {
        prepared(tool, call).map(|_| ())
    }

    /// The minimal call of a form: `action`, its required arguments, the first of each `one_of`
    /// group, and `session` where the schema requires it (1.7).
    fn minimal(tool: &ToolSpec, action: &Action, form: &Form) -> Map<String, Value> {
        let mut call = Map::new();
        call.insert("action".into(), json!(action.name));
        if tool.name != "session" {
            call.insert("session".into(), json!("s-4"));
        }
        for a in form.args.iter().filter(|a| a.required) {
            call.insert(a.name.into(), sample(a.kind));
        }
        for group in form.one_of {
            let a = form.arg(group[0]).unwrap();
            call.insert(a.name.into(), sample(a.kind));
        }
        call
    }

    fn in_group(form: &Form, name: &str) -> bool {
        form.one_of.iter().any(|g| g.contains(&name))
    }

    // ------------------------------------------------------------ the schema, read back

    /// One form's arguments, as a schema's descriptions state them or as the table has them.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Shape {
        args: BTreeSet<String>,
        required: BTreeSet<String>,
        groups: BTreeSet<BTreeSet<String>>,
    }

    /// The table's shape of a form.
    fn spec_shape(form: &Form) -> Shape {
        Shape {
            args: form.args.iter().map(|a| a.name.to_string()).collect(),
            required: form
                .args
                .iter()
                .filter(|a| a.required)
                .map(|a| a.name.to_string())
                .collect(),
            groups: form
                .one_of
                .iter()
                .map(|g| g.iter().map(|n| (*n).to_string()).collect())
                .collect(),
        }
    }

    type FormKey = (String, String);

    /// `ask (human: to …)` → (`ask`, `human`); `release` → (`release`, `release`), the name of an
    /// action's only form.
    fn title_key(title: &str) -> FormKey {
        match title.split_once(" (") {
            Some((a, rest)) => {
                let form = rest.split([':', ')']).next().unwrap_or_default();
                (a.to_string(), form.to_string())
            }
            None => (title.to_string(), title.to_string()),
        }
    }

    /// The shape of every form as the description of `action` states it.
    fn shapes_of_action(schema: &Map<String, Value>) -> BTreeMap<FormKey, Shape> {
        let d = schema["properties"]["action"]["description"]
            .as_str()
            .unwrap();
        let mut lines = d.lines();
        assert_eq!(lines.next(), Some(ACTION_HEAD));
        lines
            .map(|l| {
                let (title, args) = l.rsplit_once(": ").unwrap_or_else(|| panic!("{l}"));
                let mut shape = Shape::default();
                if args != "no arguments" {
                    for a in args.split(", ") {
                        if let Some(name) = a.strip_suffix('*') {
                            shape.args.insert(name.into());
                            shape.required.insert(name.into());
                        } else if a.contains('|') {
                            let g: BTreeSet<String> = a.split('|').map(String::from).collect();
                            shape.args.extend(g.iter().cloned());
                            shape.groups.insert(g);
                        } else {
                            shape.args.insert(a.into());
                        }
                    }
                }
                (title_key(title), shape)
            })
            .collect()
    }

    /// 1.5 (§20.2, D.11.5): the description of `action` states every form's argument set, and it
    /// agrees with the table the decoder reads (1.7: the arguments' own descriptions no longer
    /// repeat it). For every action and form, the call built from its required arguments
    /// validates against the schema and decodes; so does each optional argument and each
    /// alternative of a group; an argument the form does not take is, by 1.7's rules, read as the
    /// node the call is about when it is one of its names, refused naming it and the forms that
    /// take it when it is directing or no action takes it, and otherwise ignored with a warning;
    /// and a missing required argument is refused naming it. So the schema and the decoder cannot
    /// drift apart.
    #[test]
    fn the_schema_and_the_decoder_agree_on_every_action() {
        let mut forms = 0;
        for tool in &TOOLS {
            let map = input_schema(tool);
            let by_action = shapes_of_action(&map);
            assert_eq!(by_action.len(), tool.forms().count(), "{}", tool.name);
            let schema = Value::Object(map.clone());
            let every_arg: Vec<String> = map["properties"]
                .as_object()
                .unwrap()
                .keys()
                .filter(|k| !["action", "session", "op_id"].contains(&k.as_str()))
                .cloned()
                .chain(["bogus".to_string()])
                .collect();
            for (action, form) in tool.forms() {
                forms += 1;
                let key = (action.name.to_string(), form.name.to_string());
                let here = format!("{} {}", tool.name, action.form_title(form));
                let shape = &by_action[&key];
                assert_eq!(
                    shape,
                    &spec_shape(form),
                    "{here}: the description of action"
                );
                let base = minimal(tool, action, form);
                assert!(
                    valid(&schema, &Value::Object(base.clone())),
                    "{here}: {base:?}"
                );
                decodes(tool, &base).unwrap_or_else(|e| panic!("{here}: {}", e.render()));
                let mut with_common = base.clone();
                with_common.insert("session".into(), json!("s-4"));
                with_common.insert("op_id".into(), json!("op-1"));
                assert!(
                    valid(&schema, &Value::Object(with_common.clone())),
                    "{here}"
                );
                decodes(tool, &with_common).unwrap();
                for a in form.args.iter().filter(|a| !a.required) {
                    let mut call = base.clone();
                    if in_group(form, a.name) {
                        for g in form.one_of.iter().filter(|g| g.contains(&a.name)) {
                            for other in g.iter() {
                                call.remove(*other);
                            }
                        }
                    }
                    call.insert(a.name.into(), sample(a.kind));
                    assert!(
                        valid(&schema, &Value::Object(call.clone())),
                        "{here} + {}",
                        a.name
                    );
                    decodes(tool, &call)
                        .unwrap_or_else(|e| panic!("{here} + {}: {}", a.name, e.render()));
                    if a.nullable {
                        call.insert(a.name.into(), Value::Null);
                        assert!(valid(&schema, &Value::Object(call.clone())), "{here}");
                        decodes(tool, &call).unwrap();
                    }
                }
                for foreign in every_arg.iter().filter(|n| !shape.args.contains(*n)) {
                    if (action.name == "ask" && foreign == "to")
                        || (action.name == "said" && foreign == "said_id")
                    {
                        // `to` picks the form of `ask`, and `said_id` that of `said`: adding
                        // one makes the call another form, whose own check then refuses.
                        continue;
                    }
                    let mut call = base.clone();
                    call.insert(foreign.clone(), json!("x"));
                    // The call's tool first, then the others, as a refusal names them.
                    let takers: Vec<String> = std::iter::once(tool)
                        .chain(TOOLS.iter().filter(|o| o.name != tool.name))
                        .filter_map(|o| {
                            let fs: Vec<(&'static Action, &'static Form)> = o
                                .takers(foreign)
                                .into_iter()
                                .filter(|(_, f, _)| !std::ptr::eq(*f, form))
                                .map(|(a, f, _)| (a, f))
                                .collect();
                            (!fs.is_empty())
                                .then(|| format!("{} {}", o.name, name_forms(&fs).join(", ")))
                        })
                        .collect();
                    let documented = about(tool.name, action.name, form);
                    // 1.7: another name of the node the call is about is that node.
                    if let Some(doc) =
                        documented.filter(|_| NODE_ALIASES.contains(&foreign.as_str()))
                    {
                        let mut own = base.clone();
                        let same = own.insert(doc.into(), json!("x")).is_none();
                        match (prepared(tool, &call), prepared(tool, &own)) {
                            (Err(ShellAnswer::Invalid(e)), _) if !same => {
                                assert_eq!(e.arg, format!("{doc}|{foreign}"), "{here}");
                                assert!(e.why.starts_with("two nodes given"), "{}", e.why);
                            }
                            (Ok(a), Ok(b)) => assert_eq!(a.cmd, b.cmd, "{here} + {foreign}"),
                            (Err(ShellAnswer::Invalid(a)), Err(ShellAnswer::Invalid(b))) => {
                                assert_eq!(a.arg, b.arg, "{here} + {foreign}");
                            }
                            other => panic!("{here} + {foreign}: {other:?}"),
                        }
                        continue;
                    }
                    let refused = takers.is_empty() || directing(tool.name, action.name, foreign);
                    match prepared(tool, &call) {
                        Err(ShellAnswer::Invalid(e)) if refused => {
                            assert_eq!(&e.arg, foreign, "{here}");
                            let named = if takers.is_empty() {
                                format!("; no action takes {foreign}")
                            } else {
                                format!("; {foreign} is an argument of {}", takers.join(" and "))
                            };
                            assert!(
                                e.why.contains(&named),
                                "{here} + {foreign}: {} lacks {named}",
                                e.why
                            );
                        }
                        // 1.7 (§21): ignored, with the actions that take it.
                        Ok(p) if !refused => {
                            assert_eq!(p.ignored.len(), 1, "{here} + {foreign}");
                            assert_eq!(&p.ignored[0].name, foreign);
                            assert_eq!(p.ignored[0].takers, takers.join(" and "));
                            let detail = p.ignored_detail().unwrap();
                            assert!(detail.contains(&format!("{foreign} (")), "{detail}");
                            assert_eq!(p.cmd, prepared(tool, &base).unwrap().cmd, "{here}");
                        }
                        other => panic!("{here} + {foreign} (refused: {refused}): {other:?}"),
                    }
                }
                // A missing required argument: the decoder names it.
                for a in form.args.iter().filter(|a| a.required) {
                    let mut call = base.clone();
                    call.remove(a.name);
                    if (action.name == "ask" && a.name == "to")
                        || (action.name == "said" && a.name == "said_id")
                    {
                        continue; // without it, the call is another form
                    }
                    match decodes(tool, &call) {
                        Err(ShellAnswer::Invalid(e)) => {
                            assert_eq!(e.arg, a.name, "{here}");
                            assert!(e.why.starts_with("missing"), "{here}: {}", e.why);
                        }
                        other => panic!("{here} - {}: {other:?}", a.name),
                    }
                }
                // A group of which exactly one is given: none, or both, is refused.
                for group in form.one_of {
                    let mut none = base.clone();
                    let mut both = base.clone();
                    for n in group.iter() {
                        none.remove(*n);
                        both.insert((*n).into(), sample(form.arg(n).unwrap().kind));
                    }
                    for call in [none, both] {
                        match decodes(tool, &call) {
                            Err(ShellAnswer::Invalid(e)) => {
                                assert_eq!(e.arg, group.join("|"), "{here}");
                            }
                            other => panic!("{here} {group:?}: {other:?}"),
                        }
                    }
                }
            }
        }
        assert_eq!(
            forms, 50,
            "45 single-form actions (view agreements among them, 1.4), ask's three forms, and \
             said's two"
        );
    }

    /// Texts that panicked a value parser (review E2-bugs-1: a multi-byte last character), and
    /// others of their kind: empty, a lone sigil, a control character, a full-width digit.
    const NASTY: [&str; 21] = [
        "é",
        "1é",
        "Ω",
        "１２",
        "in 1é",
        "in é",
        "in ",
        "",
        " ",
        "#",
        "#é",
        "@",
        "/",
        "é@1",
        "e1é",
        "\u{0}",
        "12.é",
        "holders:é/**",
        "principal:é",
        "session:é",
        "entry:é",
    ];

    /// Values of `kind` that carry `s` wherever a string goes, nested objects included.
    fn with_text(kind: Kind, s: &str) -> Vec<Value> {
        let object = |args: &'static [Arg]| -> Vec<Value> {
            let mut out = Vec::new();
            for a in args {
                for v in with_text(a.kind, s) {
                    let mut o = match sample(Kind::Object(args)) {
                        Value::Object(o) => o,
                        _ => Map::new(),
                    };
                    o.insert(a.name.into(), v);
                    out.push(Value::Object(o));
                }
            }
            out
        };
        match kind {
            Kind::Nodes
            | Kind::Keys
            | Kind::Eids
            | Kind::Labels
            | Kind::Enums(_)
            | Kind::TextList => vec![json!([s])],
            Kind::TextMap => vec![json!({ s: s })],
            Kind::Object(args) => object(args),
            Kind::OneOf(variants) => variants.iter().flat_map(|v| object(v)).collect(),
            Kind::Basis => vec![
                json!([s]),
                json!([{"external": s, "checked_at": s, "recheck": s}]),
            ],
            Kind::Seq | Kind::U64 | Kind::U32 | Kind::I32 | Kind::Bool => Vec::new(),
            _ => vec![json!(s)],
        }
    }

    /// The decoder is total: every string argument of every form, nested ones included, set to
    /// any of [`NASTY`], decodes or is refused, and never panics. A panic killed the request's
    /// task, so the client got no answer at all (review E2-bugs-1).
    #[test]
    fn no_text_in_any_argument_panics_the_decoder() {
        let mut calls = 0;
        for tool in &TOOLS {
            for action in tool.actions {
                for form in action.forms {
                    let base = minimal(tool, action, form);
                    for s in NASTY {
                        for name in ["session", "op_id"] {
                            let mut call = base.clone();
                            call.insert(name.into(), json!(s));
                            let _ = decodes(tool, &call);
                            calls += 1;
                        }
                        for a in form.args {
                            for v in with_text(a.kind, s) {
                                let mut call = base.clone();
                                for g in form.one_of.iter().filter(|g| g.contains(&a.name)) {
                                    for other in g.iter() {
                                        call.remove(*other);
                                    }
                                }
                                call.insert(a.name.into(), v);
                                let _ = decodes(tool, &call);
                                calls += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(calls > 5_000, "{calls}");
    }

    /// 1.5 (§20.2): no schema has a combinator at its top level, which the Anthropic API refuses;
    /// each is one object, with no `additionalProperties: false`, whose properties are `action`,
    /// `session`, `op_id`, and every argument of the tool's actions. 1.7: it requires `action`
    /// and `session`, but only `action` in the `session` tool.
    #[test]
    fn no_schema_has_a_combinator_at_its_top_level() {
        for tool in &TOOLS {
            let s = input_schema(tool);
            for k in [
                "oneOf", "anyOf", "allOf", "not", "if", "then", "else", "$ref",
            ] {
                assert!(s.get(k).is_none(), "{} has {k} at its top level", tool.name);
            }
            let keys: BTreeSet<&str> = s.keys().map(String::as_str).collect();
            assert_eq!(
                keys,
                BTreeSet::from(["properties", "required", "type"]),
                "{}",
                tool.name
            );
            assert_eq!(s["type"], json!("object"));
            let required = if tool.name == "session" {
                json!(["action"])
            } else {
                json!(["action", "session"])
            };
            assert_eq!(s["required"], required, "{}", tool.name);
            let props: BTreeSet<&str> = s["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let mut want: BTreeSet<&str> = tool.arg_names().into_iter().collect();
            want.extend(["action", "session", "op_id"]);
            assert_eq!(props, want, "{}", tool.name);
            assert_eq!(
                s["properties"]["action"]["enum"],
                json!(tool.action_names())
            );
            // Every property has a type or alternatives, and a description unless every action
            // that takes it leaves it out (1.7: quiet), or it is `op_id` outside `session`.
            for (name, p) in s["properties"].as_object().unwrap() {
                assert!(
                    p.get("type").is_some() || p.get("anyOf").is_some(),
                    "{} {name}",
                    tool.name
                );
                let quiet = tool.takers(name).iter().all(|(_, _, a)| !a.described)
                    && !["action", "session", "op_id"].contains(&name.as_str());
                let described = p["description"].as_str().is_some_and(|d| !d.is_empty());
                let expected = !quiet && (name != "op_id" || tool.name == "session");
                assert_eq!(described, expected, "{} {name}", tool.name);
            }
        }
    }

    /// What the schemas say, as an agent reads them: D.11.5's examples, the forms of `talk ask`,
    /// and the shapes that differ between actions, each one schema that takes them all (1.7).
    #[test]
    fn each_tool_is_one_object_that_names_each_actions_arguments() {
        let desc = |tool: usize, name: &str| -> String {
            input_schema(&TOOLS[tool])["properties"][name]["description"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let hold = desc(3, "action");
        assert!(hold.starts_with(ACTION_HEAD), "{hold}");
        assert!(
            hold.contains(
                "\nrelease: token*, done*, next_step*, reason*, waits_for, intent_done, settles, \
                 learnings, unsent"
            ),
            "{hold}"
        );
        assert!(hold.contains("\nnext: query, lease\n"), "{hold}");
        let talk = desc(4, "action");
        for line in [
            "\ndecide: at*, text*, settles, supersedes, basis, evidence\n",
            "\nask (human: to user or principal:<key>): at*, text*, to*, options*, recommendation, default, due, again\n",
            "\nask (person: no to, a person outside the swarm): at*, text*, where, again, blocking\n",
            "\nask (agent: to session:, holder:, or holders:): at*, text*, to*, again, blocking\n",
            "\nsaid (record: without said_id, before you send): at*, where, text*, kind*, corrects, seen\n",
            "\nsaid (confirm: with said_id, after you sent it): said_id*, confirmed*\n",
            "\nretract: entry|path, text*, reason*\n",
        ] {
            assert!(talk.contains(line), "{line} in {talk}");
        }
        assert!(
            desc(5, "action").ends_with(
                "\nhealth: no arguments\nconstitution: no arguments\nexport: after, limit"
            )
        );
        // 1.7: an argument says what it is, once; the lines of `action` say which actions take
        // it, and the tool's description the node, time, and duration rules.
        assert_eq!(
            desc(4, "blocking"),
            "Whether the work cannot go on without the answer"
        );
        assert_eq!(desc(4, "options"), d_options!());
        // The `to` of `ask` is quiet: its lines say what it takes; that of `from_user` and `route`
        // says where the words go (1.7, §8.2).
        assert_eq!(
            desc(4, "to"),
            concat!(
                "from_user, route: ",
                d_from_user_node!(),
                "\nsay: session:<id>, holder:<node>, holders:<node>/**, principal:<key>, or user; \
                 without to: at's holders or its next holder; at a closed node, the session that \
                 closed it while live"
            )
        );
        assert!(!desc(4, "said_id").contains("It picks the form"));
        assert!(
            input_schema(&TOOLS[1])["properties"]["path"]
                .get("description")
                .is_none(),
            "the node a call is about is quiet"
        );
        assert_eq!(desc(0, "session"), d_session!());
        assert_eq!(desc(2, "session"), d_session!());
        for tool in &TOOLS {
            let s = Value::Object(input_schema(tool)).to_string();
            for repeated in [
                "actions: ",
                "required for: ",
                "A path or #<id>, not a key",
                "in 10m",
            ] {
                assert!(!s.contains(repeated), "{}: {repeated}", tool.name);
            }
        }
        // Shapes that differ between actions: one schema that takes them all.
        let view = input_schema(&TOOLS[1]);
        assert_eq!(
            view["properties"]["since"]["type"],
            json!(["integer", "string"])
        );
        let node = input_schema(&TOOLS[2]);
        assert_eq!(
            node["properties"]["body"]["type"],
            json!(["string", "null"]),
            "create takes a text, update a text or null"
        );
        assert_eq!(
            node["properties"]["repeat"]["type"],
            json!(["object", "null"])
        );
        assert_eq!(node["properties"]["title"]["type"], json!("string"));
        let talk_s = input_schema(&TOOLS[4]);
        assert_eq!(
            talk_s["properties"]["kind"]["enum"],
            json!(["ack", "question", "result", "reply", "correction"]),
            "one enum for to_user's and said's kinds"
        );
        // A nested object keeps its closed shape; `waits_for`'s variants are an anyOf.
        let hold_s = Value::Object(input_schema(&TOOLS[3]));
        let release = |w: Value| json!({"action": "release", "session": "s-4", "token": "1.1", "done": "d", "next_step": "n", "reason": "blocked", "waits_for": w});
        assert!(valid(&hold_s, &release(json!({"question": "e9"}))));
        assert!(!valid(
            &hold_s,
            &release(json!({"question": "e9", "node": "/a"}))
        ));
        assert!(!valid(&hold_s, &release(json!({"external": "x"}))));
        // The schema takes a call that the decoder takes without one of its arguments (1.7):
        // `evidence` is an argument of `decide`, and records nothing for a `say`.
        let note = json!({"action": "say", "session": "s-4", "at": "/a", "text": "seen"});
        assert!(valid(&Value::Object(talk_s.clone()), &note));
        let Value::Object(note) = note else { panic!() };
        let mut evidence = note.clone();
        evidence.insert("evidence".into(), json!("reported"));
        assert!(valid(
            &Value::Object(talk_s.clone()),
            &Value::Object(evidence.clone())
        ));
        let p = prepared(&TOOLS[4], &evidence).unwrap();
        assert_eq!(p.cmd, prepared(&TOOLS[4], &note).unwrap().cmd);
        assert_eq!(
            p.ignored_detail().as_deref(),
            Some(
                "talk say does not take evidence (talk decide takes it), so the call was applied without it"
            )
        );
        // 1.8 (§20.7, it2 l1): the schema takes `blocking` beside `to: user`, one object for all
        // forms, but the decoder refuses it: it marks another form of `ask`, and the hint ends with
        // every form's signature.
        let open = json!({"action": "ask", "session": "s-4", "at": "/a", "text": "?", "to": "user", "options": []});
        assert!(valid(&Value::Object(talk_s.clone()), &open));
        let Value::Object(open) = open else { panic!() };
        decodes(&TOOLS[4], &open).unwrap();
        let mut blocking = open.clone();
        blocking.insert("blocking".into(), json!(true));
        assert!(valid(
            &Value::Object(talk_s.clone()),
            &Value::Object(blocking.clone())
        ));
        let Err(ShellAnswer::Invalid(e)) = prepared(&TOOLS[4], &blocking) else {
            panic!("blocking with to: user is refused");
        };
        assert_eq!(e.arg, "blocking");
        assert!(
            e.why.contains("to ask a person in a thread, leave out to"),
            "{}",
            e.why
        );
        assert_eq!(
            e.signature.lines().collect::<Vec<_>>(),
            [
                "talk ask for a human (to: user or principal:<key>) takes {at*, text*, to*, \
                 options*, recommendation, default, due, again}",
                "talk ask for a person outside the swarm (no to) takes {at*, text*, where, again, \
                 blocking}",
                "talk ask for agents (to: session:, holder:, or holders:) takes {at*, text*, to*, \
                 again, blocking}",
            ]
        );
        // `session` is in `required`, and the decoder still reads it from the binding (§20.1).
        let mut without = open.clone();
        without.remove("session");
        assert!(!valid(
            &Value::Object(talk_s),
            &Value::Object(without.clone())
        ));
        decodes(&TOOLS[4], &without).unwrap();
    }

    /// Each argument's description carries the description of every form that takes it.
    #[test]
    fn each_argument_carries_every_actions_description_of_it() {
        for tool in &TOOLS {
            let s = input_schema(tool);
            for name in tool.arg_names() {
                let d = s["properties"][name]["description"]
                    .as_str()
                    .unwrap_or_default();
                for (a, f, x) in tool
                    .takers(name)
                    .into_iter()
                    .filter(|(_, _, x)| x.described)
                {
                    assert!(
                        d.contains(x.desc),
                        "{} {} {name}: {d}",
                        tool.name,
                        a.form_title(f)
                    );
                }
            }
        }
    }

    /// Every argument, at every depth, has a description.
    #[test]
    fn every_argument_has_a_description() {
        fn walk(args: &[Arg], at: &str) {
            for a in args {
                assert!(!a.desc.trim().is_empty(), "{at}.{}", a.name);
                match a.kind {
                    Kind::Object(inner) => walk(inner, &format!("{at}.{}", a.name)),
                    Kind::OneOf(variants) => {
                        for v in variants {
                            walk(v, &format!("{at}.{}", a.name));
                        }
                    }
                    _ => {}
                }
            }
        }
        walk(&COMMON, "common");
        walk(BASIS_EXTERNAL_ARGS, "basis");
        for tool in &TOOLS {
            for action in tool.actions {
                for form in action.forms {
                    walk(form.args, &format!("{} {}", tool.name, action.name));
                }
            }
        }
        // And the schema carries them, but those of quiet arguments (1.7), whose name, type, and
        // line say it all: the node a call is about, here.
        let s = input_schema(&TOOLS[3]);
        assert_eq!(
            s["properties"]["token"]["description"],
            json!("Your hold's token, such as 12.3")
        );
        assert!(s["properties"]["target"].get("description").is_none());
    }

    // ------------------------------------------------------------ §20.2's argument table

    /// The rows of §20.2's argument table: (argument cell, description cell).
    fn table() -> Vec<(String, String)> {
        let start = PROTOCOL
            .find("| Argument | Its description says |")
            .unwrap();
        PROTOCOL[start..]
            .lines()
            .skip(2)
            .take_while(|l| l.starts_with('|'))
            .map(|l| {
                let inner = l.trim_start_matches("| ").trim_end_matches(" |");
                let (a, d) = inner.split_once(" | ").unwrap();
                (a.to_string(), d.to_string())
            })
            .collect()
    }

    /// What a row's cell says, as the description carries it: without backticks and references,
    /// and without the words that say what a description says; capitalised, unless it starts with
    /// a value in backticks, such as `handoff` (1.10).
    fn content(cell: &str) -> String {
        let code = cell.starts_with('`');
        let refs = Regex::new(r"\s*\((?:§[0-9.]+|[A-Z][0-9]+)(?:,\s*(?:§[0-9.]+|[A-Z][0-9]+))*\)")
            .unwrap();
        let s = refs.replace_all(&cell.replace('`', ""), "").to_string();
        if let Some(lines) = s.strip_prefix("One line per value: ") {
            return lines.replace("; ", "\n");
        }
        let s = ["Also that ", "Also what it bounds: ", "That "]
            .iter()
            .find_map(|p| s.strip_prefix(p))
            .map_or(s.clone(), str::to_string);
        if code {
            return s;
        }
        let mut c = s.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    }

    /// Every argument of a kind, at any depth: (tool, action, path, description).
    fn args_where(pred: impl Fn(&Arg) -> bool + Copy) -> Vec<(String, &'static str)> {
        fn walk(
            args: &'static [Arg],
            at: &str,
            pred: impl Fn(&Arg) -> bool + Copy,
            out: &mut Vec<(String, &'static str)>,
        ) {
            for a in args {
                let here = format!("{at}.{}", a.name);
                if pred(a) {
                    out.push((here.clone(), a.desc));
                }
                match a.kind {
                    Kind::Object(inner) => walk(inner, &here, pred, out),
                    Kind::OneOf(variants) => {
                        for v in variants {
                            walk(v, &here, pred, out);
                        }
                    }
                    Kind::Basis => walk(BASIS_EXTERNAL_ARGS, &here, pred, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        for tool in &TOOLS {
            for action in tool.actions {
                for form in action.forms {
                    walk(
                        form.args,
                        &format!("{} {} {}", tool.name, action.name, form.name),
                        pred,
                        &mut out,
                    );
                }
            }
        }
        out
    }

    /// The description of one argument: `tool action form path`.
    fn desc(locator: &str) -> &'static str {
        let all = args_where(|_| true);
        all.iter()
            .find(|(here, _)| here == locator)
            .unwrap_or_else(|| panic!("no argument {locator}"))
            .1
    }

    /// D.12.1 (R20): every argument description of §20.2's table.
    #[test]
    fn the_argument_descriptions_carry_section_20_2() {
        let rows = table();
        assert_eq!(rows.len(), 35);
        for (arg, cell) in &rows {
            let want = content(cell);
            let targets: Vec<(String, &'static str)> = match arg.replace('`', "").as_str() {
                "from_user of hello" => {
                    vec![("hello".into(), desc("session hello hello.from_user"))]
                }
                "kind of to_user and said" => vec![
                    ("to_user".into(), desc("talk to_user to_user.kind")),
                    ("said".into(), desc("talk said record.kind")),
                ],
                // 1.7 (§8.2): `to` of from_user and route, and `node`, read as `to`.
                "to of from_user and route" => vec![
                    ("from_user".into(), desc("talk from_user from_user.to")),
                    ("node".into(), desc("talk from_user from_user.node")),
                    ("route".into(), desc("talk route route.to")),
                    ("hello".into(), desc("session hello hello.from_user.to")),
                ],
                "lease" => vec![
                    ("take".into(), desc("hold take take.lease")),
                    ("next".into(), desc("hold next next.lease")),
                ],
                "repeat" => vec![
                    ("create".into(), desc("node create create.repeat")),
                    ("update".into(), desc("node update update.repeat")),
                ],
                "run_within" => vec![(
                    "create".into(),
                    desc("node create create.repeat.run_within"),
                )],
                "since" => vec![
                    ("feed".into(), desc("view feed feed.since")),
                    ("user_log".into(), desc("view user_log user_log.since")),
                ],
                "A request's origin" => vec![
                    ("request".into(), desc("talk request request.origin")),
                    ("create".into(), desc("node create create.request.origin")),
                ],
                "re" => vec![("to_user".into(), desc("talk to_user to_user.re"))],
                "intent of checkpoint" => vec![(
                    "checkpoint".into(),
                    desc("hold checkpoint checkpoint.intent"),
                )],
                "evidence" => vec![("decide".into(), desc("talk decide decide.evidence"))],
                "options" => vec![("ask".into(), desc("talk ask human.options"))],
                "op_id" => vec![("common".into(), COMMON[1].desc)],
                "question of waits_for" => vec![(
                    "release".into(),
                    desc("hold release release.waits_for.question"),
                )],
                "status of view list" => vec![("list".into(), desc("view list list.status"))],
                "caps of hello" => vec![("hello".into(), desc("session hello hello.caps"))],
                "horizon of hello" => vec![("hello".into(), desc("session hello hello.horizon"))],
                "text of decide" => vec![("decide".into(), desc("talk decide decide.text"))],
                "confirmed of said" => {
                    vec![("said".into(), desc("talk said confirm.confirmed"))]
                }
                "principal of to_user" => {
                    vec![("to_user".into(), desc("talk to_user to_user.principal"))]
                }
                // 1.8.
                "at of to_user" => vec![("to_user".into(), desc("talk to_user to_user.at"))],
                // 1.6.
                "verbatim of to_user" => {
                    vec![("to_user".into(), desc("talk to_user to_user.verbatim"))]
                }
                "basis of decide" => vec![("decide".into(), desc("talk decide decide.basis"))],
                "checked_at" => {
                    vec![("decide".into(), desc("talk decide decide.basis.checked_at"))]
                }
                // 1.7.
                "session" => vec![("common".into(), COMMON[0].desc)],
                "version of update" => vec![("update".into(), desc("node update update.version"))],
                "digest_version of update" => {
                    vec![("update".into(), desc("node update update.digest_version"))]
                }
                // 1.8.
                "verified of close" => vec![("close".into(), desc("node close close.verified"))],
                "needs of create, and needs_add of update" => vec![
                    ("create".into(), desc("node create create.needs")),
                    ("update".into(), desc("node update update.needs_add")),
                ],
                "on of object" => vec![("object".into(), desc("talk object object.on"))],
                // 1.9.
                "outcome of close" => vec![("close".into(), desc("node close close.outcome"))],
                // 1.10.
                "stands of resolve" => {
                    vec![("resolve".into(), desc("talk resolve resolve.stands"))]
                }
                // 1.12.
                "defer of resolve" => {
                    vec![("resolve".into(), desc("talk resolve resolve.defer"))]
                }
                "unsent of close and release" => vec![
                    ("close".into(), desc("node close close.unsent")),
                    ("release".into(), desc("hold release release.unsent")),
                ],
                "reason of release" => {
                    vec![("release".into(), desc("hold release release.reason"))]
                }
                other => panic!("a row of §20.2 not covered: {other}"),
            };
            assert!(!targets.is_empty(), "{arg}");
            for (here, d) in targets {
                assert!(
                    d.contains(&want),
                    "{arg} at {here}:\n  {d}\nlacks\n  {want}"
                );
            }
        }
    }

    /// 1.7 (§20.2): what every node, time, and duration argument's description said is said once,
    /// in the description of each tool that takes one, and in no argument's; that any call's node
    /// can be named `path`, in that of each tool with an action about a node.
    #[test]
    fn each_tool_says_the_node_time_and_duration_rules_once() {
        fn kinds(args: &'static [Arg], out: &mut Vec<Kind>) {
            for a in args {
                out.push(a.kind);
                match a.kind {
                    Kind::Object(inner) => kinds(inner, out),
                    Kind::OneOf(variants) => {
                        for v in variants {
                            kinds(v, out);
                        }
                    }
                    Kind::Basis => kinds(BASIS_EXTERNAL_ARGS, out),
                    _ => {}
                }
            }
        }
        const NODE: &str = "A node is a path or #id, never a key";
        const PATH: &str = "any call's node can be named path";
        const TIMES: &str = "Times: RFC 3339 or in 10m";
        const DURATIONS: &str = "durations: 50s, 15m, 2h, 7d";
        for (tool, spec) in crate::decode::Tool::ALL.into_iter().zip(&TOOLS) {
            let mut all = Vec::new();
            for (_, f) in spec.forms() {
                kinds(f.args, &mut all);
            }
            let d = crate::mcp::tools::description(tool);
            let node = all.iter().any(|k| {
                matches!(
                    k,
                    Kind::Node | Kind::Nodes | Kind::Place | Kind::NodeAt | Kind::RouteTo
                )
            });
            // The aliases are for the node a call is about: `session` takes a node only as the
            // routing target of `hello {from_user}` (1.7, §8.2), which no alias names.
            let about_one = spec
                .forms()
                .any(|(a, f)| about(spec.name, a.name, f).is_some());
            let time = all.contains(&Kind::When);
            let dur = all.contains(&Kind::Dur);
            assert_eq!(d.contains(NODE), node, "{}", spec.name);
            assert_eq!(d.contains(PATH), about_one, "{}", spec.name);
            assert_eq!(d.contains(TIMES), time, "{}", spec.name);
            assert_eq!(d.contains(DURATIONS), dur, "{}", spec.name);
        }
        assert!(crate::mcp::tools::TALK.contains("a conversation key goes in where"));
        assert!(crate::mcp::tools::TALK.contains("approve {on} adds @<version>"));
        assert!(crate::mcp::tools::NODE.contains("path is the new node's own full path"));
    }
}
