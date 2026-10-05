"""One run: a scenario, a model, and an arm, on a fresh server.

The run starts the server and the proxy, plays the prelude, starts each agent when its scenario
says, delivers the agents' lines, the humans' and the people's replies, and the world's answers,
ends sessions at the scenario's fixed points, lets the server run for the scenario's drain time,
stops it, reads its history, and counts the slips. A run chosen for the interview then resumes each
agent session once for its debrief (interview.py), after the counting, so that it cannot change it.
Everything lands in the run directory:

    run.json             what ran, when, and with which settings; the deliveries; any failure
    server/              forum.toml, server.log, export.jsonl (the history), forum.db
    proxy.jsonl          every tools/call with the server's full response, by agent
    tools-list.*.json    the tools/list as the server sent it, and as the agents were served it
    agents/<id>.*        the client's raw stream, its stderr, and the normalised transcript
    slips.json           the slip counter's result
    interview.json       the interview's questions and answers per agent session, and its cost
    agents/<id>.interview.*  the resumed client's raw stream and stderr
    agents/<id>.client-session.json  the Claude session the client keeps, recorded before the
                         client starts
    agents/<id>.session.jsonl  a Claude session as its client kept it, copied after the interview
                         or, in a run not interviewed, as the run finishes; the client's own copy
                         is removed

A Claude client keeps its record of every session of the run, which the isolation check reads
(isolation.claude_account) and the interview resumes. The run leaves nothing in the client's
directory however it ends: `finish` tidies the recorded sessions whatever fails before it ends, an
interruption (`interrupt`, or a KeyboardInterrupt in the thread that runs `execute`) goes through
`finish` too, and a run that could not (the process was killed) is tidied when the harness next
starts (interview.tidy_orphans).
"""

from __future__ import annotations

import json
import re
import shutil
import threading
import time
import traceback
from pathlib import Path

from . import agents as A
from . import interview as IV
from . import isolation as ISO
from . import mcp as M
from . import packages as PK
from . import params as P
from . import proxy as X
from . import scenario as SC
from . import server as S
from . import sim as SIM
from . import slips as SL


VAR = re.compile(r"\$([a-z][a-z0-9_]*)")

# What a Codex agent of the lab is told before its role: the forum server's own instructions,
# which a Claude client shows its model and Codex shows none (what a future system prompt would
# carry).
CODEX_INSTRUCTIONS = """Instructions from the MCP server "forum" (its tools are `mcp__forum.*`):

<server_instructions>
{instructions}
</server_instructions>

{role}"""


def _resolve(value, env: dict):
    """`$name` replaced by a saved value: the whole string keeps the value's type, and a `$name`
    inside a longer string (`user:$e140`) is replaced by its text."""
    if isinstance(value, str) and value.startswith("$") and value[1:] in env:
        return env[value[1:]]
    if isinstance(value, str) and "$" in value:
        return VAR.sub(lambda m: str(env[m.group(1)]) if m.group(1) in env else m.group(0), value)
    if isinstance(value, dict):
        return {k: _resolve(v, env) for k, v in value.items()}
    if isinstance(value, list):
        return [_resolve(v, env) for v in value]
    return value


def _dig(data, path: str):
    for part in path.split("."):
        if isinstance(data, dict):
            data = data.get(part)
        elif isinstance(data, list) and part.isdigit() and int(part) < len(data):
            data = data[int(part)]
        else:
            return None
    return data


