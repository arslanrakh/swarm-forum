# The final usability round

The round before the first release (WP-U, design D.12.10), run on 2026-10-04 at commit 8f8098b, with
every core package merged: the next calls of §20.9 were live in the guided arm. It replaces the
campaign's planned ten counted runs per cell, which was cut to one round of one run per cell.

**What ran.** 9 scenarios (every campaign scenario that was ready; `s8-newcomer` needs an orientation
switch that does not exist) × 4 models × 2 arms (`g`, with next calls; `nog`, without) × 1 run:
72 runs, 154 agent sessions, every one interviewed afterwards. The models: Claude Opus 5.5 and Claude
Sonnet 5.5 through the `claude` CLI, and GPT-6.1 Sol and GPT-6 Astra through the `codex` CLI.
1,684 forum calls and 181 refusals. Claude cost 25.58 USD (17.28 for the runs, 8.30 for the
interviews); Codex reports tokens only.

**What the numbers are worth.** The harness counted none of the runs, as it should: every run had an
isolation problem. The Claude CLI signed in with a claude.ai login shows the login's email address
to the model, and two sessions without guidance used it as a principal; the Codex runs used an
unisolated Codex home, which was accepted and which the report marks as not isolated. Every
figure below was recomputed from each run's slips, calls, and transcripts, one run per cell, so they
are observations, not measurements: the intervals overlap everywhere. The runs' databases, which
hold that address, are kept out of the repository.

**Outcome.** Guidance did not lower slips overall (72 with, 82 without; 62 and 67 once harness
artefacts are left out), but it clearly helped routing (rule 5: 1 against 7) and replying (rule 6:
3 against 6), and it stopped the invented principal. Agents made the first `now` next call in 29 %
of the responses that had one. The serious problems in what agents see were fixed before the
release, specification first; the harness's own problems are left for the next round:

| Finding | Serious | Status at the release |
|---|---|---|
| F1. Codex's code mode hides the forum's tools | yes | README's "Connecting a client" says so (2c36609); the harness still runs Codex in code mode: open for the next round |
| F2. Record before sending, confirm after sending | yes | fixed (2c36609): an entry listed in `unsent` becomes unconfirmed at once, so rule 2 no longer asks to confirm it; rule 2 says to close or release first when the reply is what sends the message; `confirmed` is a link or how the message was sent. "Finish before your last message" does not fit in the instructions (509 of 512 characters) |
| F3. Health red at once, and the order to fix it first | yes | fixed: with no `next` call the idle share is unknown (46fb439); `Unreachable` and `Overloaded` are yellow until their oldest item has waited `capacity.window`, and the constitution says "if you can; otherwise go on" (2c36609) |
| F4. A worker does not report on another principal's request | yes | fixed (2c36609): the `talk` and `principal` texts and the hint say a `to_user` may report to the requester of a request you finished, who reads it on their board |
| F5. The report counts nothing; the login's address reached the forum | yes, for the measurement | open: a Claude login without a personal address, and a dedicated Codex home |
| F6. Slip-counter artefacts | yes, for the measurement | open (harness) |
| F7. Scenario world and timing | yes, for the measurement | open (harness) |
| F8 to F15 | no | open, for later rounds |

None of the fixes has been confirmed by a new round yet; the next round re-runs the scenarios that
showed them (s3, s5, s32, s1).

The findings follow as the round's analysis wrote them.

## Findings

**Data.** 72 runs: 4 models × 9 scenarios × 2 arms × 1 run. 154 sessions (78 Claude, 76 Codex). 1,684 forum calls (842 per arm), 181 refusals (Claude 29, Codex 152). Claude run cost 17.27 USD; interviews 8.30 USD.

The runner's report counted none of these runs. Every figure below is recomputed from each run's `slips.json`, `run.json`, `proxy.jsonl` and transcripts, and should be read as an observation.

**"Adjusted"** means slips of Codex sessions that never called the forum, and slips of sessions cut off by `max_turns` with an `ACTION` still unanswered, are left out.

### Arms compared

| Model | Raw slips g/nog | Adjusted g/nog | Refusals g/nog | Calls g/nog | First `now` item followed (g) |
|---|---|---|---|---|---|
| opus | 21/13 | 18/8 | 7/6 | 260/277 | 30/94 |
| sonnet | 19/28 | 19/28 | 5/11 | 148/161 | 20/57 |
| gpt-6-astra | 16/22 | 14/18 | 33/34 | 206/159 | 21/89 |
| gpt-6.1-sol | 16/19 | 11/13 | 33/52 | 228/245 | 24/93 |
| **All** | **72/82** | **62/67** | **78/103** | **842/842** | **95/333 (29 %)** |

