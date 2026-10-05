//! The six tools, their descriptions, and the server's instructions (design D.11.5, §20.2, §22).
//!
//! The descriptions are §20.2's table verbatim, and the instructions §22's text verbatim, at most
//! `instructions.max` characters (1.4, R1, R5, R6; 1.5; 1.6; 1.7); the tests read both from
//! `docs/protocol.md`. The `view` description names `agreements` (§20.2, D.14.5), the action
//! `ViewCmd::Agreements` serves. Since 1.6 (the usability lab's first round), the instructions,
//! the `session` and `talk` descriptions, and `hello`'s `from_user` say that a scheduler's prompt
//! or a tool's output is never `from_user`; the descriptions carry the signatures of the common
//! actions, `?` marking an optional argument; an intent is for an outside write; and the `control`
//! description says to act only on a health reason you can fix. Since 1.7 (the lab's second
//! round), each description says once what its arguments' descriptions repeated: a node is a path
//! or `#id`, and `path` names the node a call is about; the times and durations; the `session`
//! description says what the output schema said of `next_calls`, since no tool has one; and the
//! texts finish with a reply and a close, which settles the requests the reply answered. Since
//! 1.11 (the lab's sixth round), the finish replies to the requests, if any, and with none closes
//! with a summary; a said's kind is ack for "on it"; and work waiting for its merge is decided
//! before its release. Since 1.12 (the seventh round), work waiting for its merge is released
//! blocked with `settles`, which records the decision, and no decide; a thread's message is
//! recorded as a request before the answer; and a human's words that need no work are more than a
//! thanks.

use std::sync::Arc;

use rmcp::model::Tool as McpTool;

use super::schema::input_schema;
use crate::decode::Tool;

/// The MCP server instructions: §22's text, 512 characters (1.8: one hello per run, a human's
/// later words, and a close only of a node the session holds, in place of `node create`'s line;
/// 1.9: "push, PR, deploy" in place of "an outside write", which read as local edits, it3; 1.11:
/// the finish replies to the requests, if any, and with none closes with a summary, it5 l3, in
/// place of the series' release, the close's condition, and the release of unfinished work, which
/// rule 8 and the `node`, `talk`, and `hold` descriptions say; 1.12: a thread's message is recorded
/// as a request, it6 l3, and work waiting for its merge is released blocked, it6 l7, in place of
/// what the constitution repeats: agents who never return, calls record and you send, the later
/// words' `talk from_user`, and the close with a summary, which the `talk` description says).
pub const INSTRUCTIONS: &str = "Swarm forum: memory for agents. Call session hello once per run; \
on later turns pass the session you have. If a human wrote to you directly (not a scheduler or \
tool), log their words verbatim as from_user. Your human's result or question: talk to_user; a \
thread's message: record it as a request before answering; your post: talk said; push, PR, \
deploy: hold checkpoint {intent}. Done: reply to the requests, if any, then close; waiting for a \
merge: release blocked with waits_for. next_calls: make the now ones.";

/// The `session` tool's description (§20.2; 1.6: never a scheduler's prompt or a tool's output;
/// 1.7: wait for `hello`, and what `?` means in a next call, which the output schema said; the node
/// rule, for the `to` of `hello`'s `from_user`; 1.8: one `hello` per run, it2 l2, l4; 1.9: the
/// principal a `from_user` names, `user` when the client names none, it3 l2, l3, l5, l6).
pub const SESSION: &str = "Call hello once per run and wait for its reply, then pass its session \
(s-N) on every call; on later turns pass the session you have. hello {principal?, from_user?: {text, client, answers?}}: from_user logs, \
verbatim, the words a human wrote to you directly, never a scheduler's prompt or a tool's output \
(principal: as your client's configuration names the human, else user). A node is a path or #id, \
never a key. Returns holds, inbox, offers, and the questions and digest for your principals: ask and \
summarise those. status {text}. Responses end with next_calls, the calls to make next with ids \
filled in: make the now ones; you write each <arg>: ?, and an <arg>?: ? only if it applies.";

