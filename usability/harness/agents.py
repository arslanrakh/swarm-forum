"""Agents driven through their vendors' own command-line clients, with only the forum attached.

- Claude models: `claude -p` in streaming-JSON mode, one process per agent session, so that each
  human line continues the same session. Only the forum's MCP server is loaded
  (`--mcp-config` with `--strict-mcp-config`), only its six tools are allowed, the built-in tools
  are off (`--tools ""`), no settings source, CLAUDE.md, memory, skill, or slash command is loaded,
  and the session runs in an empty temporary directory. `check_isolation` verifies this. The
  client's small, fast model is pinned to the session's own model, and the models it reports
  using are recorded (`models`). The run has the client keep its record of the session on disk
  (`persist`), under a session id the harness chooses: the isolation check reads what the client
  added to the model's context from it (isolation.py), and the interview resumes it.
- OpenAI models: `codex exec --json`, one process per turn, resumed by thread id for the next turn,
  with the forum as its only MCP server, the user configuration and rules ignored, a read-only
  sandbox, the shell and the other optional tools disabled, in an empty temporary directory.

`claude_interview_command` and `codex_interview_command` resume (Claude) or fork (Codex) a
finished session for the interview after the run (interview.py): the same isolation, with no MCP
server, no Claude tool, and Codex's tools as far off as its CLI allows.

Each driver turns the client's streamed JSON into one normalised transcript (a list of turns, each
with its forum calls and their results, the final text, and the usage), and ends the session at a
scenario's fixed point by killing the client right after the response to the call that reaches it.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import subprocess
import threading
import time
import uuid
from pathlib import Path

from .mcp import status_session
from .server import minimal_env

FORUM_TOOLS = ["session", "view", "node", "hold", "talk", "control"]

# Environment variables that carry the vendors' API keys or logins; the rest of the caller's
# environment never reaches an agent.
AUTH_VARS = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "CODEX_HOME",
]

# Claude Code switches that keep everything but the forum out of the session.
CLAUDE_ISOLATION_ENV = {
    "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1",
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
    "CLAUDE_CODE_DISABLE_ORG_MEMORY": "1",
    "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS": "1",
    "CLAUDE_CODE_DISABLE_WORKFLOWS": "1",
    "CLAUDE_CODE_DISABLE_CRON": "1",
    "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS": "1",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    "CLAUDE_CODE_DISABLE_TERMINAL_TITLE": "1",
    # The forum's tools load with the session, not behind a tool search the session lacks.
    "ENABLE_TOOL_SEARCH": "false",
    # No claude.ai connectors of the operator's account.
    "ENABLE_CLAUDEAI_MCP_SERVERS": "false",
    # Claude Code 2.1.287 negotiates MCP 2026-07-28 with the forum, whose tools/list result then
    # lacked the `ttlMs` and `cacheScope` that this version requires, and the client dropped every
    # forum tool. The legacy negotiation lists them: the server logs 2025-11-25 for these sessions
    # (2025-06-18 is the harness's own prelude client, mcp.py). Reported with WP-U; the server
    # serves the hints since WP-E5, and `--mcp-negotiation client` leaves the negotiation to the
    # client (`MCP_NEGOTIATIONS`).
    "MCP_PROTOCOL_NEGOTIATION": "legacy",
}

# `--mcp-negotiation`: `legacy` sets the switch above; `client` leaves it out, so that Claude Code
# negotiates its own default version (2026-07-28 for 2.1.287).
MCP_NEGOTIATIONS = ("legacy", "client")


# Claude Code sends some requests of its own to a small, fast model (Haiku by default). It was
# decided that no Haiku model takes part in this test: both variables that name that model are
# set to the session's own model, and the models the client reports using are checked
# (`other_models`).
CLAUDE_SMALL_MODEL_VARS = ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "ANTHROPIC_SMALL_FAST_MODEL")


def claude_isolation_env(negotiation: str, model: str | None = None) -> dict:
    """The isolation switches, without the forced legacy negotiation under `client`, and with the
    small, fast model pinned to `model` when it is given."""
    if negotiation not in MCP_NEGOTIATIONS:
        raise ValueError(f"mcp_negotiation {negotiation!r}: expected one of {MCP_NEGOTIATIONS}")
    env = dict(CLAUDE_ISOLATION_ENV)
    if negotiation == "client":
        env.pop("MCP_PROTOCOL_NEGOTIATION", None)
    if model:
        env.update({k: model for k in CLAUDE_SMALL_MODEL_VARS})
    return env


# Claude Code marks a message it made up itself (an API error, an interruption) with this model.
SYNTHETIC_MODEL = "<synthetic>"


def other_models(seen, asked: str) -> list[str]:
    """The models a Claude client reported using that are not the one asked for (a dated or a
    `[1m]` form of it counts as the same model)."""
    return sorted(
        m
        for m in set(seen or [])
        if m and m != SYNTHETIC_MODEL and m != asked and not m.startswith((asked + "-", asked + "["))
    )


# Settings passed on the command line (no settings file is read): the built-in plugins off.
CLAUDE_SETTINGS = {
    "enabledPlugins": {
        "cc-plugin-agents-md@builtin": False,
        "cc-plugin-plugin-authoring@builtin": False,
    },
}

# Codex features that add tools or context; all are turned off.
CODEX_DISABLED_FEATURES = [
    "multi_agent_v2",
    "shell_tool",
    "unified_exec",
    "apps",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "image_generation",
    "in_app_browser",
    "multi_agent",
    "plugins",
    "remote_plugin",
    "view_image",
    "goals",
    "sleep_tool",
    "tool_suggest",
    "skill_search",
    "hooks",
    "memories",
    "skill_mcp_dependency_install",
    "workspace_dependencies",
]


# Code mode's setting that offers a namespace's tools as direct functions, outside `exec` (Codex
# 0.159.2): with it the model sees `mcp__forum.session` and the rest at the top level, as a Claude
# model does. Without it the forum's tools are deferred inside `exec`, where half of the first
# final round's Codex sessions never found them (usability/report.md, F1).
CODEX_DIRECT_FORUM = 'features.code_mode.direct_only_tool_namespaces=["mcp__forum"]'


def codex_flags(model: str, role: str, effort: str, mcp_url: str | None, direct: bool = False) -> list[str]:
    """The flags of every Codex process the harness starts: the user configuration and rules
    ignored, no project doc, no web search, every optional feature in `CODEX_DISABLED_FEATURES`
    off, and the forum as the only MCP server when `mcp_url` is given (none for the interview);
    `direct` offers the forum's tools as direct functions (`CODEX_DIRECT_FORUM`)."""
    f = ["--json", "--skip-git-repo-check", "--ignore-user-config", "--ignore-rules", "-m", model]
    if mcp_url is not None:
        f += [
            "-c",
            f"mcp_servers.forum.url={json.dumps(mcp_url)}",
            "-c",
            'mcp_servers.forum.bearer_token_env_var="FORUM_AGENT_TOKEN"',
            "-c",
            "mcp_servers.forum.tool_timeout_sec=120",
            "-c",
            'mcp_servers.forum.default_tools_approval_mode="approve"',
        ]
        if direct:
            f += ["-c", CODEX_DIRECT_FORUM]
    f += [
        "-c",
        f"developer_instructions={json.dumps(role)}",
        "-c",
        "project_doc_max_bytes=0",
        "-c",
        f"model_reasoning_effort={json.dumps(effort)}",
        "-c",
        'web_search="disabled"',
    ]
    for feat in CODEX_DISABLED_FEATURES:
        f += ["--disable", feat]
    return f