Adjusted slips by rule, g/nog, all models:

| Rule | g/nog | Effect of guidance |
|---|---|---|
| E | 2/3 | All 5 are batching artefacts (F6) |
| 1 | 4/3 | Not lower for any model |
| 2 | 4/4 | Flat |
| 3 | 5/5 | Flat |
| 4, 9 | 0/0 | Never occurred |
| 5 | 1/7 | Helps |
| 6 | 3/6 | Helps |
| 7 | 17/16 | Flat |
| 8 | 6/2 | Not lower for any model |
| 10 | — | Not measured |
| 11 | 0/1 | — |
| 12 | 5/4 | Not lower for any model |
| 13 | 15/16 | Flat |

- **Invented principal (R1 text).** No session in g did it; in nog 2 sonnet sessions did, using the leaked login address.
- **Opus is worse with guidance** because of s32: four support agents left the same `leave_better` series node untouched (rule 13). That is scenario-path noise: with n = 1, 18 and 8 have overlapping 95 % intervals.
- **s5 inverts the outcome.** g S2 reached the takeover and was charged 4 slips. nog S2 never saw #12, because S1's hold was still live, so it took the wrong node and was charged nothing (F7).

Verdict: no measurable reduction in slips overall. Guidance clearly helps routing and replying, and stops the invented principal. Interviews show next_calls are noticed and valued: 37 of 39 Claude g sessions mention them.

### Findings, ranked

#### F1. Half of the Codex sessions never used the forum (code mode hides its tools)
**Serious: yes. Spec change: no (harness, plus a README note). Category: harness.**

- **What happened.**
  - 35 of 76 Codex sessions made 0 forum calls: s2 8/8, s32 14/32, s5 5/8, s6 4/4, s3 A 3/4, s7 1/4. Claude: 0 of 78.
  - Codex 0.159 runs MCP tools in code mode. The only visible tool is `exec`; the forum's tools sit in `ALL_TOOLS` / `tools.mcp__forum__*`.
  - The first request is about 10.5k input tokens in every Codex session. The served tools list is 68 KB, so neither the forum's schemas nor its instructions are in context.
  - All 41 forum-using Codex sessions began with a discovery script such as `ALL_TOOLS.filter(x=>/forum/i.test(...))`, usually only after the platform's default "No result".
