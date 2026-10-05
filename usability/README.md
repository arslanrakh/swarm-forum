# Usability test with live agents (WP-U)

The usability test of design D.13.2, WP-U: live agents of several models and vendors use the forum
through their vendors' own clients, with only this MCP server attached, in scripted scenarios, in two
arms, with and without the next-call guidance of §20.9. After each run a slip counter reads the
server's history and the server's responses, and an interview asks each agent session, resumed in
its own client, what it was doing and what was convenient or confusing. The report puts every cell
of the WP-U row side by side, with the agents' answers next to their slips. It spends model tokens
and runs only when called.

## Running it

```
usability/run.sh list                                     # the scenarios, and which can run now
usability/run.sh probe --model claude:claude-sonnet-5-5   # what each client loads besides the forum
usability/run.sh run --scenario s0-smoke --model claude:claude-sonnet-5-5 --arm g,nog --runs 1
usability/run.sh campaign --model claude:claude-sonnet-5-5 --model claude:claude-opus-5-5 \
  --model codex:gpt-6.1-sol --model codex:gpt-6-astra --allow-unisolated --runs 10
usability/run.sh report --out usability/out               # the report again, from saved runs
usability/run.sh recount --out usability/out              # the slip counter again, on saved runs
usability/run.sh interview --out usability/out            # interview saved runs that have no interview yet
usability/run.sh campaign --model script:none --force --prelude-only --arm g --runs 1   # dry run of the data
```

`run.sh` builds `forum-server` with the test-only cargo feature `usability` (debug only) and hands
over to the harness (`harness/`, Python 3.10 or later, standard library only). A model is
`claude:<model>` (the `claude` CLI) or `codex:<model>` (the `codex` CLI). Keys and logins come from
the environment and the CLIs' own logins, never from this repository. Runs land in `usability/out/`
(not tracked) unless `--out` says otherwise; `--parallel N` runs N at once, each on its own server.
`run` and `campaign` record what they were asked for in `<out>/manifest.json`, against which the
report looks for missing cells. `--interview all|none|N` chooses the runs whose agents are
interviewed after the count: all, none, or the first N runs of each cell (model, scenario, and arm),
3 by default (below). A Ctrl-C stops the runs in progress, each finishing as `interrupted` (not
counted, not interviewed), and starts no other. The harness's own tests need no model:
`python3 -m unittest discover -s usability/tests` (the end-to-end ones need the `usability` build).

**The campaign's models** are four, two from each vendor: `claude:claude-sonnet-5-5`,
`claude:claude-opus-5-5`, `codex:gpt-6.1-sol`, and `codex:gpt-6-astra` (`CAMPAIGN_MODELS` in
`harness/__main__.py`, which `campaign` runs unless `--model` names others). No other model is part
of the campaign. They are enough for the criterion of at least 3 models from 2 vendors (below), but
only with the Codex runs counted. For now the Codex models run with the operator's own `CODEX_HOME`
(`--allow-unisolated`), so their runs and interviews are reported as not isolated and left out of
every count, and the criterion reads "no" until they run in a dedicated home with a passed probe.
The campaign is run again periodically, as the server changes, with the same four models.

## The lab

The lab (`run.sh lab`, `harness/lab.py`) is the quick loop beside the campaign, for finding and fixing what
makes the tools hard to use: seeded forums at several stages of their life, three agents of three models on each,
every session interviewed, and a digest per run for a reader. No arms and no pass criteria.

```
usability/run.sh lab --out DIR                       # every seed, in parallel
usability/run.sh lab --out DIR --seed l1,l4 --rotate 1 --mix codex:gpt-6.1-sol,codex:gpt-6-astra,claude:claude-sonnet-5-5
usability/run.sh lab --out DIR --prelude-only        # play the seeds' preludes and stop
```