def segment_lists(text: str) -> dict[str, list[str]]:
    """As `segments`, but each `POST:` or `ACTION:` is one item, with the fenced block (```) that follows
    it, if any: a patch or a script written under an ACTION line belongs to that action."""
    out: dict[str, list[str]] = {"post": [], "action": [], "reply": []}
    lines = (text or "").splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        s = line.strip()
        head = s.split(":", 1)[0].strip("*` ").upper()
        if head in ("POST", "ACTION") and ":" in s:
            item = [s.split(":", 1)[1].strip()]
            j = i + 1
            while j < len(lines) and not lines[j].strip():
                j += 1
            if j < len(lines) and lines[j].strip().startswith("```"):
                k = j + 1
                while k < len(lines) and not lines[k].strip().startswith("```"):
                    k += 1
                item += lines[j : k + 1]
                i = k
            elif j == i + 1:
                # The lines right under the ACTION line, up to a blank line or the next POST/ACTION, belong
                # to it (a PR body, a file's new content, a mail): the lab's fifth round still saw such
                # bodies cut to their first line.
                k = j
                while k < len(lines) and lines[k].strip():
                    h = lines[k].strip().split(":", 1)[0].strip("*` ").upper()
                    if h in ("POST", "ACTION") and ":" in lines[k]:
                        break
                    k += 1
                item += lines[j:k]
                i = k - 1
            out[head.lower()].append("\n".join(item).strip())
        else:
            out["reply"].append(line)
        i += 1
    out["reply"] = ["\n".join(out["reply"]).strip()] if "\n".join(out["reply"]).strip() else []
    return out


def segments(text: str) -> dict[str, str]:
    """An agent's reply split as the roles say: `POST: ...` lines are posts in the conversation it
    serves, `ACTION: ...` lines are outside actions for the platform, and the rest is what its own
    human reads."""
    out = {"post": [], "action": [], "reply": []}
    for line in (text or "").splitlines():
        s = line.strip()
        head = s.split(":", 1)[0].strip("*` ").upper()
        if head == "POST" and ":" in s:
            out["post"].append(s.split(":", 1)[1].strip())
        elif head == "ACTION" and ":" in s:
            out["action"].append(s.split(":", 1)[1].strip())
        else:
            out["reply"].append(line)
    return {k: "\n".join(v).strip() for k, v in out.items()}


SOURCE_SEGMENT = {"human": "reply", "person": "post", "world": "action"}

# The world's answer to `ACTION:` lines that none of the scenario's world rules answers. Every role
# says that the platform does an action and gives the result in its next message, so an action is
# always answered; without this, the session got no next message and ended there. In the first
# final round (2026-10-04), 69 of 154 sessions ended so, 34 of them Codex sessions whose first reply
# was an action and that never called the forum. A scenario may set its own (`world_default`), or
# null for none. The answer names no forum tool: a role never says how to use the forum.
WORLD_DEFAULT = "No result: the platform could not do this action."


def rule_fires(rule: dict, text: str) -> bool:
    low = text.lower()
    words = [w.lower() for w in rule.get("match", [])]
    hit = any(w in low for w in words) if words else True
    when = rule.get("when", "told")
    if when == "asked":
        return "?" in text and hit
    if when == "always":
        return True
    return bool(words) and hit