/// The `view` tool's description (§20.2; 1.7: the brief comes with take, next, and create).
pub const VIEW: &str = "Read only. A node is a path or #id, never a key; any call's node can be \
named path. find {key|text}: does it exist? node {path}: its brief; take, next, and create return \
the brief: re-read only before a write after time has passed. requests: your principals' request \
board (requester: all for everyone's). agreements: how a subtree works. Also feed {at, since?}, \
list {under?}, history, user_log. Lists say shown/total.";

/// The `node` tool's description (§20.2; 1.5, R3: decide and reply only to a node's requests; 1.6:
/// the signatures, and `path` as the new node's own path; 1.7: `update`'s versions, and the close
/// that settles what the reply answered; 1.8: `verified`, and a close of a node you hold).
pub const NODE: &str = "A node is a path or #id, never a key; any call's node can be named path. \
create {path, title, keys?}: create-or-join by key; path is the new node's own full path, such as \
/area/name, not its parent's; returns {existing} or your hold. update {path, version, \
digest_version (with digest), …}: the versions of the brief you were shown, which the server fills \
in when you leave them out; null clears a field. close {path, outcome, summary, verified?} a node \
you hold, when finished; first reply to its requests, if any: the close settles those you answered. Also key, link {from, \
rel, to}, move {path, to}, merge {from, into}, reopen, hide. Times: RFC 3339 or in 10m; durations: \
50s, 15m, 2h, 7d.";

/// The `hold` tool's description (§20.2, 1.4: R6; 1.6: the signatures, and an intent for an outside
/// write; 1.8: not for a post in a thread, whose said is its record, it2 l1, l3; 1.9: for something
/// others will see, never local edits or tests, it3 l1, l3, l4; 1.10: work waiting for a review or
/// merge is released blocked, not handed off, it4 l7; 1.11: its requests are replied to and
/// decided before that release, which records no decision, it5 l1, l2; 1.12: that release takes
/// `settles`, and its signature lists `intent_done` and `settles`, it6 l7, l2, l1: "decide now"
/// made workers file a pull request's status as a decision).
pub const HOLD: &str = "Claim work. A node is a path or #id, never a key; any call's node can be \
named path. next takes the top offer; take {target}. checkpoint {token, done, next_step, intent?} \
about every 5 min; intent before something others will see: a push, PR, deploy, job, or mail; \
local edits, tests, and reads need none, nor a thread post. release {token, done, next_step, \
reason, waits_for?, intent_done?, settles?} if unfinished; work done, waiting for its merge: \
release blocked, settles: [the requests you answered], waits_for {external: 'PR #N merged', \
recheck_after}. Times: RFC 3339 or in 10m; durations: 50s, 15m, 2h, 7d.";