- **Seeds** (`harness/seeds.py`) are scenarios built in code: `l1-day-one` (empty), `l2-solo-month` (one
  developer), `l3-company-quarter` (ten people, three teams), `l4-mature-year` (about 300 closed nodes),
  `l5-research` (no human), and `l6-incident` (a pause, a vanished holder's intent, duplicates). Each has the
  intended flow for the reader, the world's facts, and three agents. The prelude plays the forum's past through the
  tools, and moves the forum's clock forward between episodes (`{"advance": "2d"}`): the server's lagged clock
  (feature `usability`: `usability_clock_lag` and `usability_clock_jump_file` in `forum.toml`) starts that far
  behind and catches up when the harness writes to its jump file, so ages, expiries, and retirements are the
  server's own, and the agents arrive at the real time.
- **Models.** Agent i of the j-th seed gets `mix[(i + j) % n]` (`--rotate` shifts it). Codex agents get the
  forum's tools as direct functions (`features.code_mode.direct_only_tool_namespaces=["mcp__forum"]`, Codex
  0.159.2; without it code mode defers them inside `exec`), and the server's instructions before their role,
  since Codex shows a model no MCP server's instructions.
- **The world, humans, and people** are played by a model (`harness/sim.py`, Claude Sonnet, no tools), from the
  seed's facts and the scripted rules' texts, and from what the same agent was told before. Keyword rules
  answered only what their words foresaw: in the first smoke run they answered reads with "printed nothing",
  fired on "fix" inside a read, and had a human thank the agent for a pull request that did not exist.
- **Output**: each seed's run directory as `run` writes it, plus `digest.md` (every turn, call, response, reply,
  and interview answer) and `stats.json`; `summary.md` over the seeds.

## Layout

| Path | What |
|---|---|
| `run.sh` | The entry point: builds the server, then `python3 -m harness` |
| `scenarios/params-short.json` | §22's durations shortened so that `Params::valid` holds and every deadline falls within an hour, keeping the default orders that shape what agents meet (below) |
| `scenarios/s*.json` | The scenarios as data (format: `harness/scenario.py`) |
| `harness/server.py` | A fresh database, port, and tokens per run; tokens reach the server only through its environment |
| `harness/proxy.py` | The pass-through every agent goes through: it logs the server's full response to every call, adapts the schemas for clients that refuse them, and saves the tools/list it served |
| `harness/agents.py` | The Claude and Codex drivers, and a scripted driver for tests |
| `harness/runner.py` | One run: prelude, agents, humans, people, world, fixed end points, drain, export |
| `harness/history.py` | The server's history replayed into the tables the counter needs, with the flags of §14 and the candidates of §10.4 that the history shows |
| `harness/slips.py` | The slip counter |
| `harness/packages.py` | Which core package builds each command, and which the server has: a refusal by a command not built yet is the server's gap |
| `harness/interview.py` | The interview after a run: the questions, the note that tells a session stopped right after a forum call which calls it made, each session resumed once in its own client with no tool, `interview.json`, what the client kept of the run's calls, and what the client kept of a Claude session copied into the run directory and removed from the client's, however the run ended |
| `harness/report.py` | The report |
| `harness/lab.py`, `harness/seeds.py`, `harness/sim.py` | The lab: its command and digests, its seeds, and its simulated world and people |
| `harness/isolation.py` | What a client loaded, and what Claude Code added to the model's context from its login (read from its record of the session): checked after every session, and by `probe` |
| `tests/` | The harness's tests: the counter rule by rule on synthetic runs (`synth.py`), the schema adapter on the server's tools/list (`data/tools-list.json`, captured from the server, and live), the report, and a scripted end-to-end run; the interview (`test_interview.py`) with fake `claude` and `codex` clients and a fake HOME: its command lines, its questions' exact words, the note on a session stopped after a call, its storage, the calls and tokens it attributes, the cell sampling, the campaign's models, the guard on the client's directory, the tidying however a run ends, a run with a fake server, proxy, and agent (no build needed), what Claude Code adds from its login and the probe that finds it, and the report's sections |

## The arms

- *With guidance* (`g`): the server as specified, in the version `docs/protocol.md` states (1.5
  now). The report's header reads it from there, and each run records the version it was made on
  (`protocol_version` in `run.json`).
- *Without guidance* (`nog`): the server's `usability_arm = "without_guidance"` switch (WP-E3): 1.2's
  instructions and `session` description, and no `next_calls` block. The other tool descriptions,
  the input schemas, and the output schema stay those of the version specified.

## Scenarios