def claude_interview_command(model: str, role: str, session_id: str, budget_usd: float | None = None) -> list[str]:
    """`claude -p --resume` of a finished agent session, for its interview: the run's isolation
    (no settings source, the built-in plugins off, no slash command, the role appended), with no
    MCP server (`--strict-mcp-config` and no `--mcp-config`), no built-in tool (`--tools ""`), the
    forum's tools denied as well, nothing written back to the session (the run's record of it
    stays as it was), and, when given, a budget (`--max-budget-usd`: the client stops after the
    request that reaches it and reports `error_max_budget_usd`; a resumed session's cost includes
    the run's turns, which the caller adds). The questions come on stdin."""
    budget = ["--max-budget-usd", f"{budget_usd:.4f}"] if budget_usd is not None else []
    return [
        "claude",
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--model",
        model,
        "--strict-mcp-config",
        "--tools",
        "",
        "--disallowedTools",
        ",".join(f"mcp__forum__{t}" for t in FORUM_TOOLS),
        "--permission-mode",
        "dontAsk",
        "--setting-sources",
        "",
        "--settings",
        json.dumps(CLAUDE_SETTINGS),
        "--disable-slash-commands",
        "--no-session-persistence",
        *budget,
        "--append-system-prompt",
        role,
        "--resume",
        session_id,
    ]