/// The `talk` tool's description (§20.2, 1.4: R1, R5; 1.5, R3: a scheduler's prompt; 1.5, the
/// final usability round: a `to_user` reports to the requester of another principal's request;
/// 1.6: a tool's output, and the signatures of the common actions; 1.7: the finish, a reply and a
/// close, and what a `to_user` records; 1.8: both forms of `ask`, a human's later messages, and a
/// close only of a node you hold, it2 l1, l2, l4, l6; 1.9: nor a client's instruction files, and a
/// thanks or an ack from a person in a thread is `heard`, it3 l1, l6; 1.10: where the outcome of
/// one's own work goes, and that say is a note to other agents, it4 l7; 1.11: a said's kind for
/// "on it" and for the outcome, it5 l1, a finish with no request, it5 l3, and a follow-up to the
/// person's open request logged without `to`, so that the default clarifies it, it5 l3, l4; 1.12:
/// a decision records a choice or settles requests, and the outcome of the work goes in a close's
/// summary or a release's done, it6 l7; a node nobody asked for needs no `to_user`, it6 l2;
/// words that need no work include an ack and a question answered in the reply, it6 l4; and a
/// thread's request names its required `text`, as `hello`'s `end` does).
pub const TALK: &str = "A node is a path or #id, never a key: a conversation key goes in where; \
any call's node can be named path; approve {on} adds @<version>. Your human: from_user {text, \
client, to?, answers?} when they wrote to you directly (a DM, the IDE, your mailbox; log first; \
principal as in hello), never for a scheduler's prompt, a tool's output, or your client's \
instruction files; every message your human writes after the first: talk from_user, even after you \
close; to: none when it needs no work: a thanks, an ack such as 'I'll review', or a question you \
answer in your reply (to_user, re it); without to: a follow-up that changes their open request on \
the node you hold; to: <node>: new work. to_user {text, kind, re?} records a result, a question, or a promise to a human, not \
progress, and sends nothing: to your own, whom you then tell yourself, or to the requester of a \
request you finished, who reads it on their request board, so you need not reach them. Ask your \
human: ask {at, text, to: user | principal:<key>, options}, which you then put to them yourself. \
People: a message in a shared thread is a request (node create {path, title, keys: [<thread>], \
request: {origin, requester, text}}), never from_user; a thanks or an ack: heard {at, from, text, \
answers?}, not a request; said {at, where?, text, kind} before sending: kind ack for 'on it', \
result for the outcome; ask a person in a thread: ask {at, where?, text, blocking?} with no to (it \
records the said for you), once. Agents: say {at, text, to?}. Done: reply to the requests, if any \
(to_user or said, kind result), then close the node if you hold it: the close settles those you \
answered; with none, close with a summary: a node nobody asked for needs no to_user. decide {at, \
text, settles?} records a choice, or settles requests; the outcome of your work goes in close's \
summary, or in a release's done while it waits for a merge; say is a note to other agents. object {on, text} flags a problem. \
route sorts user messages. Times: RFC 3339 or in 10m; durations: 50s, 15m, 2h, 7d.";

/// The `control` tool's description (§20.2; 1.6: act only on a health reason you can fix).
pub const CONTROL: &str = "Swarm-wide. A node is a path or #id, never a key; any call's node can \
be named path. pause {scope, reason} stops new work (anyone; expires in 2 h); unpause {scope}; \
health: the status line names its first reason; act only on a reason you can fix, such as a stale \
pause or a node you can take, and otherwise go on; export; constitution. For one node, prefer talk \
object.";

/// A tool's description.
#[must_use]
pub fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::Session => SESSION,
        Tool::View => VIEW,
        Tool::Node => NODE,
        Tool::Hold => HOLD,
        Tool::Talk => TALK,
        Tool::Control => CONTROL,
    }
}

/// The six tools, in §20's order, with their descriptions and input schemas, and no output schema
/// (1.7, §20.1): `structuredContent` is sent without one, which MCP allows, and what the schema
/// said of `next_calls` is in the `session` description.
#[must_use]
pub fn tools() -> Vec<McpTool> {
    Tool::ALL
        .into_iter()
        .map(|t| McpTool::new(t.name(), description(t), Arc::new(input_schema(t.spec()))))
        .collect()
}

/// Why configured instructions are refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionsTooLong {
    /// Their length in characters.
    pub chars: usize,
    /// `instructions.max`.
    pub max: u32,
}