Each scenario holds the humans' lines, the people outside the swarm and their replies, the world's
answers to outside actions, the schedule of agent sessions, the fixed points at which a session is
ended mid-task (the same in both arms), the server parameters, and a prelude of scripted calls that
sets the forum up. Agents have only the forum's tools: the roles tell them to write an outside
action as `ACTION: ...` and a post in a conversation as `POST: ...`; the harness answers from the
scenario's rules. The rest of a reply is what the agent's own human reads. Lines are delivered
between turns: a human never interrupts a turn. A person may leave a post unanswered, but every role
promises an action's result in the next message, so an action that no world rule answers gets "No
result: the platform could not do this action." (`WORLD_DEFAULT` in `harness/runner.py`; a scenario
may set its own `world_default`). Before this answer existed, a session whose reply ended in such an
action got no next message and ended there: in the first final round (2026-10-04), 69 of 154
sessions ended so, 34 of them Codex sessions whose first reply asked the platform to read the forum
and that never called it.

| Scenario | Flow (design D.13.2) | Runs now |
|---|---|---|
| `s0-smoke` | The harness's own check, not in the campaign: a worker vanishes mid-task, the next takes over | yes |
| `s1-user-series` | A message from the user; a series; a clarification (Wizard-of-Oz S1) | yes |
| `s2-person-thread` | A person's message in a team thread, fired twice (S2) | yes |
| `s3-company` | A company: Alice's IDE agent with two questions and a digest entry; a worker that answers Bob (S3) | yes |
| `s4-autonomous-intent` | No human: an open intent left behind, offers, a lapsed blocking question, a closed node whose basis changed (S4) | yes |
| `s5-takeover` | Long work with an outside action; a session ended after its intent (S5) | yes |
| `s6-weekly-reminder` | Scheduled work whose slot nothing covers (S6) | yes |
| `s7-research-basis` | Research: a refuted note under objection, a closed theorem whose basis changed (S7) | yes |
| `s8-newcomer` | A newcomer in a mature forum (D.14.5) | needs an orientation switch |
| `s9-tidying` | Tidying: leave_better, a duplicate to merge | yes |
| `s32-example` | §32's eight steps, every session live, s-5 ended after its intent | yes |

The campaign scenarios are translated from the Wizard-of-Oz rounds; what a live run cannot recreate
(28 days of session starts, events in the past) is listed in each file's `seed_limits`. Prelude
nodes that are information or offered work are created with `take: false`, so that no release puts
the offer tag on information. A scenario is ready when `harness/packages.py` (`BUILT`) has every
package its `needs_packages` names; `run.sh list` shows what each still needs. Every core package
up to WP-D7 has landed, so every campaign scenario but `s8-newcomer` is ready. A dry run of the
preludes (`--prelude-only`) against the whole core passes every one of them. `s3-company`'s prelude
waits 4 minutes for a question to lapse, which outlasts both the server's idle MCP session
(`presence.silence`, 2 minutes) and the seed's hold: the harness's MCP client starts a new MCP
session in the same forum session when the server has expired the old one (`harness/mcp.py`), and
the seed checkpoints its hold during the wait. `s8-newcomer` asks for a server switch that drops
`hello`'s orientation, which does not exist yet: its arm without orientation is listed in the file's
`extra_arms`, and the report counts it as a missing cell. Its prelude also stops at its fifth step,
which updates the root with version 2 after an `agreements` update that left the version at 1.

**Parameters.** `params-short.json` keeps every rule of `Params::valid`, and of the other orders
among §22's defaults it keeps those that shape what agents meet: `presence.silence = lease.first`
(a session that vanished leaves `present` when its first lease ends), and `refusal.ttl`,
`refusal.window`, and `archive.after` above `session.retain`. It inverts three, because the agents
run at their real speed while the deadlines are compressed: `question.recheck < lease.max`,
`user_question.due < lease.ceiling`, and `deliver.window > user_question.due`; keeping them would
push a recheck past the hour or cut a hold or a delivery below what a live agent needs.
`harness/params.py` (`ORDERS`) states each order and its reason, and a test checks them.

## Clients