# For the interview, Codex runs with the forum's server left out, the run's features off, code
# mode's host off as well (code mode then fails closed: `exec` runs nothing, and the tools nested
# in it, `apply_patch` and the clock tool, are gone), and a read-only sandbox. That is as far as
# the CLI allows: Codex 0.159 still declares code mode's `exec` and `wait`, `request_user_input`,
# and, with the operator's own CODEX_HOME, the multi-agent tools, and reports none of them.
CODEX_INTERVIEW_CONFIG = ['sandbox_mode="read-only"']
CODEX_INTERVIEW_DISABLED = ["code_mode_host"]


def codex_interview_command(model: str, role: str, thread_id: str, effort: str = "low") -> list[str]:
    """`codex exec fork` of a finished agent thread, for its interview: the run's flags without the
    forum's MCP server, code mode's host off, a read-only sandbox, and `--ephemeral`. The fork sees
    the thread's history, and no rollout of it is kept: the thread's rollout, which the isolation
    check reads, stays as the run left it, and neither a rollout nor a session index entry is
    written for the fork (interview.codex_left checks both; Codex's own databases are not read).
    (`exec resume --ephemeral` appends the interview's turn to the
    thread's rollout and to Codex's thread history: Codex 0.159.2 honours `--ephemeral` only for a
    new thread.) The questions come on stdin (`-`)."""
    f = codex_flags(model, role, effort, None)
    for c in CODEX_INTERVIEW_CONFIG:
        f += ["-c", c]
    for feat in CODEX_INTERVIEW_DISABLED:
        f += ["--disable", feat]
    return ["codex", "exec", "fork", *f, "--ephemeral", thread_id, "-"]


def temp_root() -> Path:
    """The directory in which each agent session gets its own temporary working directory."""
    return Path(os.environ.get("TMPDIR", "/tmp")).resolve()


def workdir_for(name: str) -> Path:
    """A new agent session's working directory: unique to this process, agent, and moment."""
    return temp_root() / f"forum-agent-{os.getpid()}-{name}-{time.time_ns()}"


def is_workdir(path: Path, name: str) -> bool:
    """Whether `path` has the form of a working directory `workdir_for(name)` makes."""
    path = Path(path)
    return (
        path.is_absolute()
        and path.parent == temp_root()
        and re.fullmatch(rf"forum-agent-[0-9]+-{re.escape(name)}-[0-9]+", path.name) is not None
    )


def claude_home() -> Path:
    """Claude Code's own directory: `CLAUDE_CONFIG_DIR`, which the agents inherit, else `~/.claude`."""
    v = os.environ.get("CLAUDE_CONFIG_DIR")
    return Path(v).expanduser() if v else Path.home() / ".claude"


def claude_project_dir(workdir: str | Path) -> Path:
    """Where Claude Code keeps the sessions run in `workdir`: under its `projects/`, in a directory
    named after the path, every character but an ASCII letter or digit replaced by `-`."""
    return claude_home() / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(workdir))


def agent_env(extra: dict | None = None) -> dict:
    env = minimal_env()
    for k in AUTH_VARS:
        if k in os.environ:
            env[k] = os.environ[k]
    env.update(extra or {})
    return env


def split_tool(name: str) -> str | None:
    """`mcp__forum__hold` (Claude) or `hold` (Codex, with server `forum`) → `hold`."""
    if name.startswith("mcp__forum__"):
        return name[len("mcp__forum__") :]
    return name if name in FORUM_TOOLS else None


def result_text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(c.get("text", "") for c in content if isinstance(c, dict))
    if isinstance(content, dict):
        return result_text(content.get("content", []))
    return ""


