"""The lab's simulated world, humans, and people: a model plays them, from a seed's facts.

Keyword rules (runner.Run._replies) answer only what their words foresaw: in the lab's first smoke run an
inspection request got "printed nothing", a rule fired on the word "fix" inside a read, and a human
thanked the agent for a pull request that did not exist, so the agents spent their turns on the world
instead of the forum. Here a model answers instead, from the seed's facts (the scripted rules' texts
included, so a seed's key facts stay fixed) and from what the same agent was told before, so that
answers stay consistent:

- the platform answers every `ACTION:` line of a reply, in one message;
- the person in the conversation answers `POST:` lines when a person would;
- the agent's own human answers the rest of the reply when a person would.

"(no reply)" means silence; an agent with nothing more to read ends its session. Each call is one
`claude -p` turn of `SIM_MODEL`, with no tool, no MCP server, no session kept, and the run's isolation
switches; what it costs is recorded in the run (`sim` in run.json).
"""

from __future__ import annotations

import json
import subprocess
import tempfile
import threading
import time

from . import agents as A

SIM_MODEL = "claude-sonnet-5-5"
SILENT = "(no reply)"

WORLD_SYSTEM = """You simulate the outside world for an AI agent in a usability test: the platform that runs its \
shell commands, git, CI, deploys, dashboards, logs, jobs, mail, and file reads. The agent writes requests as ACTION \
lines; you return what the platform would output.

Rules:
- Answer every action, numbered as given, as terse realistic tool output or a one-line status (at most about 120 \
words per action). No advice, no commentary, never mention the test.
- Stay consistent with FACTS and with EARLIER OUTPUTS. FACTS are the truth; never contradict them.
- Where FACTS say nothing, invent plausible, mundane details that do not change the story: no new incidents, no new \
requirements, no new people, and no surprises that would decide the agent's task for it.
- A read returns content (excerpts are fine). A write (edit, commit, push, pull request, deploy, job, post, mail) \
succeeds and returns its ids or links, unless FACTS say it fails.
- Commands run where the agent works: in checkouts of the repositories named in FACTS (git, the build tools, and the \
pull request CLI work there), with the access its role describes.
- Any clock reading (date, a fresh log line, a just-created object's timestamp) uses NOW; past events keep the times \
FACTS give them.
- An action the platform cannot do gets the error a real tool would print."""

PERSON_SYSTEM = """You play {who} in a usability test of an AI agent. You talk with the agent through {channel}. Stay \
in character: brief, casual, realistic; one to three sentences.

What you know and want:
{knowledge}

Rules:
- Reply to the agent's latest message only if a real person would: answer its questions from what you know (if you \
do not know, say so, or tell it to use its judgement); acknowledge a finished result briefly; react if it claims \
something you know is wrong.
- A status update or a promise to report later needs no reply.
- Never give the agent new tasks, never coach it on its tools, never mention a forum or the test.
- If no reply is needed, output exactly: (no reply)
- Output only your message."""

_COST_LOCK = threading.Lock()


def ask(system: str, prompt: str, timeout: float = 180.0) -> tuple[str, float]:
    """One turn of the simulator model; returns its text and its cost in USD."""
    cmd = [
        "claude",
        "-p",
        "--model",
        SIM_MODEL,
        "--output-format",
        "json",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--system-prompt",
        system,
        prompt,
    ]
    env = A.agent_env(A.claude_isolation_env("client", SIM_MODEL))
    with tempfile.TemporaryDirectory(prefix="forum-sim-") as d:
        p = subprocess.run(cmd, cwd=d, env=env, capture_output=True, text=True, timeout=timeout,
                           stdin=subprocess.DEVNULL)
    try:
        out = json.loads(p.stdout)
    except json.JSONDecodeError:
        return "", 0.0
    if isinstance(out, list):  # Claude Code 2.1 prints the session's messages, the result last
        out = next((m for m in reversed(out) if isinstance(m, dict) and m.get("type") == "result"), {})
    return (out.get("result") or "").strip(), float(out.get("total_cost_usd") or 0.0)