**Claude Code** (`claude -p`, streaming JSON in and out, one process per agent session, so that each
human line continues the session) runs in an empty temporary directory with the forum as its only
MCP server (`--mcp-config`, `--strict-mcp-config`), only the forum's six tools (`--tools`,
`--allowedTools`), no settings source, no built-in plugin, no slash command or skill, and these
environment switches: `CLAUDE_CODE_DISABLE_CLAUDE_MDS`, `CLAUDE_CODE_DISABLE_AUTO_MEMORY`,
`CLAUDE_CODE_DISABLE_ORG_MEMORY`, `CLAUDE_CODE_DISABLE_BUNDLED_SKILLS`, `ENABLE_TOOL_SEARCH=false`,
and `ENABLE_CLAUDEAI_MCP_SERVERS=false`. Nothing else of the caller's environment reaches it. The
probe plants a CLAUDE.md, an AGENTS.md, a `.mcp.json`, and a skill in the working directory, and
passes when the session lists only the forum's tools and server, the model names no other tool,
it does not know the canary word, and the client's record of the session shows nothing the client
added from its login (below). No Haiku model takes part (as decided): Claude
Code's small, fast model, Haiku unless set, is pinned to the session's own model
(`ANTHROPIC_DEFAULT_HAIKU_MODEL` and `ANTHROPIC_SMALL_FAST_MODEL`, in the run and in the
interview); the models the client reports using (each message's `model`, the result's
`modelUsage`) are recorded, and any other than the one asked for is an isolation problem (an
interview problem in the interview); and `run` refuses a `claude:` model whose name holds `haiku`.

**What Claude Code adds by itself.** Whatever the switches, Claude Code adds two attachments to a
session from its own claude.ai login. A `session_context` holds the login's email
(`userEmail`), and the client shows it to the model as a system reminder. A `credential_org` holds
the login's organisation id, which the client records but shows the model nothing of. Neither is in
the client's stream; both are in its record of the session. So a run has the client keep its record
of every Claude session (`--session-id`, not only for the interview), the isolation check reads it
after the session (`isolation.claude_account`), and the run then copies it into the run directory
and removes the client's, as the interview does (below). The email is an isolation problem: it is a
ready principal key (`mail:...`, R1). The organisation id is recorded, and would be a problem only
if the model were shown it. A session whose record was not kept is a problem too, since then nothing
says what the model was shown. No CLI option or environment switch keeps the email out while the
client keeps the login. In Claude Code 2.1.287's code, the client adds it whenever it signs in with
a claude.ai login, and leaves it out only when it signs in with an API key or an `apiKeyHelper`
(`--bare` and `CLAUDE_CODE_SIMPLE=1` read no login at all, so they need one too), or when it runs
behind `ANTHROPIC_UNIX_SOCKET`. The probe of `claude:claude-sonnet-5-5` on 2026-10-03, with every
switch above, found the email (`session_context`, shown to the model) and the organisation id
(`credential_org`, not shown). So Claude runs made with a claude.ai login are reported as
not isolated and left out of every count. They can count once the client signs in with an API key
(`ANTHROPIC_API_KEY` in the environment, which the harness passes on to the agents) and a probe
passes with it.

**Codex** (`codex exec --json`, one process per turn, resumed by thread id) runs in an empty
temporary directory with a read-only sandbox, `--ignore-user-config`, `--ignore-rules`, the forum as
its only MCP server (its tools approved), project docs off, web search off, and the shell, apps,
browser, computer use, image, multi-agent, plugin, and skill features disabled. Codex reaches MCP
tools through its code mode (`exec`), which stays on. Three things the harness cannot turn off
without a separate `CODEX_HOME`: the operator's global `AGENTS.md`, the operator's skills list, and
the multi-agent role. Codex also offers its model tools that are not the forum's: `apply_patch`, a
clock tool, and the MCP client's resource tools. So:

- `run` and `campaign` refuse a Codex model unless `CODEX_HOME` names a dedicated home (log in there
  once, with no `AGENTS.md`, skills, or roles) and a probe of that model passed with that home
  (`<out>/probe/probe.json`). `--allow-unisolated` runs it anyway, and its runs are left out of
  every count.
- The probe asks the model for every tool it can call, and every session's rollout is checked for
  the tools a code cell listed or called, against an allow-list: the forum's six tools and code
  mode's `exec` and `wait`. The MCP resource tools (`list_mcp_resources`,
  `list_mcp_resource_templates`, `read_mcp_resource`) are tolerated and reported, since they reach
  only the attached server, the forum. Any other tool, `apply_patch` and the clock tool among them,
  is an isolation problem until the dedicated home's configuration turns it off. The read-only
  sandbox lets the client read the file system, but with the shell off the model has no tool that
  reads files.

Codex sessions are kept in that home's `sessions/`, and indexed in its own databases; the harness
leaves them there. With the operator's own `CODEX_HOME`, each agent thread of a run therefore stays
among the operator's own Codex sessions.