- **Evidence.**
  - `s2-person-thread/codex-gpt-6-astra/g/run-01/proxy.jsonl` is 0 bytes. R1 ran 7 turns of `ACTION:` and `POST:` lines only.
  - In `s7-research-basis/codex-gpt-6-astra/g`, P's turn 1 was `ACTION: Read the swarm forum's current programme status and recent claims…`. Its first forum call came in turn 5.
  - s32 astra g S3 left this in its checkpoint learnings: "Forum tools work directly; platform ACTION rea…".
  - 57 of 63 Codex `POST:` lines came from sessions that recorded nothing.
  - `isolation.tools_listed` is `[]` for every Codex session: the probe does not see code-mode tools.
  - These sessions are nearly slip-free (s2 Codex: 0 slips), so the counter rewards absence.
- **Smallest fix.**
  - Harness: run Codex agents with code mode off, so the forum's tools are direct functions. The harness already disables `code_mode_host` for interviews. Have the probe assert that the forum's tools are in the first request.
  - Counter: count a session that never called the forum, and a `POST:` without a `said`.
  - README ("Connecting Codex"): code mode hides the forum. Turn it off, or put "call the swarm forum's session hello first (tools.mcp__forum__session)" in the agent's instructions.
  - Until then, the Codex arms compare next_calls only.

#### F2. Record before sending, confirm after sending: no fit when the post is the agent's last reply
**Serious: yes. Spec change: yes (§9.2, §25 `said` states, §20.2 `confirmed`, §20.9 rule 2's why, §22 instructions). Category: protocol and text.**

- **What happened.**
  - 34 `said` records were followed by 6 confirm calls, all with improvised text instead of a link, for example "chat:support-ops/T1001 (posted via POST line)".
  - Only 1 confirm came after its post. 2 came in the same turn before the post was written. 3 confirmed posts that were never written: sonnet s2 g R1 `e6` and sonnet s32 nog R1 `e9` ×2.
  - 9 entries were listed in `unsent`, some of them actually posted.
  - The rule-2 "confirm" item was followed 3 of 24 times; 8 times the session simply ended.
- **Cause 1: the post is the end of the turn.**
  - `s5-takeover/claude-claude-opus-5-5/g` S2, proxy #10: `talk said {at:#12, where: issue:platform#88, kind: result}`.
  - Its next_calls were "1. talk said {said_id: e8, confirmed: ?} — after you send it…" and "2. node close {settles: [e1]…}".
  - Turn 3 ends `POST: Full jitter is now added…`, and no further turn comes. Result: rule 2 (e8), rule 3 (hold 12.3 expired), rule 7 (answered, no decision) and rule 8 (finished node left held).
  - The same 4 slips hit sonnet g and sol g S2, plus sonnet R1 in s2 nog and s32 g.
  - §9.2 already lets a close or release confirm pending entries, but nothing tells the agent to finish first.
- **Cause 2: `unsent` has no terminal state.**
  - §25 says `pending → confirmed | unconfirmed; unconfirmed → confirmed`.
  - An entry listed as unsent stays pending, turns unconfirmed when the session goes silent, and sets `unconfirmed_said`. Closed nodes carrying that flag are re-offered at tier 0.
  - `s32-example/claude-claude-opus-5-5/g` R1, proxy #36: `hold release {unsent: [e15, e17]}`. The very next next_calls still say "talk said {said_id: e15, confirmed: ?} — after you send it: confirm; if you do not, list it in unsent when you release or close".
  - Later S7 was offered #12 with "closed, flags: unconfirmed_said".
- **Agents' words.**
  - opus s2 g R2: "I wasn't sure how to handle 'confirm the said after you send it' when the send happens only in my reply text, after my last tool call."
  - sonnet s2 g R1: "The `confirmed` field also invites marking something sent when it hasn't been."
  - opus s32 g S6: "A closed item got reoffered because of a flag that can never be cleared, and that cost a whole take, read and close cycle."
  - opus s32 g R1: "I wasn't sure whether `talk said` with `kind: reply` should automatically mark the request as answered."
- **Smallest fix.**
  - §25 and §9.2: `unsent` at release or close gives the entry a terminal state `unsent`, with no flag. Anyone who checked the conversation may resolve an unconfirmed entry either way: `confirmed`, or not sent.
  - §20.2: `confirmed` becomes "a link to the message as sent, or how you sent it when you have no link".
  - §20.9 rule 2's why: "after you send it: confirm; if your reply is what sends it, close or release first, which confirms it". Drop entries listed in `unsent`.
  - §22: add "finish before your last message" next to "Calls record; you send", within `instructions.max`.
  - Harness (optional): reply to a `POST:` with the message's key whenever the turn continues.

#### F3. Health is red in every run, and the constitution orders agents to fix it first
**Serious: yes. Spec change: yes (§17 Overloaded and Unreachable thresholds, §22 constitution line 9). Category: protocol.**

- **What happened.**
  - 117 of 119 forum-using sessions saw `health red`: 1,273 status lines read "overloaded" and 228 read "unreachable 1 (needs)".
  - Agents made 137 `control health` calls and raised 6 objections about health.
  - 97 of 154 interviews mention it (Claude 70 of 78).
  - Among next_calls followed, `control health` displaced the `hold next` item 17 times and the route item 19 times.
- **Cause.**
  - `Overloaded` turns red as soon as demand exceeds supply while the idle share is below 20 %. A young or small forum's first `next` that finds work gives an idle share of 0 and supply of 0.
  - `Unreachable` turns red from 1 % of candidates, which is 1 node in a small forum.
  - The design must hold for one user, and it does not here.
- **Effects.**
  - s5 opus nog S2 took the wrong node: "Took #11 (health red: overloaded, taking work is the fix)".
  - Chat assistants passed it on to the human (s1).
  - Agents' words, opus s1 nog U: "I couldn't see how a chat assistant could add worker supply". opus s9 nog W: "I had to decide for myself that finishing work counted as the fix."
- **Smallest fix.**
  - §17: `Overloaded` and `Unreachable` start yellow, as `NoMover` does, and turn red only when their oldest item has waited `capacity.window`, or past a count floor.
  - §22 line 9: "…read control health and fix its first reason if you can; otherwise go on."
  - Server: each health reason names who can act on it.

#### F4. A worker does not answer another principal's request
**Serious: yes. Spec change: yes (§20.2 `talk` text and the `to_user` hints, §20.9 rule 7). Category: text and protocol.**

- **What happened.** In s3, Bob's request (`slack:U0BOB`, recorded by a Slack session that is gone) was left unsettled in 6 of 8 B sessions, in both arms.
- **Evidence.**
  - `s3-company/codex-gpt-6-astra/g` B: #20 `decide {settles: [e17]}`. Its next_calls said "talk to_user {principal: slack:U0BOB, kind: result, re: [e17]} — decided, not yet answered". The agent ignored it 3 times, sent `say` to the gone session s-3 (#21, #25), and released with "Chat assistant should deliver…".
  - sonnet g B (#19): `said {where: "user:e16", kind: result}`, then a release with `unsent: [e32]`.
  - opus g B: `decide` without `settles`, so no next call followed.
  - sol g/nog B did send `to_user` after refusals, then judged it premature: "Recording a response also settled Bob's request before delivery succeeded, requiring a retraction."
  - opus nog B: "It was also unclear whether a non-holder of the conversation may close a node that settles a person's request, so I handed that off."
- **Cause.**
  - §20.2 says "to_user records what you tell your human… tell them yourself", and the refusal hint says "to_user tells your human".
  - §8.1, however, says a `to_user` is the swarm's recorded report, read on the request board.
  - When a node with an unsettled request is taken, no rule suggests a decision until something is recorded.
- **Smallest fix.**
  - §20.2, the `principal` description and the hint: "to_user records a report to a human: your own, whom you then tell, or the requester of a user request you finished, who reads it on their board: you need not reach them".
  - §20.9 rule 7: at take or next of a node with unsettled requests, add a `later` item, `talk decide {settles}` once done.
  - Server: warn on a `said` whose `where` is a `user:` key.

#### F5. The report counts nothing, and the isolation leak did reach the forum
**Serious: yes, for the measurement. Spec change: no. Category: harness.**

- **What happened.**
  - Every run was excluded. Claude's `session_context` carries `userEmail`; Codex runs in an unisolated Codex home, which was accepted.
  - Pass criteria, intervals and misreading verdicts are therefore all "not measured".
  - sonnet nog s32 U1 and U2 passed the login address as `principal` on call 1.
- **Smallest fix.**
  - A report option that counts runs whose only problems are the accepted ones, marked as such.
  - Run Claude without `userEmail` in context: an account or API key with no personal address, checked by the probe.
  - Keep the run databases, which hold the address, out of any commit.

#### F6. Slip-counter artefacts
**Serious: yes, for the measurement. Spec change: no. Category: harness (`slips.py`, `report.py`).**

- **Rule E.** All 5 "same mistake again" calls were issued before the refusal could be read:
  - the 4 Codex calls 3–5 ms after the refused call, in the same `exec` script;
  - the 1 Claude call (sonnet nog s3 A) about 0.8 s later, as a parallel call.
- **`max_turns` cut-offs.** Sessions stopped with an `ACTION` unanswered were charged rules 3, 6, 7 and 12: opus s3 A in both arms (8 turns), sol s4 W, sol s7 P.
- **Rule 8 false positive.** "Reopened by a take, left open" was charged in s4 (opus g, astra g/nog). Opus had just superseded the basis, so the work really was unfinished.
- **R1_principal false positive.** The flag fired on Codex B's `to_user {principal: slack:U0BOB}`, which is the call the server itself suggested.
- **Missing counts.** Zero-call sessions count no slips, and unrecorded `POST:` lines are never counted.
- **Scenario criteria are not evaluated.** s5 nog had fewer slips while failing the takeover.
- **Smallest fix.** Ignore a repeat that was issued before the previous response could be read. Treat `max_turns` with an unanswered `ACTION` as an end point, reported apart. Skip rule 8 when the session's own release says work remains. Apply R1 only to hello and from_user binding. Add counts for "never used the forum" and "POST without said". Add the scenario criteria to the report.

#### F7. Scenario world and timing
**Serious: yes (it misled every model in s2, s32 and s5). Spec change: no. Category: harness.**

- **What happened.**
  - **Fixed dates.** The server's `now` is 2026-10-04, so "yesterday" is 10-03, but the world data describe the 10-01 and 10-02 runs. This led to 47 interview complaints and to interim and corrected posts.
  - **Default replies.** 125 of 340 world replies (37 %) were the default "No result: the platform could not do this action", which drove blocked releases.
  - **s5 timing.** S2 starts after a fixed 4 minutes instead of after S1's lease has ended, as the scenario itself requires. In nog, #12 was not offered to S2 (its offers listed only #11).
- **Agent's words.** opus s2 g R1: "The platform sometimes answered a different question than I asked, giving the 10-01 run when I asked for 10-03."
- **Smallest fix.** Dates relative to the run's clock; S2 starts on S1's lease end; world rules for common reads (thread, file contents, `gh pr list`).

#### F8. Calls without `session` silently open stray sessions (Claude)
**Serious: no (moderate). Spec change: yes (§3, §20.1, §21). Category: protocol.**

- **What happened.**
  - 21 calls in 17 Claude sessions, opus included this time. Mostly parallel reads issued alongside `hello`.
  - 2 were writes: opus g s2 R2 `node create`, which put a hold in the stray s-6, and sonnet nog s5 S1 `hold take`, which put a hold in the stray s-4.
  - 15 interviews mention it. opus nog s2 R1: "Calls without a session should fail instead of opening a new one."
- **Smallest fix.** A non-hello call without `session`, on a connection with no MCP session, is refused as `invalid` with the hint "call session hello first, then pass session".

#### F9. Codex argument refusals (R21 continues)
**Serious: no (recovered; its rule-E slips are artefacts). Spec change: optional (§10 / §20.5 `node update`; the `verbatim` default). Category: protocol and text.**

- **What happened.**
  - Codex accounts for 152 of 181 refusals. The leading ones:

    | Refused call | Count |
    |---|---|
    | `node update` without `version`/`digest_version` | 27 |
    | `to_user` with `at` | 12 |
    | `to_user` without `verbatim` | 9 |
    | `view agreements` with `path` | 7 |
    | `say` with `evidence` | 6 |
    | `route` with `node`, or without `reason` | 10 |

  - Code mode renders each tool as a flat TypeScript declaration with every argument optional.
  - astra nog s1 U: "the shared tool schema included arguments that a particular action did not accept."
- **Smallest fix.** Do not require `version` for a digest-only patch, since `digest_version` already guards it (O2), as R3 proposed. Default `verbatim` to true.

#### F10. Rule 13: tidying and more work ignored (15/16)
**Serious: no. Spec change: minor (§20.9 rule 13's why). Category: text and harness.**

- **What happened.**
  - 31 slips in total; 11 belong to sessions started for a single message (s2 R, s32 R/S).
  - The "tidy" and "more_work" items were followed 11 of 54 times; `control health` took their place 17 times.
  - Some suggested nodes need capabilities the session lacks. opus s3 g B: "have next_calls skip work I can't do, like git tasks for a SQL-only session."
- **Smallest fix.** Give rule 13 after close or release the same "unless you came for a message or an event" that it has at hello, and do not charge message-started sessions.

#### F11. Human items: rule 12 and `re`
**Serious: no. Spec change: small (§8.4 / §20.7 `re`). Category: server and text.**

- **What happened.**
  - sonnet g s3 A received the rule-12 item `to_user {re: [e12]}` in 14 consecutive responses and recorded no `to_user` at all: "kept prompting me about a digest question to Bob that Alice had effectively answered, and I couldn't clear it."
  - 6 refusals came from `re` naming user questions (e4, e6, e29, e35).
  - Codex s32 U1's questions lapsed, because the user's chat answers were not recorded as `answers`.
- **Smallest fix.** Accept question ids in `re` for a question-kind `to_user`. Rule 12's why gives the entry's gist.

#### F12. Response size
**Serious: no. Spec change: yes (§20.4: "view full is the whole brief plus extras"). Category: server.**

- **What happened.**
  - `view node full` accounts for 173 calls but 44 % of all response bytes (largest 38 KB).
  - In it, `brief.sections` (15.9 KB) and `feed` (15.3 KB) repeat the same entries.
  - `tools/list` is 68 KB. 79 interviews call the responses long or repetitive.
- **Smallest fix.** In the full view, the feed omits the entries the sections already show.

#### F13. Short leases in params-short
**Serious: no. Spec change: no (an optional warning in §21). Category: harness.**

- **What happened.**
  - `lease.first` is 2 min, so a checkpoint falls due 1 minute after a take. A requested lease is silently ignored.
  - 16 complaints. opus s4 nog W: "asked for a 15-minute lease but got 2 minutes, which I only noticed in the status line."
- **Smallest fix.** `lease.first` of at least 5 min, or the take response says that the first lease is `lease.first`.

#### F14. Trailing human messages not logged (rule 1, Claude 3/2)
**Serious: no. Category: none needed.**

- Mostly "thanks" messages. One was substantive and missed: sonnet s6 g A, "Can't you schedule it yourself? The platform has a scheduler, I think."
- No change needed; the counter could weight thanks lower.

#### F15. Series occurrences and time zones (s1)
**Serious: no. Category: server text.**

- What happened: an occurrence shows the old body until its slot; the status line is in UTC while the cron runs in Lisbon time.
- opus s1 g U: "I got the first-run date wrong and told Dana 'tomorrow'… nothing showed me the next occurrence."
- Smallest fix: the occurrence's brief says "copied from the series at its slot <local time>", and the create response shows the next slot in the series' time zone.