class Run:
    def __init__(
        self,
        scenario: dict,
        vendor: str,
        model: str,
        arm: str,
        run_dir: Path,
        binary: Path,
        schema_adapter: str = "off",
        text_only: bool = False,
        mcp_negotiation: str = "legacy",
        max_minutes: float = 60.0,
        prelude_only: bool = False,
        isolation_problems: list[str] | None = None,
        interview: bool = False,
        agent_models: dict | None = None,
        lab: dict | None = None,
        log=print,
    ):
        self.prelude_only = prelude_only
        # The lab (lab.py): each agent's own `vendor:model`, and the lab's client settings
        # (`codex_direct`: the forum's tools as direct functions in Codex; `codex_instructions`:
        # the server's instructions put before a Codex agent's role, since Codex shows a model no
        # MCP server's instructions; `codex_effort`).
        self.agent_models = dict(agent_models or {})
        self.lab = dict(lab or {})
        # The lab's simulated world, humans, and people (sim.py), in place of the keyword rules.
        self.sim = SIM.Simulator(scenario, log=log) if self.lab.get("sim") else None
        # The interview after the counting.
        self.interview = interview
        self.s = scenario
        self.vendor = vendor
        self.model = model
        self.arm = arm
        self.dir = run_dir
        self.binary = binary
        self.flatten = schema_adapter == "on" or (schema_adapter == "auto" and vendor == "claude")
        self.text_only = text_only
        self.mcp_negotiation = mcp_negotiation
        self.max_seconds = max_minutes * 60
        self.log = log
        self.meta: dict = {
            "scenario": scenario["id"],
            "vendor": vendor,
            "model": model,
            "arm": arm,
            # The protocol's version as `docs/protocol.md` stated it when the run was made.
            "protocol_version": S.protocol_version(),
            "schema_adapter": "flatten" if self.flatten else "none",
            "text_only": text_only,
            # Claude agents: `legacy` forces MCP_PROTOCOL_NEGOTIATION=legacy; `client` leaves the
            # version to the client (agents.MCP_NEGOTIATIONS).
            "mcp_negotiation": mcp_negotiation if vendor in ("claude", "lab") else None,
            "deliveries": [],
            "agents": {},
            "prelude": [],
            "events": [],
            "status": "running",
            # Problems known before the run (a Codex model run without a passed probe): the report
            # leaves such a run out of every count.
            "isolation_problems": list(isolation_problems or []),
            # The core packages the counter takes the server to have (packages.py).
            "built_packages": sorted(PK.BUILT),
            "interview_chosen": interview,
            "agent_models": self.agent_models,
            "lab": self.lab,
        }
        self.cv = threading.Condition()
        self.done_agents: dict[str, float] = {}
        self.events: dict[str, float] = {}
        self.event_prompts: dict[str, list[dict]] = {}
        self.sessions: dict[str, A.AgentSession] = {}
        # Set when the agents must stop: the run's time limit, or an interruption.
        self.stop_all = threading.Event()
        # Set only when the harness was interrupted: the prelude's and the drain's waits end, and
        # nothing is counted or interviewed. A run that reached its time limit still drains, is
        # counted, and is interviewed.
        self.halted = threading.Event()
        self.server = None
        self.proxy = None

    @property
    def interrupted(self) -> bool:
        return self.halted.is_set()

    def interrupt(self) -> None:
        """Stops the run from another thread (the harness was interrupted): the sessions are killed,
        the waits end, and the run finishes as `interrupted`, not counted and not interviewed."""
        self.halted.set()
        self.stop_all.set()
        for sess in list(self.sessions.values()):
            sess.kill(reason="interrupted")

    # -- the run --------------------------------------------------------------------------------

    def execute(self) -> dict:
        # A fresh database per run: a run directory left by an earlier run is replaced, after what
        # its client kept of its sessions, if anything, is tidied (interview.tidy_run).
        if self.dir.exists():
            IV.tidy_run(self.dir)
            shutil.rmtree(self.dir)
        self.dir.mkdir(parents=True)
        params = P.load(SC.scenarios_dir(), self.s.get("params"))
        self.meta["params"] = params
        tokens = dict(self.s["tokens"])
        agent_tokens = {}
        for a in self.s["agents"]:
            name = f"agent-{a['id']}".lower()
            tokens[name] = self.s["tokens"][a["token"]]
            agent_tokens[a["id"]] = name
        # A seed whose prelude advances the forum's clock starts it that far behind, so that the
        # agents arrive at the real time (`advance` steps, `prelude`).
        lag = sum(P.ms(step["advance"]) for step in self.s.get("prelude", []) if "advance" in step)
        kw = {"clock_lag_ms": lag} if lag else {}
        self.server = S.Server(self.binary, self.dir / "server", params, self.arm, tokens, **kw)
        self.meta["clock_lag_ms"] = lag
        self.proxy = None
        self.meta["started"] = time.time()
        self.agent_tokens = agent_tokens
        try:
            self.server.start()
            self.proxy = X.Proxy(self.server.url, self.dir / "proxy.jsonl", dump_dir=self.dir).start()
            for a in self.s["agents"]:
                self.proxy.register(
                    self.server.tokens[agent_tokens[a["id"]]],
                    a["id"],
                    flatten_schemas=self.flatten,
                    text_only=self.text_only,
                )
            self.server_instructions = None
            if self.lab.get("codex_instructions"):
                c = M.McpClient(self.server.url, next(iter(self.server.tokens.values())))
                c.initialize()
                self.server_instructions = c.instructions
                c.close()
            if not self.prelude():
                self.meta["status"] = "interrupted" if self.interrupted else "prelude_failed"
                return self.finish(drain=False)
            if self.prelude_only:
                self.meta["status"] = "prelude_ok"
                return self.finish(drain=False)
            self.t0 = time.time()
            threads = [threading.Thread(target=self.agent_main, args=(a,), daemon=True) for a in self.s["agents"]]
            for t in threads:
                t.start()
            deadline = time.monotonic() + self.max_seconds
            for t in threads:
                t.join(timeout=max(0.0, deadline - time.monotonic()))
            if any(t.is_alive() for t in threads) and not self.interrupted:
                self.meta["status"] = "timeout"
                self.stop_all.set()
                for sess in list(self.sessions.values()):
                    # A turn this cuts ends as `run_timeout`: the harness ended it mid-task.
                    sess.kill(reason="run_timeout")
                for t in threads:
                    t.join(timeout=30)
            else:
                for t in threads:
                    t.join(timeout=30)
                self.meta["status"] = "interrupted" if self.interrupted else "done"
            return self.finish(drain=not self.interrupted)
        except Exception:
            self.meta["status"] = "error"
            self.meta["error"] = traceback.format_exc()
            return self.finish(drain=False)
        except BaseException:
            # KeyboardInterrupt, SystemExit: the sessions are killed and the run finishes (its
            # client's sessions tidied) before the interruption goes on.
            self.meta["status"] = "interrupted"
            self.interrupt()
            self.finish(drain=False)
            raise

    def finish(self, drain: bool) -> dict:
        transcripts: dict = {}
        try:
            if drain:
                wait = P.ms(self.s.get("drain", "0s")) / 1000
                self.log(f"    draining {wait:.0f} s so that the deadlines left behind fire")
                self.halted.wait(wait)
            if self.proxy:
                self.proxy.stop()
            if self.server:
                self.server.stop()
            self.meta["ended"] = time.time()
            self.meta["agent_sessions"] = {
                k: sorted(v) for k, v in (self.proxy.sessions if self.proxy else {}).items()
            }
            # The tools/list each agent was served, saved in the run directory (proxy.py).
            self.meta["tools_served"] = dict(self.proxy.served) if self.proxy else {}
            history = []
            try:
                history = self.server.export() if self.server else []
            except Exception as e:  # noqa: BLE001 - recorded, the run is still reported
                self.meta["export_error"] = repr(e)
            for aid, sess in self.sessions.items():
                tr = sess.transcript()
                transcripts[aid] = tr
                (self.dir / "agents" / f"{aid}.transcript.json").write_text(json.dumps(tr, indent=1))
            proxy_log = []
            if (self.dir / "proxy.jsonl").exists():
                proxy_log = [json.loads(x) for x in (self.dir / "proxy.jsonl").read_text().splitlines() if x.strip()]
            (self.dir / "run.json").write_text(json.dumps(self.meta, indent=1))
            if self.meta["status"] in ("done", "timeout"):
                try:
                    result = SL.count(self.s, self.meta, history, proxy_log, transcripts)
                except Exception:  # noqa: BLE001 - a lab seed the counter cannot read is still interviewed
                    if not self.lab:
                        raise
                    result = {"total": None, "error": traceback.format_exc()}
                (self.dir / "slips.json").write_text(json.dumps(result, indent=1))
                self.meta["slips_total"] = result["total"]
                (self.dir / "run.json").write_text(json.dumps(self.meta, indent=1))
                if self.interview and not self.interrupted:
                    # After the counting, with the server stopped: nothing the agents say now
                    # reaches the history, the proxy's log, or the transcripts the counter read.
                    try:
                        self.meta["interview"] = IV.interview_run(
                            self.dir, self.s, self.meta, transcripts, log=self.log, stop=self.halted
                        )
                    except Exception:  # noqa: BLE001 - the run and its count stand without it
                        self.meta["interview"] = {"error": traceback.format_exc()}
        finally:
            if (self.interview or self.vendor == "claude") and not (self.meta.get("interview") or {}).get("file"):
                # Not interviewed (not chosen, or the run failed or was interrupted, or the interview
                # failed): what a Claude client kept of the run's sessions, as the run recorded it
                # before the client started, is copied into the run directory and removed from the
                # client's, as an interview does. Whatever failed above, this runs.
                self.meta["client_files"] = IV.tidy_run(self.dir, transcripts)
            (self.dir / "run.json").write_text(json.dumps(self.meta, indent=1))
        return self.meta

    # -- the prelude ----------------------------------------------------------------------------

    def prelude(self) -> bool:
        clients: dict[str, M.McpClient] = {}
        env: dict = {}
        for i, step in enumerate(self.s.get("prelude", [])):
            if "advance" in step:
                # The forum's time passes (a lab seed's history spans weeks): the lagged clock
                # catches up, and the next call settles whatever fell due meanwhile.
                self.server.catch_up(P.ms(step["advance"]))
                self.meta["prelude"].append({"i": i, "advance": step["advance"]})
                continue
            if "sleep" in step:
                # Let deadlines fall due (a question lapse, a lease end) before the next step.
                self.log(f"    prelude step {i}: waiting {step['sleep']} ({step.get('why', '')})")
                if self.halted.wait(P.ms(step["sleep"]) / 1000):
                    return False  # interrupted
                self.meta["prelude"].append({"i": i, "sleep": step["sleep"]})
                continue
            actor = step["as"] + "/" + step.get("session_name", "")
            if actor not in clients:
                c = M.McpClient(self.server.url, self.server.tokens[step["as"]])
                c.initialize()
                clients[actor] = c
            args = _resolve(step["args"], env)
            reopened = clients[actor].reopened
            r = clients[actor].call(step["tool"], args)
            rec = {"i": i, "as": step["as"], "tool": step["tool"], "args": args, "text": r.text[:4000]}
            if clients[actor].reopened != reopened:
                # The server had expired the actor's idle MCP session (a sleep step outlasts
                # presence.silence): the call went again on a new one, in the same forum session.
                rec["mcp_session_reopened"] = {"forum_session": args.get("session") or clients[actor].forum_session}
            self.meta["prelude"].append(rec)
            refused = A.is_refusal(r.text) or r.is_error
            if refused and not step.get("may_fail"):
                self.meta["prelude_failure"] = rec
                self.log(f"    prelude step {i} refused: {r.text.splitlines()[1:3]}")
                return False
            for var, path in step.get("save", {}).items():
                env[var] = _dig(r.structured or {}, path)
            found = M.status_session(r.text)
            if found:
                env[f"session_{step['as']}"] = found[1]
        for c in clients.values():
            c.close()
        # The agents arrive at the real time, whatever the steps left of the lag.
        if getattr(self.server, "clock_lag_ms", 0):
            self.server.catch_up(self.server.clock_lag_ms)
        return True

    # -- the agents -----------------------------------------------------------------------------

    def _wait_start(self, a: dict) -> bool:
        st = a.get("start", {"at": "0s"})
        delay = P.ms(st.get("delay", "0s")) / 1000
        with self.cv:
            if "at" in st:
                target = self.t0 + P.ms(st["at"]) / 1000
            elif "after" in st:
                while st["after"] not in self.done_agents and not self.stop_all.is_set():
                    self.cv.wait(timeout=1)
                target = self.done_agents.get(st["after"], time.time()) + delay
            elif "on" in st:
                while st["on"] not in self.events and not self.stop_all.is_set():
                    if self._all_others_done(a["id"]):
                        return False  # the event can no longer come
                    self.cv.wait(timeout=1)
                target = self.events.get(st["on"], time.time()) + delay
            else:
                target = self.t0
        while time.time() < target and not self.stop_all.is_set():
            time.sleep(min(1.0, target - time.time()))
        return not self.stop_all.is_set()

    def _all_others_done(self, me: str) -> bool:
        others = [x["id"] for x in self.s["agents"] if x["id"] != me]
        waiting_on_events = [x["id"] for x in self.s["agents"] if x["id"] != me and "on" in x.get("start", {})]
        return all(o in self.done_agents or o in waiting_on_events for o in others)

    def _raise(self, event: str, delay: str | None, prompt: dict | None) -> None:
        at = time.time() + P.ms(delay or "0s") / 1000
        with self.cv:
            if event not in self.events:
                self.events[event] = at
                if prompt:
                    self.event_prompts.setdefault(event, []).append(prompt)
                self.meta["events"].append({"event": event, "at": at})
                self.cv.notify_all()

    def _deliver(self, aid: str, line: dict) -> None:
        if self.sim:
            self.sim.note_line(aid, line.get("from", "human"), line["text"])
        rec = {"agent": aid, "from": line.get("from", "human"), "text": line["text"], "t": time.time()}
        if "trigger_met" in line:
            rec["trigger_met"] = line["trigger_met"]
        if line.get("default"):
            rec["default"] = True  # the world's answer to an action no rule answered (WORLD_DEFAULT)
        self.meta["deliveries"].append(rec)

    def agent_main(self, a: dict) -> None:
        aid = a["id"]
        try:
            if not self._wait_start(a):
                return
            self.log(f"    agent {aid} starts")
            vendor, model = self.vendor, self.model
            if aid in self.agent_models:
                vendor, _, model = self.agent_models[aid].partition(":")
            role = a["role"]
            if vendor == "codex" and self.server_instructions:
                role = CODEX_INSTRUCTIONS.format(instructions=self.server_instructions.strip(), role=role)
            sess = A.make(
                vendor,
                aid,
                model,
                self.proxy.url,
                self.server.tokens[self.agent_tokens[aid]],
                role,
                self.dir / "agents",
                end=a.get("end"),
                max_calls_per_turn=a.get("max_calls_per_turn", 40),
                turn_timeout=a.get("turn_timeout_s", 600),
                script=a.get("script"),
                mcp_negotiation=self.mcp_negotiation,
                # A Claude client keeps its record of every session: the isolation check reads what
                # the client added to the model's context from it, and the interview resumes it.
                persist=True,
                direct_tools=bool(self.lab.get("codex_direct")),
                effort=self.lab.get("codex_effort", "low"),
            )
            if getattr(sess, "persist", False):
                # Before the client starts, so that the session it keeps can be tidied however the
                # run ends (interview.tidy_run, interview.tidy_orphans).
                IV.write_record(self.dir / "agents", aid, sess.vendor, str(sess.workdir), sess.session_id)
            self.sessions[aid] = sess
            self.meta["agents"][aid] = {
                "started": time.time(),
                "workdir": str(sess.workdir),
                "vendor": vendor,
                "model": model,
                "role": role,
            }
            queue: list[dict] = list(self.event_prompts.get(a.get("start", {}).get("on", ""), []))
            pending: list[dict] = list(a.get("lines", []))
            used: set = set()
            turns = 0
            while not self.stop_all.is_set() and turns < a.get("max_turns", 8):
                # Replies already queued come first; then the next scripted line, once its call
                # has been made; a held-back line whose call never came is said anyway at the end.
                if not queue and pending:
                    nxt = dict(pending.pop(0))
                    if nxt.get("after_call"):
                        nxt["trigger_met"] = self._matched(sess, nxt["after_call"])
                    queue.append(nxt)
                if not queue:
                    break
                line = queue.pop(0)
                self._deliver(aid, line)
                turn = sess.turn(line["text"])
                turns += 1
                self.log(f"    agent {aid} turn {turn.n}: {len(turn.calls)} calls, ended by {turn.ended_by}")
                if sess.ended:
                    break
                reply = self._replies(a, turn.text, used)
                queue.extend(reply)
            sess.finish()
            self.meta["agents"][aid].update(
                {
                    "ended": time.time(),
                    "turns": len(sess.turns),
                    "calls": len(sess.calls),
                    "vanished": sess.ended,
                    "isolation": ISO.after_session(sess),
                }
            )
            sess.close()
        except Exception:
            self.meta["agents"].setdefault(aid, {})["error"] = traceback.format_exc()
        finally:
            with self.cv:
                self.done_agents[aid] = time.time()
                self.cv.notify_all()

    def _matched(self, sess: A.AgentSession, want: dict) -> bool:
        for c in sess.calls:
            if c["tool"] == want.get("tool") and (c.get("args") or {}).get("action") == want.get("action"):
                if not want.get("ok") or not c.get("refused"):
                    return True
        return False

    def _sim_replies(self, a: dict, text: str) -> list[dict]:
        """The lab's replies: the platform answers the actions, the person the posts, the human the
        rest, each played by the simulator model (sim.py)."""
        seg = segments(text)
        items = segment_lists(text)
        out: list[dict] = []
        actions = items["action"]
        if actions:
            w = self.sim.world(a["id"], actions)
            out.append({"from": "world", "text": "[platform: the output of your ACTION lines; tool output, not a "
                        f"message from a person]\n{w}"})
        if a.get("conversation") and seg["post"]:
            r = self.sim.person(a["id"], a["conversation"], items["post"])
            if r:
                p = self.s["people"][a["conversation"]]
                m = re.search(r"chat:([^)\s]+)", p.get("where", ""))
                with self.cv:
                    self.message_n = getattr(self, "message_n", 100) + 1
                    n = self.message_n
                mid = f", message M{n} (message id chat:{m.group(1)}/M{n})" if m else ""
                out.append({"from": "person", "text": f"New message in {p.get('where', 'the conversation')}{mid} from "
                            f"{p.get('who', a['conversation'])}: {r}"})
        if a.get("human") and seg["reply"]:
            r = self.sim.human(a["id"], a["human"], seg["reply"])
            if r:
                out.append({"from": "human", "text": r})
        self.meta["sim"] = {"calls": len(self.sim.calls), "cost_usd": round(sum(c["cost_usd"] for c in self.sim.calls), 4)}
        return out

    def _replies(self, a: dict, text: str, used: set) -> list[dict]:
        if self.sim:
            return self._sim_replies(a, text)
        out: list[dict] = []
        sources = []
        if a.get("human"):
            h = self.s["humans"][a["human"]]
            sources.append(("human", a["human"], h.get("replies", [])))
        if a.get("conversation"):
            p = self.s["people"][a["conversation"]]
            sources.append(("person", a["conversation"], p.get("replies", [])))
        sources.append(
            ("world", "world", [r for r in self.s.get("world", []) if r.get("agent", a["id"]) in (a["id"], "*")])
        )
        seg = segments(text)
        for kind, who, rules in sources:
            answered = False
            for i, rule in enumerate(rules):
                key = (kind, who, i)
                if key in used and rule.get("once", True):
                    continue
                part = seg[rule.get("on", SOURCE_SEGMENT[kind])]
                if not part or not rule_fires(rule, part):
                    continue
                used.add(key)
                then = rule.get("then")
                say = rule["say"]
                if kind == "person":
                    p = self.s["people"][who]
                    say = f"New message in {p.get('where', 'the conversation')} from {p.get('who', who)}: {say}"
                elif kind == "world":
                    say = f"[platform] {say}"
                line = {"from": kind, "text": say}
                if then and "event" in then:
                    self._raise(then["event"], then.get("delay"), line)
                else:
                    out.append(line)
                answered = True
                break
            default = self.s.get("world_default", WORLD_DEFAULT)
            if kind == "world" and not answered and seg["action"] and default:
                out.append({"from": "world", "text": f"[platform] {default}", "default": True})
        return out