**Adaptations, recorded in every run.** The first smoke run (on 1.4) found three client problems
that kept Claude agents from the forum; spec 1.5 and WP-E5 fixed them in the server, so by default
the harness adapts nothing (`--schema-adapter off`, `--mcp-negotiation client`), and the options
remain only to reproduce 1.4's runs:

1. Claude Code 2.1.287 negotiates MCP `2026-07-28`, and dropped every tool while the forum's
   `tools/list` lacked the `ttlMs` and `cacheScope` that version requires; the server now serves
   them. `--mcp-negotiation legacy` sets `MCP_PROTOCOL_NEGOTIATION=legacy` (2025-11-25 is then
   negotiated). The proxy forwards the `Mcp-Method`, `Mcp-Name`, and `Mcp-Param-*` headers that
   2026-07-28 requires.
2. The Anthropic API refuses a tool whose input schema has a top-level `oneOf`, which 1.4 (R21) gave
   every tool, so Claude Code dropped all six. 1.5 gives each tool one object schema (§20.2).
   `--schema-adapter on` (or `auto`, for Claude agents) makes the proxy flatten a `oneOf` schema
   into one object as 1.5 does: each action's branch expanded into its forms, the actions an enum
   whose description lists the arguments each action and form requires, every argument of every
   form kept, with the union of its schemas (`anyOf` where the forms differ), marked with the
   actions that take it (`*` required, `+` required in some forms). A schema with no top-level
   `oneOf` passes unchanged. The run directory keeps the tools/list as the server sent it
   (`tools-list.server.json`) and as the agents were served it.
3. Claude Code shows its model a tool result's structured content when there is one, not its text.
   1.5's structured content carries the status line and everything else the text has (§20.1).
   `--text-only` still makes the proxy drop the structured content and the output schemas, so that
   the client shows the text.

## The slip counter