class EndPoint:
    """A scenario's fixed point at which the session is ended mid-task.

    `{"after_calls": n}`: right after the response to the session's n-th forum call;
    `{"after_call": {"tool": t, "action": a, "ok": true, "with": arg}, "nth": k}`: right after the
    response to the k-th call that matches (`ok`: the call was not refused; `with`: it passed `arg`,
    such as a checkpoint's `intent`).
    """

    def __init__(self, spec: dict | None):
        self.spec = spec or {}
        self.matched = 0

    def reached(self, calls: list[dict]) -> bool:
        if not self.spec:
            return False
        if "after_calls" in self.spec:
            return len(calls) >= int(self.spec["after_calls"])
        want = self.spec.get("after_call", {})
        c = calls[-1]
        ok = not c.get("refused")
        if want.get("tool") and c.get("tool") != want["tool"]:
            return False
        if want.get("action") and (c.get("args") or {}).get("action") != want["action"]:
            return False
        if want.get("ok") and not ok:
            return False
        if want.get("with") and not (c.get("args") or {}).get(want["with"]):
            return False
        self.matched += 1
        return self.matched >= int(self.spec.get("nth", 1))


def is_refusal(text: str) -> bool:
    return any(line.startswith("error: ") for line in text.splitlines()[:4])


class Turn:
    def __init__(self, n: int, prompt: str):
        self.n = n
        self.prompt = prompt
        self.started = time.time()
        self.ended = None
        self.calls: list[dict] = []
        self.text = ""
        self.usage: dict = {}
        self.ended_by = None

    def as_dict(self) -> dict:
        return {
            "n": self.n,
            "prompt": self.prompt,
            "started": self.started,
            "ended": self.ended,
            "calls": self.calls,
            "text": self.text,
            "usage": self.usage,
            "ended_by": self.ended_by,
        }


class AgentSession:
    """The common part of both drivers: the transcript, the end point, and the limits."""

    vendor = "?"

    def __init__(
        self,
        name: str,
        model: str,
        mcp_url: str,
        token: str,
        role: str,
        out_dir: Path,
        end: dict | None = None,
        max_calls_per_turn: int = 40,
        turn_timeout: float = 600.0,
    ):
        self.name = name
        self.model = model
        self.mcp_url = mcp_url
        self.token = token
        self.role = role
        self.out = out_dir
        self.out.mkdir(parents=True, exist_ok=True)
        self.workdir = workdir_for(name)
        self.workdir.mkdir(parents=True)
        self.end = EndPoint(end)
        self.max_calls = max_calls_per_turn
        self.turn_timeout = turn_timeout
        self.turns: list[Turn] = []
        self.calls: list[dict] = []
        self.ended = False  # the session vanished (end point, limit) or exited
        # Why the harness killed the session from outside its turn loop (`kill(reason)`: the run's
        # time limit, an interruption): a turn the kill cuts ends with this reason, not `exit`.
        self.killed_by: str | None = None
        # The models the client reported using (Claude: each message's model and the result's
        # `modelUsage`), which `other_models` checks against the one asked for.
        self.models: set[str] = set()
        self.init: dict = {}
        self.raw = open(self.out / f"{name}.raw.jsonl", "a")

    def _record(self, line: str) -> None:
        self.raw.write(line.rstrip("\n") + "\n")
        self.raw.flush()

    def _add_call(self, turn: Turn, tool: str, args: dict, text: str, is_error: bool) -> None:
        c = {
            "i": len(self.calls) + 1,
            "t": time.time(),
            "tool": tool,
            "args": args,
            "text": text,
            "is_error": is_error,
            "refused": is_refusal(text),
        }
        self.calls.append(c)
        turn.calls.append(c)

    def client_session(self) -> dict | None:
        """The client's own id of this session, and whether the client kept it on disk so that
        the interview can resume it."""
        return None

    def _killed(self, reason: str | None) -> None:
        """Records why the session is killed, before the client is; the first reason stays."""
        if reason and not self.killed_by:
            self.killed_by = reason

    def transcript(self) -> dict:
        return {
            "agent": self.name,
            "vendor": self.vendor,
            "model": self.model,
            "init": self.init,
            "turns": [t.as_dict() for t in self.turns],
            "ended": self.ended,
            "killed_by": self.killed_by,
            "models": sorted(self.models),
            "workdir": str(self.workdir),
            "client_session": self.client_session(),
        }

    def _stderr(self):
        """The client's stderr log, opened for one process and closed with the session."""
        f = open(self.out / f"{self.name}.stderr.log", "a")
        self._files = getattr(self, "_files", []) + [f]
        return f

    def close(self) -> None:
        self.raw.close()
        for f in getattr(self, "_files", []):
            f.close()
        shutil.rmtree(self.workdir, ignore_errors=True)