def rule_hint(rule: dict) -> str:
    words = ", ".join(rule.get("match") or [])
    when = {"asked": "If asked", "told": "If told", "always": "Whenever it writes"}.get(rule.get("when", ""), "If")
    about = f" about {words}" if words else ""
    return f"- {when}{about}: {rule['say']}"


class Simulator:
    """The world, humans, and people of one lab run."""

    def __init__(self, seed: dict, log=print):
        self.s = seed
        self.log = log
        self.history: dict[str, list[str]] = {}
        self.calls: list[dict] = []

    def facts(self, aid: str) -> list[str]:
        """The seed's facts, then its scripted world rules as conditional facts: a rule's text is what an
        action about its words would show, not something that has happened."""
        out = list(self.s.get("facts", []))
        for r in self.s.get("world", []):
            if r.get("agent", aid) in (aid, "*"):
                words = ", ".join(r.get("match") or [])
                out.append(f"IF an action is about {words}: it shows this (only once the agent does it): {r['say']}")
        return out

    def _ask(self, kind: str, aid: str, system: str, prompt: str) -> str:
        text, cost = ask(system, prompt)
        with _COST_LOCK:
            self.calls.append({"kind": kind, "agent": aid, "cost_usd": cost, "prompt_chars": len(prompt)})
        return text

    def world(self, aid: str, actions: list[str]) -> str | None:
        if not actions:
            return None
        # One world for every agent of the run, so that what one agent's action changed, another sees.
        hist = self.history.setdefault("world", [])
        prompt = f"NOW: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\n\nFACTS:\n"
        prompt += "\n".join(f"- {f}" for f in self.facts(aid))
        prompt += "\n\nEARLIER OUTPUTS (all agents; agent " + aid + " asks now):\n"
        prompt += "\n".join(hist[-16:]) if hist else "(none)"
        prompt += "\n\nNEW ACTIONS:\n" + "\n".join(f"{i}. {a}" for i, a in enumerate(actions, 1))
        out = self._ask("world", aid, WORLD_SYSTEM, prompt)
        if not out:
            out = "Error: the platform did not answer; try again."
        hist.append(f"AGENT {aid} ACTIONS: " + " | ".join(actions) + "\nOUTPUT: " + out)
        return out

    def _person(self, kind: str, aid: str, who: str, channel: str, knowledge: list[str], said: str) -> str | None:
        hist = self.history.setdefault(f"{kind}:{aid}", [])
        system = PERSON_SYSTEM.format(who=who, channel=channel, knowledge="\n".join(knowledge) or "- (nothing special)")
        prompt = "CONVERSATION SO FAR:\n" + ("\n".join(hist[-16:]) if hist else "(none)")
        prompt += f"\n\nTHE AGENT'S LATEST MESSAGE:\n{said}\n\nYour reply, or (no reply):"
        out = self._ask(kind, aid, system, prompt)
        hist.append(f"AGENT: {said}")
        if not out or out.strip().lower().strip(".") in (SILENT.strip("()"), SILENT):
            return None
        hist.append(f"YOU: {out}")
        return out

    def note_line(self, aid: str, kind: str, text: str) -> None:
        """A line the agent received from its human or person (the first message, a scripted line)."""
        key = {"human": "human", "person": "person"}.get(kind)
        if key:
            self.history.setdefault(f"{key}:{aid}", []).append(f"YOU: {text}")

    def human(self, aid: str, name: str, reply: str) -> str | None:
        if not reply.strip():
            return None
        h = self.s["humans"][name]
        knowledge = [h.get("knowledge", "")] if h.get("knowledge") else []
        knowledge += [rule_hint(r) for r in h.get("replies", [])]
        return self._person("human", aid, f"{h['who']} ({h.get('principal')})", h.get("channel", "chat"),
                            knowledge, reply)

    def person(self, aid: str, name: str, posts: list[str]) -> str | None:
        if not posts:
            return None
        p = self.s["people"][name]
        knowledge = [p.get("knowledge", "")] if p.get("knowledge") else []
        knowledge += [rule_hint(r) for r in p.get("replies", [])]
        return self._person("person", aid, p["who"], p.get("where", "a chat thread"), knowledge, "\n".join(posts))