After each run it reads the history (`forum-server usability-export`, a read-only subcommand of the
`usability` feature that prints `control export`'s JSON lines for the whole database), the proxy's
log (the server's full response to every call), and the transcripts. `history.py` replays the
history into what the agents left behind, deriving what the core derives without an event (the
offer tag a hold's end adds, an expiry's attempt, a wait's resolution, the flags of §14 that the
history shows, the candidates of `next` in §10.4). Each rule is judged when its "When" says: a
close at its close, a worker at its end, a question while it was pending; what happened later does
not undo a slip. What the harness's fixed points caused is reported apart (`apart` in `slips.json`),
never as a slip. The slip each rule of §20.9 names is counted per run:

| Rule | Counted as a slip |
|---|---|
| E | A call refused, then the same tool and action refused again with the same code for the same argument; a session that ended on a refusal |
| 1 | A line from the agent's own human that no `from_user` entry of that agent holds by the end |
| 2 | A `said` entry never confirmed, unless the session listed it in `unsent` at a release or close; one recorded by the call at which the harness ended the session is apart |
| 3 | A hold that expired, with or without checkpoints, unless the harness ended its session before its checkpoint was due (5 minutes after the issue, last checkpoint, or renewal, or 1 minute before the lease ends); an intent open at the end, charged to the last agent that held its node after it opened (another session's intent left unchecked included) |
| 4 | A `said`, `to_user`, or close written while the node had a request, `said`, `heard`, question, or answer by another session past what the writer had been shown and after its hold was issued, or its conversation node (§9.1) had one past what the writer had been shown |
| 5 | A user entry the agent recorded that was never routed, or routed by another session |
| 6 | A request decided but never answered, judged at an agent's close of its node or else at the end; a `to_user` result naming a request (directly or through its user entry) of another person, aliases being one person; a report of another kind naming a request never answered |
| 7 | A request on a node an agent closed that lacked a decision, or both a decision and an answer, at the close, whoever settled it later; at the end, a request answered but not decided, or neither, on a node an agent held after it was recorded, or one an agent recorded |
| 8 | A series, person, or team closed by an agent; a hold that expired on a finished node (closing it was clean, and its requests were answered, it was `ready_to_close`, the holder had decided, or the holder had closed its intent); a node an agent's take reopened, left open and unheld with none of the flags a close would leave |
| 9 | A node an agent created that is open at the end, was never held, has no `offer` tag, no flag, no wait, and no children, and is not a container, an occurrence, or a signal |
| 10 | Not measured yet |
| 11 | A note, agent question, or answer that reached an agent session (its recipients; a `holder:` note's taker; a `principal:` note, the sessions serving that person; a `holders:` note, the sessions holding in the branch) which made a call after it arrived and before its `until` and never read it; an agent question that reached an agent session and that nobody answered |
| 12 | A question for a human that defaulted or lapsed while pending at a stage addressed to a person an agent session served (by `hello` or by a `from_user` entry it recorded) and was live in, with no `to_user` of kind `question` to that person by that session meanwhile; a digest entry for such a person pending while the session was live and not shown before it ended |
| 13 | A worker (no principal) that ended on its own, holding nothing, while the first node of the last `leave_better` it was given (by a close, a release, or an empty `next`) was open and free, or the node its create joined was free, or `next` would have offered it an item: the server's own answer when the last call was an empty `next` or a `hello`, otherwise the counter's model of §10.4 at that moment (waits, dependencies, needs and caps, horizon, pauses, and refusals excluded) |

Also per run: refusals by code, and apart from them the refusals by commands of a core package the
server does not have yet (`packages.py`: the placeholder's hint is shared with real `BadParams`
refusals, so the command decides); `late` entries; the warnings, with `unsettled_requests` and
`open_commitments` named; the share of responses whose first `now` next call the agent's next call
followed (same tool and action); forum calls, tokens, and cost; and the misreadings the 1.4 texts
address that a call shows: R1 as an invented principal (a key that neither `user` nor any human of
the scenario has, in any argument of any call, a refused one included, counted once per agent and
key) and as a thread's message logged as `from_user`, R5, R20, R21, and session ids guessed. The
refusal of an invented principal is R1, not R20: in R3 a worker passed `user:<a name>`, refused for
its scheme, and then another made-up key, refused only because `hello {from_user}` was not built.
R6, R24, and the rest of R1 need a reader.

## The interview

After a run is counted, each of its agent sessions is resumed once in its own client and asked a
fixed debrief (`harness/interview.py`). It comes after the counting and with the server stopped, so
it cannot change what was counted: the resumed client has no MCP server and no tool, and what it
says reaches neither the history, nor the proxy's log, nor the transcripts the counter reads.

- **Claude Code**: `claude -p --resume <session id>` in the session's own working directory, made
  again empty, with the run's isolation (the same environment switches, no settings source, the
  built-in plugins off, no slash command, the role appended), no MCP server (`--strict-mcp-config`
  without `--mcp-config`), no built-in tool (`--tools ""`), the forum's tools denied as well, and
  `--no-session-persistence`, so that the session the client kept ends where the run ended it. A
  Claude session can be resumed only if the client kept it: a run passes `--session-id` instead of
  `--no-session-persistence` (runs made before R3's fixes did so only when chosen for the interview;
  every run does now, for the isolation check above), which changes nothing the model sees, and the
  client keeps the session under its own projects directory (`~/.claude/projects`, or under
  `CLAUDE_CONFIG_DIR`), in a directory named after the temporary working directory. After the
  interview, or as a run not interviewed finishes, the harness copies the session into the run
  directory (`agents/<id>.session.jsonl`, and the session's own directory, if the client made one,
  as `agents/<id>.session/`) and removes that directory, so that a run leaves nothing in the
  client's directory. A guard refuses, touching nothing, unless the path is the one derived from the
  run's own temporary working directory of that agent (`forum-agent-<pid>-<agent>-<time>` directly
  under `TMPDIR`, ASCII digits only), the session id is the one the harness chose, and the directory
  holds nothing but that session's files as the client writes them (`<id>.jsonl` a regular file,
  `<id>/` a directory, neither a symlink); the refusal is recorded. Nothing is removed unless every
  entry was copied and the copy checked against it. A later `run.sh interview --again` puts the copy
  back for the client before it resumes the session, and removes it again after. The interview has a
  budget (`--max-budget-usd`): 0.50 USD beyond what the session had cost, since the cost the client
  reports for a resumed session includes the run's turns. One turn with no tool is one request,
  which the client lets finish, so the budget bounds anything beyond it; a session that reaches it
  is recorded (`over_budget`), its answer kept.
- **Whatever ends a run**, it leaves nothing in the client's directory. Before a Claude client of a
  run starts, the run records its session (`agents/<id>.client-session.json`: the working directory,
  the session id, the harness's process). A run that is not interviewed (it was not chosen, failed,
  or was interrupted, or the interview failed) copies and removes its recorded sessions as an
  interview does, whatever failed before; so does the probe; a run directory that a new run replaces
  is tidied first; and `run`, `campaign`, and `interview` start by tidying every run directory whose
  records no finished run took care of (a run whose process was killed), unless the harness process
  that made them still runs.
- **Codex**: `codex exec fork <thread id> --ephemeral` with the run's flags, no MCP server, code
  mode's host off (`--disable code_mode_host`: code mode then fails closed, and the tools nested in
  it, `apply_patch` and the clock tool, are gone), and a read-only sandbox. The fork sees the
  thread's history, and Codex keeps no rollout of it: the thread's rollout, which the isolation
  check reads, stays as the run left it, no rollout of the fork is written, and the home's session
  index (`session_index.jsonl`) does not name the fork. Each interview checks these three and
  records anything else in `interview.json`, leaving it alone. The Codex home also keeps threads in
  databases of its own (`*.sqlite`), which the harness neither reads nor touches, so it cannot say
  that they hold nothing of the fork. The harness first used `exec resume --ephemeral`, which wrote
  the interview's turn into the thread's rollout and thread history: Codex 0.159.2 honours
  `--ephemeral` only for a new thread. The tools are as far off as the CLI allows, not all off:
  Codex reports no list of the tools it offers, and asked in a fork with these flags, the model
  named code mode's `exec` and `wait` (which fail closed), `request_user_input`, and, with the
  operator's own `CODEX_HOME`, the multi-agent tools. A tool the model calls is recorded. A fork
  reports the token usage of the whole thread, the run's turns included: the interview's own is
  what it adds to the thread's last total in the rollout (both are recorded). Codex runs with the
  operator's own `CODEX_HOME` (`--allow-unisolated`) are interviewed too; the report marks them as
  not isolated.