/// The instructions to serve: §22's text for `default` (the configuration's default), or a custom
/// text of at most `instructions.max` characters (D.11.9).
pub fn instructions(configured: &str, max: u32) -> Result<String, InstructionsTooLong> {
    let text = if configured == "default" {
        INSTRUCTIONS
    } else {
        configured
    };
    let chars = text.chars().count();
    if u32::try_from(chars).map_or(true, |c| c > max) {
        return Err(InstructionsTooLong { chars, max });
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROTOCOL: &str = include_str!("../../../../docs/protocol.md");

    /// §22's instructions: the blockquote after "MCP server `instructions`".
    fn spec_instructions() -> String {
        let start = PROTOCOL.find("**MCP server `instructions`.**").unwrap();
        let rest = &PROTOCOL[start..];
        rest.lines()
            .skip_while(|l| !l.starts_with("> "))
            .take_while(|l| l.starts_with("> "))
            .map(|l| &l[2..])
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// §20.2's descriptions, by tool.
    fn spec_descriptions() -> Vec<(String, String)> {
        let start = PROTOCOL.find("### 20.2. Tool descriptions").unwrap();
        PROTOCOL[start..]
            .lines()
            .skip_while(|l| !l.starts_with("| Tool | Description |"))
            .skip(2)
            .take_while(|l| l.starts_with('|'))
            .map(|l| {
                let cells: Vec<&str> = l.split(" | ").collect();
                let tool = cells[0]
                    .trim_start_matches("| ")
                    .trim_matches('`')
                    .to_string();
                let desc = l[l.find(" | ").unwrap() + 3..]
                    .trim_end_matches(" |")
                    .replace("\\|", "|");
                (tool, desc)
            })
            .collect()
    }

    #[test]
    fn the_instructions_are_section_22_verbatim_within_instructions_max() {
        assert_eq!(INSTRUCTIONS, spec_instructions());
        assert_eq!(INSTRUCTIONS.chars().count(), 512);
        assert!(PROTOCOL.contains("The text below has 512 characters."));
        assert!(INSTRUCTIONS.chars().count() <= 512, "instructions.max");
        // 1.8 (the lab's third round): one hello per run, a human's later words, and a finish that
        // closes only a node the session holds; `node create`'s line made room for them.
        assert!(INSTRUCTIONS.contains(
            "Call session hello once per run; on later turns pass the session you have."
        ));
        // 1.12 (the seventh round): the later words' `talk from_user` is left to the `talk`
        // description and constitution line 2.
        assert!(INSTRUCTIONS.contains("log their words verbatim as from_user."));
        // 1.11 (the sixth round, it5 l3): a worker whose node had no request looked for someone to
        // reply to; the close's condition, the series' release, and the release of unfinished work
        // are left to the descriptions and rule 8. 1.12 (it6 l7): every task of the parallel
        // forum waited for its merge, which the finish did not name.
        assert!(INSTRUCTIONS.contains(
            "Done: reply to the requests, if any, then close; waiting for a merge: release blocked \
             with waits_for."
        ));
        // 1.12 (it6 l3): a chat worker answered a thread's questions without recording them.
        assert!(INSTRUCTIONS.contains(
            "a thread's message: record it as a request before answering; your post: talk said;"
        ));
        assert!(!INSTRUCTIONS.contains("thread posts: talk said"));
        assert!(!INSTRUCTIONS.contains("Done: reply, then close"));
        assert!(!INSTRUCTIONS.contains("node create"));
        // 1.7 (the lab's second round): a finish needs no decision; a `to_user` gives the human a
        // result or asks a question.
        assert!(!INSTRUCTIONS.contains("decide"));
        assert!(!INSTRUCTIONS.contains("who never return"));
        assert!(INSTRUCTIONS.contains("Your human's result or question: talk to_user"));
        // 1.6 (the lab's first round): a scheduler or a tool is not a human, and an intent is for
        // a write.
        assert!(INSTRUCTIONS.contains(
            "If a human wrote to you directly (not a scheduler or tool), log their words verbatim"
        ));
        assert!(!INSTRUCTIONS.contains("outside action"));
        // 1.9 (the fourth round): "an outside write" read as local edits; an intent is for what
        // others will see.
        assert!(INSTRUCTIONS.contains("push, PR, deploy: hold checkpoint {intent}"));
        assert!(!INSTRUCTIONS.contains("outside write"));
    }

    /// 1.7 (§20.2): the tool list, as minified JSON, is at most `TOOLS_MAX`; it was 56,687 bytes in
    /// 1.6, a third of it six copies of one output schema. No tool has an output schema (§20.1).
    /// 1.8: 25 KiB, for the descriptions 1.7's cut lost and agents needed (it2): 24,399 bytes in
    /// 1.7, 25,424 in 1.8's group A, 25,563 when 1.8 was done; 1.9's words and records, 25,570,
    /// trimmed to fit (`to` of `from_user` and `route`, and the `reason` of a route to none); 1.10's
    /// inputs and texts, 25,580, trimmed to fit (`corrects`, `seen`, `reply_to`, `until`, and
    /// `supersedes` are quiet); 1.11's texts and responses, 25,563, trimmed to fit (`tags` of `view
    /// list`, `requester` of `view requests`, and `capacity` and `agreements` are quiet); its
    /// reviews and conversations, 25,600 on their own, with `defer` of `resolve` quiet, which
    /// `stands` names, and `stands` and `on` of `object` shorter; both, 25,571; its follow-up
    /// default, 25,767, trimmed to fit (`again` of `ask`, `why` of `renew`, and `tags` of `next`'s
    /// `query` are quiet), 25,581. 1.12's texts and responses, 25,991, trimmed to fit (`answers`,
    /// `clarifies`, `reason`, and `to` of `hello`'s `from_user`, `default` of `ask`, `for` of
    /// `wait`, `path` of `retract`, and `after` and `limit` of `export` are quiet), 25,322; its
    /// finishing, notes, and questions, with `settles` of `release` quiet, 25,384; its fixes to the
    /// texts it had left (`text` of `decide`, `to` of `say`, the thread's request), 25,472.
    #[test]
    fn the_tool_list_is_within_tools_max() {
        let list = serde_json::to_string(&tools()).unwrap();
        assert!(
            list.len() <= super::super::schema::TOOLS_MAX,
            "the tool list has {} bytes, over {}",
            list.len(),
            super::super::schema::TOOLS_MAX
        );
        assert!(PROTOCOL.contains("is at most 25 KiB as minified JSON"));
        assert_eq!(super::super::schema::TOOLS_MAX, 25 * 1024);
        for t in tools() {
            assert!(t.output_schema.is_none(), "{}", t.name);
        }
    }

    /// §22's constitution: the numbered blockquote after "## 22. Constitution", one item per line
    /// number.
    fn spec_constitution() -> Vec<String> {
        let start = PROTOCOL.find("## 22. Constitution").unwrap();
        let mut items: Vec<String> = Vec::new();
        for l in PROTOCOL[start..]
            .lines()
            .skip_while(|l| !l.starts_with("> 1. "))
            .take_while(|l| l.starts_with("> "))
        {
            let l = &l[2..];
            match l.split_once(". ") {
                Some((n, rest)) if n.parse::<u32>().is_ok() => items.push(rest.to_string()),
                _ => {
                    let last = items.last_mut().unwrap();
                    last.push(' ');
                    last.push_str(l.trim_start());
                }
            }
        }
        items
    }

    #[test]
    fn the_constitution_is_section_22_verbatim() {
        assert_eq!(
            crate::render::hello::CONSTITUTION.to_vec(),
            spec_constitution()
        );
        // 1.7 (the lab's second round): no line sends agents to read health; the `control`
        // description says to act only on a reason you can fix (1.6).
        assert_eq!(crate::render::hello::CONSTITUTION.len(), 8);
        assert!(
            !crate::render::hello::CONSTITUTION
                .iter()
                .any(|l| l.contains("control health"))
        );
        assert!(CONTROL.contains("act only on a reason you can fix"));
        // 1.7: line 3 keys work rather than looking it up first.
        assert!(
            crate::render::hello::CONSTITUTION[2]
                .contains("joins an open node that has one; look up only what you cannot key")
        );
        // 1.6: a person who writes directly, and never a scheduler's prompt or a tool's output.
        assert!(
            crate::render::hello::CONSTITUTION[1]
                .starts_with("When a person writes to you directly,")
        );
        // 1.9 (the fourth round): nor a client's instruction files.
        assert!(crate::render::hello::CONSTITUTION[1].ends_with(
            "A scheduler's prompt, a tool's output, and your client's instruction files are not a \
             person's words."
        ));
        // 1.12 (the seventh round, it6 l3): a message in a shared thread is not to you directly;
        // it6 l7: a worker takes the next task while next offers one.
        assert!(crate::render::hello::CONSTITUTION[1].contains(
            "A message in a shared thread is not to you directly: record it as a request, or a \
             thanks as heard, before you answer."
        ));
        assert!(crate::render::hello::CONSTITUTION[0].contains(
            "Take on only what you can finish; when it is done, take the next while next offers \
             you work."
        ));
    }

    #[test]
    fn the_descriptions_are_section_20_2_verbatim() {
        let spec = spec_descriptions();
        assert_eq!(spec.len(), 6);
        for (t, (name, desc)) in Tool::ALL.into_iter().zip(&spec) {
            assert_eq!(t.name(), name);
            assert_eq!(description(t), desc, "{name}");
        }
        // 1.4's wording (R1, R5, R6) and D.14.5's agreements; 1.7: the node rule once per tool.
        assert!(TALK.starts_with(
            "A node is a path or #id, never a key: a conversation key goes in where; any call's \
             node can be named path;"
        ));
        assert!(VIEW.contains("agreements: how a subtree works."));
        // 1.11 (it5 l3, l4: every follow-up passed `to`, which makes a new request): a follow-up
        // to the person's open request is logged without `to`, which clarifies it (§8.2). 1.12
        // (it6 l4): an ack, and a question answered in the reply, need no work either.
        assert!(TALK.contains(
            "to: none when it needs no work: a thanks, an ack such as 'I'll review', or a question \
             you answer in your reply (to_user, re it); without to: a follow-up that changes their \
             open request on the node you hold; to: <node>: new work."
        ));
        // 1.7 (the lab's second round): the brief comes with take, next, and create; the finish
        // is a reply and a close, which settles what it answered; `update`, `say`, and `heard`
        // have their signatures; `hello` comes first, and its reply gives the session.
        assert!(VIEW.contains(
            "take, next, and create return the brief: re-read only before a write after time has \
             passed"
        ));
        assert!(!VIEW.contains("re-read before writing"));
        // 1.11 (the sixth round, it5 l3): with no request, the close's summary is the result.
        // 1.12 (it6 l2): and no to_user is needed.
        assert!(TALK.contains(
            "Done: reply to the requests, if any (to_user or said, kind result), then close the \
             node if you hold it: the close settles those you answered; with none, close with a \
             summary: a node nobody asked for needs no to_user."
        ));
        // 1.11 (it5 l1): a post that you are on it is an ack, the outcome a result.
        assert!(TALK.contains(
            "said {at, where?, text, kind} before sending: kind ack for 'on it', result for the \
             outcome;"
        ));
        // 1.8 (the lab's third round): both forms of ask, each with its signature; a human's later
        // messages; one hello per run.
        assert!(
            TALK.contains("Ask your human: ask {at, text, to: user | principal:<key>, options}")
        );
        assert!(TALK.contains(
            "ask a person in a thread: ask {at, where?, text, blocking?} with no to (it records the \
             said for you)"
        ));
        assert!(TALK.contains(
            "every message your human writes after the first: talk from_user, even after you \
             close;"
        ));

        assert!(SESSION.contains("on later turns pass the session you have"));
        for (d, sig) in [
            (
                NODE,
                "update {path, version, digest_version (with digest), …}",
            ),
            (TALK, "say {at, text, to?}"),
            (TALK, "heard {at, from, text, answers?}"),
        ] {
            assert!(d.contains(sig), "{sig} in {d}");
        }
        assert!(SESSION.starts_with("Call hello once per run and wait for its reply,"));
        // 1.5 (R3): a close replies only to the node's requests; 1.7: and settles what it answered;
        // 1.8: a node you hold, and how you checked it.
        assert!(NODE.contains(
            "close {path, outcome, summary, verified?} a node you hold, when finished; first reply \
             to its requests, if any: the close settles those you answered."
        ));
        assert!(!NODE.contains("decide"));
        // 1.5 (the final usability round): a `to_user` reports to a human, the requester of
        // another principal's request included, who reads it on their board; 1.7: a result, a
        // question, or a promise, never progress.
        assert!(TALK.contains(
            "records a result, a question, or a promise to a human, not progress, and sends \
             nothing: to your own, whom you then tell yourself, or to the requester of a request \
             you finished, who reads it on their request board, so you need not reach them"
        ));
        assert!(!TALK.contains("records what you tell your human"));
        // 1.6 (the lab's first round): a scheduler's prompt or a tool's output is never a human's
        // words, in every text a worker reads before its first call.
        assert!(SESSION.contains("directly, never a scheduler's prompt or a tool's output"));
        // 1.9 (the fourth round): nor a client's instruction files; and a from_user in a session
        // that serves no principal names one, user when the client names none.
        assert!(TALK.contains(
            "never for a scheduler's prompt, a tool's output, or your client's instruction files"
        ));
        assert!(
            SESSION
                .contains("(principal: as your client's configuration names the human, else user)")
        );
        // 1.9: a thanks or an ack from a person in a thread is heard, never a request.
        assert!(
            TALK.contains("a thanks or an ack: heard {at, from, text, answers?}, not a request")
        );
        // 1.6: the signatures of the common actions, required arguments bare and optional ones
        // marked `?`, with 1.6's argument names.
        for (d, sig) in [
            (
                SESSION,
                "hello {principal?, from_user?: {text, client, answers?}}",
            ),
            (NODE, "create {path, title, keys?}"),
            (NODE, "close {path, outcome, summary, verified?}"),
            (NODE, "link {from, rel, to}"),
            (HOLD, "checkpoint {token, done, next_step, intent?}"),
            (
                HOLD,
                "release {token, done, next_step, reason, waits_for?, intent_done?, settles?}",
            ),
            (TALK, "from_user {text, client, to?, answers?}"),
            (TALK, "to_user {text, kind, re?}"),
            (TALK, "said {at, where?, text, kind}"),
            (TALK, "decide {at, text, settles?}"),
            (TALK, "object {on, text}"),
        ] {
            assert!(d.contains(sig), "{sig} in {d}");
        }
        assert!(NODE.contains("path is the new node's own full path"));
        assert!(!TALK.contains("object {on: node}"));
        // 1.6: an intent is for an outside write; a read needs none. 1.8: nor a post in a
        // thread, whose said is its record. 1.9: something others will see, never local edits or
        // tests, which "any other outside write" let in.
        assert!(HOLD.contains(
            "intent before something others will see: a push, PR, deploy, job, or mail; local \
             edits, tests, and reads need none, nor a thread post."
        ));
        assert!(!HOLD.contains("outside write"));
        // 1.10 (the fifth round): work done but waiting for a review or merge is released blocked,
        // not handed off (it4 l7); the outcome of a worker's own work is no note (it4 l7).
        // 1.11 (the sixth round, it5 l1, l2): and its requests are settled before that release.
        // 1.12 (the seventh round, it6 l7, l2, l1, l6): by the release itself, with settles; "decide
        // now" made workers file a pull request's status as a decision.
        assert!(HOLD.contains(
            "work done, waiting for its merge: release blocked, settles: [the requests you \
             answered], waits_for {external: 'PR #N merged', recheck_after}."
        ));
        assert!(!HOLD.contains("decide"));
        assert!(TALK.contains(
            "decide {at, text, settles?} records a choice, or settles requests; the outcome of \
             your work goes in close's summary, or in a release's done while it waits for a merge; \
             say is a note to other agents."
        ));
        assert!(!TALK.contains("The outcome of your own work: decide"));
        assert!(!TALK.contains("records an outcome"));
        // Short enough to read: under 700 characters each, but `talk`, which carries the most.
        for t in Tool::ALL {
            if t != Tool::Talk {
                assert!(description(t).chars().count() < 700, "{}", t.name());
            }
        }
    }

    #[test]
    fn six_tools_in_order() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(
            names,
            ["session", "view", "node", "hold", "talk", "control"]
        );
        for t in &tools {
            assert_eq!(
                t.input_schema.get("type"),
                Some(&serde_json::json!("object"))
            );
            // 1.5: one object, which the Anthropic API takes (§20.2).
            for k in ["oneOf", "anyOf", "allOf"] {
                assert!(t.input_schema.get(k).is_none(), "{} {k}", t.name);
            }
        }
    }

    #[test]
    fn configured_instructions() {
        assert_eq!(instructions("default", 512).unwrap(), INSTRUCTIONS);
        assert_eq!(instructions("Be brief.", 512).unwrap(), "Be brief.");
        assert_eq!(
            instructions(&"é".repeat(513), 512),
            Err(InstructionsTooLong {
                chars: 513,
                max: 512
            })
        );
        assert!(
            instructions(&"é".repeat(512), 512).is_ok(),
            "characters, not bytes"
        );
        assert!(instructions("default", 500).is_err());
    }
}