class ClaudeSession(AgentSession):
    """One `claude -p --input-format stream-json` process for the whole agent session."""

    vendor = "claude"

    def __init__(self, *a, mcp_negotiation: str = "legacy", persist: bool = False, **kw):
        super().__init__(*a, **kw)
        if mcp_negotiation not in MCP_NEGOTIATIONS:
            raise ValueError(f"mcp_negotiation {mcp_negotiation!r}: expected one of {MCP_NEGOTIATIONS}")
        self.mcp_negotiation = mcp_negotiation
        # Whether the client keeps its record of the session on disk; what the model sees is the same.
        self.persist = persist
        self.session_id = str(uuid.uuid4()) if persist else None
        self.proc: subprocess.Popen | None = None
        self.lines: list[str] = []
        self.cv = threading.Condition()
        self.reader: threading.Thread | None = None

    def command(self) -> list[str]:
        mcp = {
            "mcpServers": {
                "forum": {
                    "type": "http",
                    "url": self.mcp_url,
                    "headers": {"Authorization": "Bearer ${FORUM_AGENT_TOKEN}"},
                }
            }
        }
        cfg = self.workdir.parent / f"{self.workdir.name}.mcp.json"
        cfg.write_text(json.dumps(mcp))
        self.mcp_config = cfg
        return [
            "claude",
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            self.model,
            "--mcp-config",
            str(cfg),
            "--strict-mcp-config",
            "--tools",
            ",".join(f"mcp__forum__{t}" for t in FORUM_TOOLS),
            "--allowedTools",
            ",".join(f"mcp__forum__{t}" for t in FORUM_TOOLS),
            "--permission-mode",
            "dontAsk",
            "--setting-sources",
            "",
            "--settings",
            json.dumps(CLAUDE_SETTINGS),
            "--disable-slash-commands",
            *(["--session-id", self.session_id] if self.persist else ["--no-session-persistence"]),
            "--append-system-prompt",
            self.role,
        ]

    def client_session(self) -> dict | None:
        sid = self.session_id or self.init.get("session_id")
        return {"id": sid, "persisted": self.persist} if sid else None

    def kept_record(self) -> Path | None:
        """Where the client keeps its record of this session, when it keeps one (`persist`)."""
        return claude_project_dir(self.workdir) / f"{self.session_id}.jsonl" if self.persist else None

    def isolation_env(self) -> dict:
        return claude_isolation_env(self.mcp_negotiation, self.model)

    def _start(self) -> None:
        env = agent_env(self.isolation_env() | {"FORUM_AGENT_TOKEN": self.token})
        self.proc = subprocess.Popen(
            self.command(),
            cwd=self.workdir,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr(),
            text=True,
            bufsize=1,
            start_new_session=True,
        )
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        assert self.proc and self.proc.stdout
        for line in self.proc.stdout:
            with self.cv:
                self.lines.append(line)
                self.cv.notify_all()
        with self.cv:
            self.lines.append("")  # EOF marker
            self.cv.notify_all()

    def _next_line(self, deadline: float) -> str | None:
        with self.cv:
            while not self.lines:
                left = deadline - time.monotonic()
                if left <= 0:
                    return None
                self.cv.wait(timeout=left)
            return self.lines.pop(0)

    def kill(self, reason: str | None = None) -> None:
        """Kills the client. A `reason` from outside the turn loop (the run's time limit, an
        interruption) becomes the ended_by of the turn the kill cuts."""
        self._killed(reason)
        if self.proc and self.proc.poll() is None:
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.proc.wait()
        self.ended = True

    def turn(self, prompt: str) -> Turn:
        t = Turn(len(self.turns) + 1, prompt)
        self.turns.append(t)
        if self.ended:
            t.ended_by = "gone"
            return t
        if self.proc is None:
            self._start()
        msg = {"type": "user", "message": {"role": "user", "content": prompt}}
        self._record(json.dumps({"harness": "prompt", "turn": t.n, "text": prompt}))
        assert self.proc and self.proc.stdin
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        pending: dict[str, tuple[str, dict]] = {}
        seen_usage: dict[str, dict] = {}
        said: list[str] = []
        deadline = time.monotonic() + self.turn_timeout
        while True:
            line = self._next_line(deadline)
            if line is None:
                t.ended_by = "timeout"
                self.kill()
                break
            if line == "":
                # The client exited: on its own, or killed from outside the turn loop.
                t.ended_by = self.killed_by or "exit"
                self.ended = True
                break
            self._record(line)
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            kind = ev.get("type")
            if kind == "system" and ev.get("subtype") == "init":
                self.init = ev
            elif kind == "assistant":
                m = ev.get("message", {})
                if m.get("model"):
                    self.models.add(str(m["model"]))
                if m.get("id") and isinstance(m.get("usage"), dict):
                    seen_usage[m["id"]] = m["usage"]
                for block in m.get("content", []):
                    if block.get("type") == "tool_use":
                        pending[block["id"]] = (block.get("name", ""), block.get("input") or {})
                    elif block.get("type") == "text" and block.get("text"):
                        # Every text of the turn is the reply: an ACTION line written before a later tool
                        # call is still a request to the platform (the lab's first round lost such lines).
                        said.append(block["text"])
            elif kind == "user":
                content = ev.get("message", {}).get("content", [])
                if not isinstance(content, list):
                    continue
                for block in content:
                    if block.get("type") != "tool_result":
                        continue
                    name, args = pending.pop(block.get("tool_use_id"), ("", {}))
                    tool = split_tool(name)
                    if tool is None:
                        continue
                    self._add_call(t, tool, args, result_text(block.get("content")), bool(block.get("is_error")))
                    if self.end.reached(self.calls):
                        t.ended_by = "end_point"
                        self.kill()
                        break
                    if len(t.calls) >= self.max_calls:
                        t.ended_by = "call_limit"
                        self.kill()
                        break
                if self.ended:
                    break
            elif kind == "result":
                t.text = "\n\n".join(said) if said else (ev.get("result") or "")
                self.models.update(str(k) for k in ev.get("modelUsage") or {})
                u = ev.get("usage") or {}
                # Claude Code reports the turn's tokens, and the session's cost so far.
                t.usage = {
                    "input_tokens": u.get("input_tokens", 0),
                    "output_tokens": u.get("output_tokens", 0),
                    "cache_read_tokens": u.get("cache_read_input_tokens", 0),
                    "cache_creation_tokens": u.get("cache_creation_input_tokens", 0),
                    "input_all_tokens": u.get("input_tokens", 0)
                    + u.get("cache_read_input_tokens", 0)
                    + u.get("cache_creation_input_tokens", 0),
                    "session_cost_usd": ev.get("total_cost_usd"),
                    "num_turns": ev.get("num_turns"),
                    "is_error": ev.get("is_error"),
                }
                t.ended_by = "result"
                break
        if not t.usage and seen_usage:
            # A turn ended before its result (an end point, a limit): the model requests'
            # own usage, one per message, without a cost.
            vals = list(seen_usage.values())
            s = lambda k: sum(int(v.get(k) or 0) for v in vals)  # noqa: E731
            t.usage = {
                "input_tokens": s("input_tokens"),
                "output_tokens": s("output_tokens"),
                "cache_read_tokens": s("cache_read_input_tokens"),
                "cache_creation_tokens": s("cache_creation_input_tokens"),
                "input_all_tokens": s("input_tokens") + s("cache_read_input_tokens") + s("cache_creation_input_tokens"),
                "session_cost_usd": None,
                "from": "messages",
            }
        t.ended = time.time()
        return t

    def finish(self) -> None:
        """Ends the session normally: stdin closes, and the client exits."""
        if self.proc and self.proc.poll() is None:
            try:
                assert self.proc.stdin
                self.proc.stdin.close()
                self.proc.wait(timeout=30)
            except (subprocess.TimeoutExpired, BrokenPipeError, OSError):
                self.kill()
        if getattr(self, "mcp_config", None):
            self.mcp_config.unlink(missing_ok=True)