One turn per agent session, the questions in one message, answered under their numbers, at most
three sentences each. The questions are few and neutral, in plain words: they do not name the
protocol's rules, hint at what the agent missed or was meant to do, or presuppose that something
went wrong, and the preamble asks the agent to say so where a question does not apply. They are
those of the Wizard-of-Oz rounds where those fit (what was convenient, what was confusing, what the
agent would change); the rounds' rating out of 10 is left out, since their reports found that
self-ratings did not track outcomes. Both arms are asked the same questions, so that their answers
compare (the guidance under test is not named); a test pins the exact words.

1. What were you trying to do in this session, and how far did you get?
2. What about the tools was convenient or easy to use?
3. What was confusing or slow?
4. Was anything unclear about what to pass to a tool or what a response meant?
5. After each tool response, how did you decide what to do next?
6. What would you change about the tools?
7. Only for a session the harness ended mid-task (a fixed point, the call limit, a turn's timeout,
   or the run's time limit): The platform stopped your session before you finished. What would you
   have done next?

**What the resumed model knows.** A client writes its record of a session behind the session, so
a session the harness killed mid-task can lose its last calls from it: in the live check below, a
Claude session cut after 3 forum calls kept the result of 1, a Codex thread cut after 3 kept 2, and
both models answered as if they had stopped there. In round R3 all four Claude sessions cut after
3 calls kept 1 or 2, and each blamed itself for a stop it had not made. Question 7 therefore asserts
nothing about what the session last received, and the interview counts the forum calls whose
results the client kept (`calls_kept`: the `mcp__forum__` tool results in the Claude session file,
the forum calls of code cells with an output in the Codex rollout), which the report sets against
the calls the run made. A session the harness stopped right after a forum call (a fixed point, the
call limit) is told, after the preamble and from the proxy's log, how many calls it made and which,
the last 10 at most: "The platform stopped your session after your 3rd forum call (session hello,
hold take, view node); your own record may lack the last results." It names what the session did,
not what the calls returned. A session stopped by a timeout is not told, since it was stopped at no
particular call. Where the client kept fewer calls than the session made, the report marks the
answers about what it did (questions 1 and 5) as unreliable. The harness does not wait for the
client to write before it kills a session at a fixed point: the client would meanwhile send its
next request.