class CodexSession(AgentSession):
    """`codex exec --json`, one process per turn, resumed by thread id."""

    vendor = "codex"

    def __init__(self, *a, direct_tools: bool = False, effort: str = "low", **kw):
        super().__init__(*a, **kw)
        self.thread_id: str | None = None
        self.proc: subprocess.Popen | None = None
        self.effort = effort
        self.direct_tools = direct_tools

    def flags(self) -> list[str]:
        return codex_flags(self.model, self.role, self.effort, self.mcp_url, direct=self.direct_tools)

    def client_session(self) -> dict | None:
        # Codex keeps every thread in its home's sessions/ (the run does not pass --ephemeral).
        return {"id": self.thread_id, "persisted": True, "effort": self.effort} if self.thread_id else None

    def command(self, prompt: str) -> list[str]:
        if self.thread_id is None:
            return ["codex", "exec", "-s", "read-only", "-C", str(self.workdir), *self.flags(), prompt]
        return ["codex", "exec", "resume", *self.flags(), self.thread_id, prompt]

    def kill(self, reason: str | None = None) -> None:
        """As ClaudeSession.kill."""
        self._killed(reason)
        if self.proc and self.proc.poll() is None:
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.proc.wait()
        self.ended = True

    def turn(self, prompt: str) -> Turn:
        t = Turn(len(self.turns) + 1, prompt)
        self.turns.append(t)
        if self.ended:
            t.ended_by = "gone"
            return t
        self._record(json.dumps({"harness": "prompt", "turn": t.n, "text": prompt}))
        env = agent_env({"FORUM_AGENT_TOKEN": self.token})
        self.proc = subprocess.Popen(
            self.command(prompt),
            cwd=self.workdir,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=self._stderr(),
            text=True,
            bufsize=1,
            start_new_session=True,
        )
        lines: list[str] = []
        cv = threading.Condition()

        def read():
            assert self.proc and self.proc.stdout
            for line in self.proc.stdout:
                with cv:
                    lines.append(line)
                    cv.notify_all()
            with cv:
                lines.append("")
                cv.notify_all()

        threading.Thread(target=read, daemon=True).start()
        deadline = time.monotonic() + self.turn_timeout
        texts: list[str] = []
        while True:
            with cv:
                while not lines:
                    left = deadline - time.monotonic()
                    if left <= 0:
                        break
                    cv.wait(timeout=left)
                line = lines.pop(0) if lines else None
            if line is None:
                t.ended_by = "timeout"
                self.kill()
                break
            if line == "":
                self.proc.wait()
                t.ended_by = t.ended_by or self.killed_by or ("result" if self.proc.returncode == 0 else "exit")
                break
            self._record(line)
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            kind = ev.get("type")
            if kind == "thread.started":
                self.thread_id = ev.get("thread_id") or self.thread_id
                self.init = self.init or ev
            elif kind == "item.completed":
                item = ev.get("item") or {}
                itype = item.get("type") or item.get("item_type")
                if itype == "mcp_tool_call" and item.get("server") == "forum":
                    res = item.get("result") or {}
                    text = result_text(res.get("content", [])) if isinstance(res, dict) else ""
                    err = item.get("error")
                    if err and not text:
                        text = f"error: client: {err}"
                    args = item.get("arguments") or {}
                    if isinstance(args, str):
                        try:
                            args = json.loads(args)
                        except json.JSONDecodeError:
                            args = {"_raw": args}
                    self._add_call(t, item.get("tool", ""), args, text, bool(err))
                    if self.end.reached(self.calls):
                        t.ended_by = "end_point"
                        self.kill()
                        break
                    if len(t.calls) >= self.max_calls:
                        t.ended_by = "call_limit"
                        self.kill()
                        break
                elif itype == "agent_message":
                    texts.append(item.get("text", ""))
            elif kind == "turn.completed":
                # Codex reports the turn's tokens; its input count includes the cached part.
                u = ev.get("usage") or {}
                t.usage = {
                    "input_tokens": u.get("input_tokens", 0),
                    "output_tokens": u.get("output_tokens", 0),
                    "cache_read_tokens": u.get("cached_input_tokens", 0),
                    "cache_creation_tokens": 0,
                    "input_all_tokens": u.get("input_tokens", 0),
                    "session_cost_usd": None,
                }
                t.ended_by = "result"
            elif kind in ("turn.failed", "error"):
                t.ended_by = "error"
        # Every message of the turn, as for Claude: an ACTION line may come before a later tool call.
        t.text = "\n\n".join(x for x in texts if x)
        t.ended = time.time()
        return t

    def finish(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.kill()


class ScriptedSession(AgentSession):
    """A scripted agent for testing the harness without a model: each turn makes the calls of the
    next entry of `script` (`[[tool, args], ...]`) through the same proxy. `$session` and `$token`
    in an argument are replaced by the forum session of the first response and the last token a
    response returned."""

    vendor = "script"

    def __init__(self, *a, script: list | None = None, **kw):
        super().__init__(*a, **kw)
        self.script = list(script or [])
        self.client = None
        self.vars: dict[str, str] = {}

    def kill(self, reason: str | None = None) -> None:
        self._killed(reason)
        self.ended = True

    def turn(self, prompt: str) -> Turn:
        from .mcp import McpClient  # local: only the test vendor needs it

        t = Turn(len(self.turns) + 1, prompt)
        self.turns.append(t)
        if self.ended or not self.script:
            t.ended_by = "gone" if self.ended else "result"
            return t
        if self.client is None:
            self.client = McpClient(self.mcp_url, self.token)
            self.client.initialize()
            self.client.list_tools()  # as a client does, so that the run saves what it was served
        for tool, args in self.script.pop(0):
            args = {
                k: self.vars.get(v[1:], v) if isinstance(v, str) and v.startswith("$") else v for k, v in args.items()
            }
            r = self.client.call(tool, args)
            found = status_session(r.text)
            if found:
                self.vars.setdefault("session", found[1])
            if isinstance(r.structured, dict) and r.structured.get("token"):
                self.vars["token"] = r.structured["token"]
            self._add_call(t, tool, args, r.text, r.is_error)
            if self.end.reached(self.calls):
                t.ended_by = "end_point"
                self.kill()
                break
        t.ended_by = t.ended_by or "result"
        t.text = "done"
        t.ended = time.time()
        return t

    def finish(self) -> None:
        if self.client:
            self.client.close()


def make(vendor: str, *a, **kw) -> AgentSession:
    if vendor != "script":
        kw.pop("script", None)
    if vendor != "claude":
        kw.pop("mcp_negotiation", None)
        kw.pop("persist", None)
    if vendor != "codex":
        kw.pop("direct_tools", None)
        kw.pop("effort", None)
    return {"claude": ClaudeSession, "codex": CodexSession, "script": ScriptedSession}[vendor](*a, **kw)


def backfill_usage(transcript: dict, raw_path: Path) -> dict:
    """Fills a Claude turn's missing usage (a turn ended before its result) from the per-message
    usage in the client's raw stream, as the driver now does while it runs."""
    if transcript.get("vendor") != "claude" or not raw_path.exists():
        return transcript
    per_turn: dict[int, dict[str, dict]] = {}
    turn = 0
    for line in raw_path.read_text().splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("harness") == "prompt":
            turn = ev.get("turn", turn + 1)
        elif ev.get("type") == "assistant":
            m = ev.get("message", {})
            if m.get("id") and isinstance(m.get("usage"), dict):
                per_turn.setdefault(turn, {})[m["id"]] = m["usage"]
    for t in transcript.get("turns", []):
        if t.get("usage") or t["n"] not in per_turn:
            continue
        vals = list(per_turn[t["n"]].values())
        s = lambda k: sum(int(v.get(k) or 0) for v in vals)  # noqa: E731
        t["usage"] = {
            "input_tokens": s("input_tokens"),
            "output_tokens": s("output_tokens"),
            "cache_read_tokens": s("cache_read_input_tokens"),
            "cache_creation_tokens": s("cache_creation_input_tokens"),
            "input_all_tokens": s("input_tokens") + s("cache_read_input_tokens") + s("cache_creation_input_tokens"),
            "session_cost_usd": None,
            "from": "messages",
        }
    return transcript


# ------------------------------------------------------------------------------------------------
# Isolation check


def claude_isolation_problems(init: dict) -> list[str]:
    """What a Claude session's `init` event shows that is not the forum alone."""
    out = []
    tools = init.get("tools", [])
    extra = [t for t in tools if split_tool(t) is None]
    if extra:
        out.append(f"tools other than the forum's: {extra}")
    missing = [t for t in FORUM_TOOLS if f"mcp__forum__{t}" not in tools]
    if missing:
        out.append(f"forum tools missing: {missing}")
    servers = [s.get("name") for s in init.get("mcp_servers", [])]
    if servers != ["forum"]:
        out.append(f"MCP servers: {servers}")
    for s in init.get("mcp_servers", []):
        if s.get("status") != "connected":
            out.append(f"MCP server {s.get('name')} is {s.get('status')}")
    for key in ("agents", "skills", "plugins", "slash_commands"):
        v = init.get(key)
        if v:
            out.append(f"{key}: {v}")
    return out