`interview.json` in the run directory holds, per agent session, the model, scenario, arm, and run,
the client's session id, whether the harness ended it mid-task (or stopped it between its turns,
`stopped_by`), the forum calls the run made and those the client kept, the note on a session stopped
after a call (`note`, and `note_from`: the proxy's log, or the transcript in a run without one), the
questions as asked, the whole reply and the answers by question, the tokens and the cost (Claude:
the cost the interview added, its budget, and whether it reached it; Codex: the interview's own
tokens, the thread's total the fork reported, and the total before it), what tools the resumed
client offered (Claude: none is expected, and anything else is recorded; Codex reports none), the
models the Claude client used, and what the client kept (`client_files`: for Claude, the copy and
the removal or the guard's refusal; for Codex, whether the thread's rollout is unchanged, and
whether a rollout or a session index entry of the fork exists). A session that cannot be resumed
(the client did not keep it, or no longer finds it) is recorded as not resumed, with the client's
own words. The resumed client's raw stream is `agents/<id>.interview.raw.jsonl`. `--interview none`
turns it off, and `run.sh interview --out DIR` interviews saved runs later (a Claude session of a
run made before R3's fixes only if its run was chosen for the interview, since only then did the
client keep it).

**Cost.** In the live check of s0-smoke on 2026-10-03, the interview of a claude-sonnet-5-5 session
cost 0.018 and 0.033 USD, most of it writing the resumed session into the prompt cache (0.007 and
0.010 USD when asked again minutes later). Codex reports tokens but no cost: a gpt-6.1-sol fork
reported 70,290 and 315,530 input tokens and 419 and 1,422 output, but 49,484 and 290,086 input and
152 and 1,202 output of them were the run's (the thread's total before the fork), so the interviews'
own were 20,806 and 25,444 input (6,912 and 9,216 of them cached) and 267 and 220 output. A
campaign interviews, by default, 4 models × 21 agent sessions (over its 10 scenarios) × 2 arms × 3
runs = 504 sessions, 252 of them Claude's. A campaign session is longer than a smoke one (up to 10
turns), and an interview reads its whole context once, so its cost grows with it: at the smoke
check's 0.01 to 0.03 USD it would be 3 to 8 USD for the Claude interviews, and the budget bounds them
at 252 × 0.50 = 126 USD. The cost of the Codex interviews is unknown; the report shows their tokens
per model.

## The report

`report.md` in the output directory, from the counted runs: those the counter read whose client
loaded nothing besides the forum (a run with isolation problems is listed and left out). Its header
names the protocol's version as `docs/protocol.md` states it, and the versions the runs were made on
when they differ. Per model, scenario, arm, and rule, the slips per run with a 95 % interval (the
exact Poisson interval for counts, which no slip in 10 runs puts at [0.00, 0.37], never a point);
the next calls followed (Wilson interval); calls and tokens per task (tokens with a percentile
bootstrap); refusals, late entries, and warnings; the misreadings per model in the arm with guidance
against the Wizard-of-Oz round, with a verdict per text (it falls when the upper end of the live
share's interval is below the round's share, else it goes back for a decision); and the pass
criteria of the WP-U row: every cell the manifest asked for reported with at least 10 counted runs
(missing cells and S8's arm without orientation listed), at least 3 models from 2 vendors, fewer
slips with guidance for every model as a sum of per-scenario means over the scenarios both arms ran,
and the rules NC-10 sends back for a decision. Then the interviews: how each vendor's sessions were
resumed, what agents said, per model and arm, each question with up to three answers quoted, cut to
240 characters (the answers to earlier words of a question only per session), with the Claude
interviews' cost and the Codex interviews' tokens, their cost unknown; and per scenario and arm,
each interviewed session's slips, what was reported apart, the misreadings its calls show, its
refused calls, who ended it and how many of its calls the client kept, the note it was told, and
anything the interview did not do as meant (a tool, another model, the budget reached, the client's
files), followed by its answers, cut to 700 characters. An answer about what the session did
(questions 1 and 5, or an unnumbered reply) is marked unreliable, in both places, when the client
kept fewer calls than the session made. The answers are quoted, never summarised. An interview
counts nothing: the answers help a reader with what the counter cannot judge (R1, R6, R24), and the
interviews of runs with isolation problems are shown too, marked. `docs/verification.md` links the
campaign's report (WP-F3).
