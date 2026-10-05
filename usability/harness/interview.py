"""The interview after a run: each agent session, resumed once in its own client, is asked a fixed
debrief, and the answers are stored next to the run's slips.

It runs after the slip counter (runner.py), so it cannot change what was counted: the server is
stopped, the resumed client has no MCP server (and Claude no tool; Codex as few as its CLI
allows), and nothing it says reaches the run's history, the proxy's log, or the transcripts the
counter reads. Its own files are `interview.json`
in the run directory and `agents/<id>.interview.raw.jsonl` and `.interview.stderr.log`, and for
Claude, the copy of the session the client kept (`agents/<id>.session.jsonl`).

- Claude: `claude -p --resume <session id>` in the session's own working directory (made again,
  empty), with the run's isolation, no MCP server, no tool, `--no-session-persistence`, so that
  the session as the client keeps it ends where the run ended it, and a budget
  (`agents.claude_interview_command`). Only a session the client kept on disk can be resumed: the
  run has the client keep every session (`persist`; runs made before R3's fixes kept only those of
  runs chosen for the interview), in a directory of the client's own named after the session's
  temporary working directory, and records that it did (`agents/<id>.client-session.json`,
  `write_record`). After the interview, or as a run not interviewed finishes, a copy goes into the
  run directory (`agents/<id>.session.jsonl`) and that directory is removed (`keep_and_remove`),
  under a guard that refuses any path but the one derived from the run's own working directory of
  that agent (`session_dir`). A later interview of the same run puts the copy back first
  (`put_back`). A run that stopped before it could do this (a crash, an interruption) is tidied
  the same way when the harness next starts (`tidy_orphans`).
- Codex: `codex exec fork <thread id> --ephemeral` with the run's flags, no MCP server, code
  mode's host off, and a read-only sandbox (`agents.codex_interview_command`): the fork sees the
  thread's history, and Codex keeps no rollout of it, so the thread's rollout, which the isolation
  check reads, stays as the run left it. Each interview checks that (`codex_left`). The CLI reports
  no list of the tools it offers and cannot turn them all off; a tool the model calls is recorded
  (`tools_problems`). A fork reports the token usage of the whole thread, the run's included: the
  interview's own is what it adds to the thread's last total in the rollout (`codex_own_usage`).

A client writes its record of a session behind the session itself, so a session the harness killed
mid-task may have lost its last calls: the interview counts the forum calls whose results the
client kept (`calls_kept`), and the report sets them against the calls the run made, marking the
answers about what the session did as unreliable when the client kept fewer. A session the harness
stopped right after a forum call (a fixed point, the call limit) is told, after the preamble, how
many calls it made and which, from the proxy's log (`stopped_note`): what it did, not what the
calls returned.

One turn per agent session. A session that cannot be resumed (the client did not keep it, or no
longer finds it) is recorded as such, with the client's own words.

The questions are few and neutral: they do not name the protocol's rules, hint at what the agent
missed, or presuppose that something went wrong. They come from the Wizard-of-Oz rounds, whose
interviews asked what was convenient (praise), what was confusing (complaints), and what the agent
would change. The rounds' rating out of 10 is left out: their reports found that self-ratings did
not track outcomes. Both arms are asked the same questions, so that their answers compare; a
session that the harness ended mid-task is also asked what it would have done next, in words that
assert nothing about what it last received. The answers are quoted in the report, cut to a bounded
length; nothing summarises them.
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
from pathlib import Path

from . import agents as A
from . import isolation as ISO

# Each question: an id (the key of its answer), a short label for the report, its words, and when it
# is asked: `always`, or `cut` (a session the harness ended mid-task). The same in both arms.
QUESTIONS = [
    {
        "id": "task",
        "label": "What it was doing",
        "when": "always",
        "text": "What were you trying to do in this session, and how far did you get?",
    },
    {
        "id": "convenient",
        "label": "Convenient",
        "when": "always",
        "text": "What about the tools was convenient or easy to use?",
    },
    {
        "id": "confusing",
        "label": "Confusing or slow",
        "when": "always",
        "text": "What was confusing or slow?",
    },
    {
        "id": "unclear",
        "label": "Unclear",
        "when": "always",
        "text": "Was anything unclear about what to pass to a tool or what a response meant?",
    },
    {
        "id": "next",
        "label": "Deciding what to do next",
        "when": "always",
        "text": "After each tool response, how did you decide what to do next?",
    },
    {
        "id": "change",
        "label": "Would change",
        "when": "always",
        "text": "What would you change about the tools?",
    },
    {
        "id": "cut",
        "label": "Would have done next",
        "when": "cut",
        "text": "The platform stopped your session before you finished. What would you have done next?",
    },
]

PREAMBLE = (
    "The task is over. This is a short debrief for the people who make these tools: nothing you write "
    "now reaches anyone in the task, and you have no tools now. Answer each question under its number, "
    "in plain words, in at most three sentences each. If a question does not apply, or you are not sure, "
    "say so; there are no right answers."
)

# The harness ended the session mid-task: a scenario's fixed point, the call limit, a turn's timeout,
# or the run's time limit (agents.AgentSession.kill(reason), runner.py).
CUT_BY = ("end_point", "call_limit", "timeout", "run_timeout")
# Of those, the ways that kill the client right after the response to a forum call, before it has
# written that call to its own record of the session: R3's four Claude sessions cut after 3 calls
# kept 1 or 2, and on resuming each model took the end of its record for a stop of its own. Such a
# session is told, from the proxy's log, which calls it made (`stopped_note`); it is a fact about
# the run, not a hint, and question 7 already says that the session was stopped.
STOPPED_AFTER_CALL = ("end_point", "call_limit")
STOPPED_NOTE = (
    "The platform stopped your session after your {nth} forum call ({calls}); your own record may lack "
    "the last results."
)
# How many of the session's last calls the note names.
STOPPED_NOTE_CALLS = 10
# The questions about what the session did, whose answers rest on the client's record of it: the
# report marks them unreliable when the record lost calls (`calls_kept` below the calls made).
ABOUT_WHAT_IT_DID = ("task", "next")
TIMEOUT_S = 300.0
# What a Claude interview may spend beyond what the session had cost (`--max-budget-usd`). One turn
# with no tool is one request, which the client lets finish; the budget bounds anything more.
BUDGET_USD = 0.50
INTERVIEWED_VENDORS = ("claude", "codex")


# -- which runs ------------------------------------------------------------------------------------


def parse_policy(value: str) -> int | None:
    """`--interview`: `all` (None: every run), `none` (0), or `N`, the first N runs of each cell."""
    v = str(value).strip().lower()
    if v == "all":
        return None
    if v == "none":
        return 0
    if v.isdigit():
        return int(v)
    raise ValueError(f"--interview {value!r}: expected all, none, or a number of runs per cell")


def chosen(k: int, policy: int | None, vendor: str = "claude") -> bool:
    """Whether run `k` (1-based, within its cell: one model, scenario, and arm) is interviewed. A
    scripted agent has no model to ask."""
    if vendor not in INTERVIEWED_VENDORS:
        return False
    return policy is None or k <= policy


def run_index(run_dir: Path) -> int:
    m = re.fullmatch(r"run-(\d+)", Path(run_dir).name)
    return int(m.group(1)) if m else 0


# -- the questions ---------------------------------------------------------------------------------


def cut_by(transcript: dict) -> str | None:
    """How the harness ended the session mid-task, or None when it ended on its own."""
    for t in transcript.get("turns", []):
        if t.get("ended_by") in CUT_BY:
            return t["ended_by"]
    return None


def questions_for(arm: str, cut: bool) -> list[dict]:
    """The questions as asked, numbered. `arm` changes nothing: both arms are asked the same."""
    out = []
    for q in QUESTIONS:
        if q["when"] == "always" or (q["when"] == "cut" and cut):
            out.append({"n": len(out) + 1, "id": q["id"], "label": q["label"], "text": q["text"]})
    return out


def prompt(questions: list[dict], note: str | None = None) -> str:
    """The preamble, the note on a session stopped after a call when there is one, and the
    questions."""
    parts = [PREAMBLE] + ([note] if note else []) + ["\n".join(f"{q['n']}. {q['text']}" for q in questions)]
    return "\n\n".join(parts)


def ordinal(n: int) -> str:
    suffix = "th" if 10 <= n % 100 <= 20 else {1: "st", 2: "nd", 3: "rd"}.get(n % 10, "th")
    return f"{n}{suffix}"


def call_label(tool: str, args) -> str:
    """A forum call as the note names it: its tool and action (`hold take`)."""
    action = args.get("action") if isinstance(args, dict) else None
    return f"{tool} {action}" if isinstance(action, str) and action else str(tool)


def logged_calls(run_dir: Path) -> dict[str, list[str]] | None:
    """Each agent's forum calls in the order the proxy logged them (`hold take`), or None when the
    run has no proxy log. The proxy logs every call the server answered, whatever the client then
    kept of it."""
    p = Path(run_dir) / "proxy.jsonl"
    if not p.is_file():
        return None
    out: dict[str, list[str]] = {}
    for ev in _events(p.read_text(errors="replace")):
        if ev.get("agent") and ev.get("tool"):
            out.setdefault(str(ev["agent"]), []).append(call_label(str(ev["tool"]), ev.get("args")))
    return out


def stopped_note(calls: list[str]) -> str | None:
    """The note on a session the harness stopped right after a forum call: how many calls it made,
    and which (the last `STOPPED_NOTE_CALLS` of them)."""
    if not calls:
        return None
    shown = calls[-STOPPED_NOTE_CALLS:]
    listed = ("…, " if len(calls) > len(shown) else "") + ", ".join(shown)
    return STOPPED_NOTE.format(nth=ordinal(len(calls)), calls=listed)


NUMBER = re.compile(r"^[ \t>#*_]*(\d{1,2})[ \t]*[.):](?:\*\*|__)?[ \t]*", re.M)
ECHO = re.compile(r"^(\*\*|__)(.{1,240}?)\1[ \t]*:?[ \t]*\n?", re.S)


def split_answers(text: str, questions: list[dict]) -> dict[str, str]:
    """The answer to each question, by its id, from a reply numbered as asked. The numbers are taken
    in order, each the first line that starts with it after the previous one; a question echoed in
    bold before its answer is dropped. An empty dict when the reply is not numbered."""
    marks = []
    pos = 0
    for q in questions:
        for m in NUMBER.finditer(text or "", pos):
            if int(m.group(1)) == q["n"]:
                marks.append((q, m.start(), m.end()))
                pos = m.end()
                break
    out: dict[str, str] = {}
    for i, (q, _start, end) in enumerate(marks):
        stop = marks[i + 1][1] if i + 1 < len(marks) else len(text)
        a = text[end:stop].strip()
        e = ECHO.match(a)
        if e and e.group(2).rstrip().endswith(("?", ":")):
            a = a[e.end() :].strip()
        out[q["id"]] = a
    return out


def cut(text: str, limit: int) -> str:
    """The text on one line, cut at a word before `limit` characters, the cut marked."""
    t = " ".join((text or "").split())
    if len(t) <= limit:
        return t
    head = t[:limit].rsplit(" ", 1)[0] if " " in t[:limit] else t[:limit]
    return head.rstrip(" ,;:") + " […]"


# -- one session -----------------------------------------------------------------------------------


def _killpg(proc: subprocess.Popen) -> None:
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    proc.wait()


def run_client(
    cmd: list[str],
    cwd: Path,
    env: dict,
    stdin: str,
    raw: Path,
    err: Path,
    timeout: float,
    stop: threading.Event | None = None,
):
    """Runs a client once: the questions on stdin, its stream into `raw`. (return code or None when
    it was stopped: on a timeout, or when `stop` was set, the stream's lines). The client runs in a
    process group of its own, which an interruption of the harness would not reach: it is killed
    whenever the harness stops waiting for it."""
    with open(err, "a") as ferr:
        proc = subprocess.Popen(
            cmd,
            cwd=cwd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=ferr,
            text=True,
            start_new_session=True,
        )
        out: list[str] = []

        def read():
            assert proc.stdout
            for line in proc.stdout:
                out.append(line)

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        try:
            try:
                assert proc.stdin
                proc.stdin.write(stdin)
                proc.stdin.close()
            except (BrokenPipeError, OSError):
                pass
            deadline = time.monotonic() + timeout
            rc = None
            while True:
                left = deadline - time.monotonic()
                if left <= 0 or (stop is not None and stop.is_set()):
                    _killpg(proc)
                    break
                try:
                    rc = proc.wait(timeout=min(0.5, left))
                    break
                except subprocess.TimeoutExpired:
                    continue
        except BaseException:
            _killpg(proc)
            raise
        reader.join(timeout=5)
        if proc.stdout:
            proc.stdout.close()
    with open(raw, "a") as f:
        f.write(json.dumps({"harness": "interview", "cmd": cmd[:3], "cwd": str(cwd)}) + "\n")
        for line in out:
            f.write(line.rstrip("\n") + "\n")
    return rc, out


def read_claude(lines: list[str]) -> dict:
    r: dict = {"text": "", "usage": {}, "cost_usd": None, "tools_offered": None, "mcp_servers": None}
    texts: list[str] = []
    tool_uses: list[str] = []
    models: set[str] = set()
    result = None
    for line in lines:
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "system" and ev.get("subtype") == "init":
            r["tools_offered"] = ev.get("tools")
            r["mcp_servers"] = [s.get("name") for s in ev.get("mcp_servers") or []]
            r["client_session"] = ev.get("session_id")
        elif ev.get("type") == "assistant":
            m = ev.get("message") or {}
            if m.get("model"):
                models.add(str(m["model"]))
            for b in m.get("content") or []:
                if b.get("type") == "text":
                    texts.append(b.get("text", ""))
                elif b.get("type") == "tool_use":
                    tool_uses.append(b.get("name", ""))
        elif ev.get("type") == "result":
            result = ev
    r["tool_calls"] = tool_uses
    if result is not None:
        models.update(str(k) for k in result.get("modelUsage") or {})
        u = result.get("usage") or {}
        r["usage"] = {
            "input_tokens": u.get("input_tokens", 0),
            "output_tokens": u.get("output_tokens", 0),
            "cache_read_tokens": u.get("cache_read_input_tokens", 0),
            "cache_creation_tokens": u.get("cache_creation_input_tokens", 0),
            "input_all_tokens": u.get("input_tokens", 0)
            + u.get("cache_read_input_tokens", 0)
            + u.get("cache_creation_input_tokens", 0),
        }
        r["cost_usd"] = result.get("total_cost_usd")
        r["subtype"] = result.get("subtype")
        # Over its budget, the client reports no result text, but the answer is in its messages.
        r["text"] = result.get("result") or "\n".join(texts)
        if result.get("is_error"):
            r["errors"] = [str(e) for e in result.get("errors") or []] or [str(result.get("subtype"))]
    else:
        r["text"] = "\n".join(texts)
        r["errors"] = ["the client gave no result"]
    r["models"] = sorted(models)
    return r


# What Codex says when code mode's host is off, as the interview asks (agents.CODEX_INTERVIEW_DISABLED).
CODE_MODE_OFF = "Code Mode is unavailable because code-mode host is disabled"


def codex_usage(u: dict) -> dict:
    """Codex's token counts (`turn.completed`'s usage, or a rollout's `total_token_usage`) in the
    harness's terms; Codex's input count includes the cached part."""
    return {
        "input_tokens": int(u.get("input_tokens") or 0),
        "output_tokens": int(u.get("output_tokens") or 0),
        "cache_read_tokens": int(u.get("cached_input_tokens") or 0),
        "cache_creation_tokens": 0,
        "input_all_tokens": int(u.get("input_tokens") or 0),
    }


def read_codex(lines: list[str]) -> dict:
    # Codex reports no list of the tools it offers: `tools_offered` stays None (unknown). Its usage
    # is the thread's total, the run's included (`codex_own_usage`).
    r: dict = {"text": "", "usage": {}, "cost_usd": None, "tools_offered": None, "mcp_servers": []}
    texts: list[str] = []
    tool_uses: list[str] = []
    errors: list[str] = []
    for line in lines:
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = ev.get("type")
        if kind == "thread.started":
            r["client_session"] = ev.get("thread_id")
        elif kind == "item.completed":
            item = ev.get("item") or {}
            itype = item.get("type") or item.get("item_type")
            if itype == "agent_message":
                texts.append(item.get("text", ""))
            elif itype == "error" and str(item.get("message", "")).startswith(CODE_MODE_OFF):
                r["code_mode_off"] = True
            elif itype == "error":
                errors.append(str(item.get("message") or item)[:300])
            elif itype not in ("reasoning", None):
                tool_uses.append(str(item.get("tool") or itype))
        elif kind == "turn.completed":
            r["usage"] = codex_usage(ev.get("usage") or {})
        elif kind in ("turn.failed", "error"):
            errors.append(json.dumps(ev.get("error") or ev.get("message") or ev)[:300])
    r["text"] = texts[-1] if texts else ""
    r["tool_calls"] = tool_uses
    if errors:
        r["errors"] = errors
    return r


NOT_FOUND = ("No conversation found", "no rollout found")


# -- what the client kept of the run ---------------------------------------------------------------

# A client writes its record of a session behind the session: a session killed mid-task can lose its
# last calls there, and the resumed model then knows nothing of them. These count the forum calls
# whose results the record holds.


def _events(data: str | bytes):
    if isinstance(data, bytes):
        data = data.decode("utf-8", errors="replace")
    for line in data.splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(ev, dict):
            yield ev


def claude_calls_kept(session_jsonl: str | bytes) -> int:
    """The forum calls whose results a Claude session file holds: `tool_result` blocks answering a
    `tool_use` of an `mcp__forum__` tool."""
    forum_ids: set[str] = set()
    kept = 0
    for ev in _events(session_jsonl):
        content = (ev.get("message") or {}).get("content")
        if not isinstance(content, list):
            continue
        for b in content:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "tool_use" and str(b.get("name", "")).startswith("mcp__forum__"):
                forum_ids.add(str(b.get("id")))
            elif b.get("type") == "tool_result" and str(b.get("tool_use_id")) in forum_ids:
                forum_ids.discard(str(b.get("tool_use_id")))
                kept += 1
    return kept


def codex_calls_kept(rollouts: dict[str, bytes]) -> int:
    """The forum calls whose results a Codex rollout holds: the forum tools a code cell called
    (`tools.mcp__forum__<tool>(...)`) or the model called directly, for each call whose output the
    rollout records."""
    pending: dict[str, int] = {}
    kept = 0
    for _path, data in sorted(rollouts.items()):
        for ev in _events(data):
            pl = ev.get("payload")
            if not isinstance(pl, dict):
                continue
            kind = pl.get("type")
            if kind == "custom_tool_call":
                n = sum(1 for t in ISO.CODE_CALL.findall(str(pl.get("input") or "")) if t.startswith("mcp__forum__"))
                pending[str(pl.get("call_id"))] = n
            elif kind == "function_call" and str(pl.get("name", "")).startswith("mcp__forum__"):
                pending[str(pl.get("call_id"))] = 1
            elif kind in ("custom_tool_call_output", "function_call_output"):
                kept += pending.pop(str(pl.get("call_id")), 0)
    return kept


def codex_total_before(rollouts: dict[str, bytes]) -> dict | None:
    """The thread's last cumulative token count in its rollout (a `token_count` event's
    `total_token_usage`), in the harness's terms; None when the rollout has none."""
    last = None
    for _path, data in sorted(rollouts.items()):
        for ev in _events(data):
            pl = ev.get("payload")
            if isinstance(pl, dict) and pl.get("type") == "token_count":
                total = (pl.get("info") or {}).get("total_token_usage")
                if isinstance(total, dict):
                    last = total
    return codex_usage(last) if last is not None else None


USAGE_KEYS = ("input_tokens", "output_tokens", "cache_read_tokens", "input_all_tokens")


def codex_own_usage(reported: dict, before: dict | None) -> tuple[dict, str | None]:
    """The interview's own tokens: what the fork reported (the thread's total) less the thread's
    total before it. (usage, a note when it cannot be told apart)."""
    if not reported:
        return {}, None
    if before is None:
        return {}, "the client reports the thread's total, and no total before the interview was found"
    if any(int(reported.get(k) or 0) < int(before.get(k) or 0) for k in USAGE_KEYS):
        return dict(reported), "the client's figures are below the thread's total before: taken as the interview's own"
    own = {k: int(reported.get(k) or 0) - int(before.get(k) or 0) for k in USAGE_KEYS}
    own["cache_creation_tokens"] = 0
    return own, None


def codex_usage_before(rollouts: dict[str, bytes], transcript: dict) -> dict | None:
    """The thread's total before the interview: the rollout's last, or else the last a turn of the
    run reported (Codex reports a turn's usage as the thread's total)."""
    before = codex_total_before(rollouts)
    if before is not None:
        return dict(before, source="rollout")
    reported = [t["usage"] for t in transcript.get("turns", []) if (t.get("usage") or {}).get("input_tokens")]
    if reported:
        return dict({k: int(reported[-1].get(k) or 0) for k in USAGE_KEYS}, cache_creation_tokens=0, source="transcript")
    return None


# -- what the client keeps of a Claude session -----------------------------------------------------

# Claude Code keeps a session as `<session id>.jsonl` (and, for some sessions, a directory of the
# same name) in a directory of its own under `projects/`, named after the session's working
# directory. A run makes it there; after the interview, or as a run not interviewed finishes, the
# harness keeps a copy in the run directory and removes that directory, and touches nothing else of
# the client's.
SESSION_ID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
# Claude Code shortens a longer name with a hash, which the harness does not reproduce.
PROJECT_NAME_MAX = 200


claude_home = A.claude_home
claude_project_dir = A.claude_project_dir


def session_dir(agent: str, workdir: str | None, session_id: str | None) -> tuple[Path | None, str | None]:
    """The client's directory of this run's session of `agent`, or why the harness may not touch
    it: (path, None) or (None, the reason). The path must be the one derived from the run's own
    temporary working directory of that agent (`agents.is_workdir`), and the directory may hold
    nothing but that session's files, as the client writes them: `<id>.jsonl`, a regular file, and
    `<id>/`, a directory. Anything else could be another session's, or not a session at all."""
    if not session_id or not SESSION_ID.fullmatch(session_id):
        return None, f"the session id {session_id!r} is not one the harness chose"
    if not workdir or not A.is_workdir(Path(workdir), agent):
        return None, f"{workdir!r} is not the run's own temporary working directory of agent {agent}"
    target = claude_project_dir(workdir)
    if len(target.name) > PROJECT_NAME_MAX:
        return None, f"the client shortens a name of more than {PROJECT_NAME_MAX} characters with a hash"
    if target.is_symlink() or (target.exists() and not target.is_dir()):
        return None, f"{target} is not a plain directory"
    if target.exists():
        mine = {f"{session_id}.jsonl", session_id}
        other = sorted(e.name for e in target.iterdir() if e.name not in mine)
        if other:
            return None, f"{target} holds files that are not this session's: {other[:5]}"
        for e in target.iterdir():
            plain = (e.is_file() if e.name.endswith(".jsonl") else e.is_dir()) and not e.is_symlink()
            if not plain:
                return None, f"{e} is not what the client writes for a session"
    return target, None


def _copy_name(agent: str) -> str:
    return f"{agent}.session"


def _tree(root: Path) -> dict[str, tuple]:
    """Every entry under `root`, by relative path: a file's bytes, a symlink's target, a directory."""
    out: dict[str, tuple] = {}
    for p in sorted(root.rglob("*")):
        rel = str(p.relative_to(root))
        if p.is_symlink():
            out[rel] = ("link", os.readlink(p))
        elif p.is_dir():
            out[rel] = ("dir",)
        else:
            out[rel] = ("file", p.read_bytes())
    return out


def keep_and_remove(agent: str, workdir: str | None, session_id: str | None, agents_dir: Path) -> dict:
    """After the interview: copies what the client kept of the session into the run directory
    (`agents/<id>.session.jsonl`, and the session's own directory, if any, as `agents/<id>.session/`),
    checks each copy against its source, and only then removes the client's directory of it, which
    the run made. Touches nothing when `session_dir` refuses, and removes nothing unless every entry
    was copied and checked."""
    target, why = session_dir(agent, workdir, session_id)
    rec: dict = {"path": str(target) if target else None, "copied": [], "removed": False}
    if why:
        rec["refused"] = why
        return rec
    assert target is not None and session_id is not None
    if not target.exists():
        rec["absent"] = True
        return rec
    src = target / f"{session_id}.jsonl"
    if src.is_file():
        dst = agents_dir / f"{_copy_name(agent)}.jsonl"
        shutil.copyfile(src, dst)
        if dst.read_bytes() != src.read_bytes():
            rec["refused"] = f"the copy {dst.name} differs from {src}; nothing removed"
            return rec
        rec["copied"].append(dst.name)
    sub = target / session_id
    if sub.is_dir():
        dst = agents_dir / _copy_name(agent)
        if dst.exists():
            shutil.rmtree(dst)
        shutil.copytree(sub, dst, symlinks=True)
        if _tree(dst) != _tree(sub):
            rec["refused"] = f"the copy {dst.name}/ differs from {sub}; nothing removed"
            return rec
        rec["copied"].append(dst.name + "/")
    # Every entry the guard allowed is copied and checked; anything else would have been refused.
    left = sorted(e.name for e in target.iterdir() if e.name not in (src.name, sub.name))
    if left:
        rec["refused"] = f"{target} holds entries that were not copied: {left[:5]}; nothing removed"
        return rec
    shutil.rmtree(target)
    rec["removed"] = True
    return rec


def put_back(agent: str, workdir: str | None, session_id: str | None, agents_dir: Path) -> bool:
    """Before interviewing a session whose client directory an earlier interview removed (`run.sh
    interview --again`): puts the run directory's copy back where the client looks for it, under
    the same guard. Whether it did."""
    copy = agents_dir / f"{_copy_name(agent)}.jsonl"
    target, why = session_dir(agent, workdir, session_id)
    if why or target is None or not copy.is_file() or (target / f"{session_id}.jsonl").exists():
        return False
    target.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(copy, target / f"{session_id}.jsonl")
    sub = agents_dir / _copy_name(agent)
    if sub.is_dir() and not (target / str(session_id)).exists():
        shutil.copytree(sub, target / str(session_id), symlinks=True)
    return True


# A run records each Claude session the client keeps, before the client starts, so that a run that
# stops before its end (a crash, an interruption, a kill) can still be tidied: by its own `finish`,
# or else when the harness next starts (`tidy_orphans`).
RECORD_SUFFIX = ".client-session.json"


def write_record(agents_dir: Path, agent: str, vendor: str, workdir: str, session_id: str) -> Path:
    agents_dir.mkdir(parents=True, exist_ok=True)
    p = agents_dir / f"{agent}{RECORD_SUFFIX}"
    rec = {
        "agent": agent,
        "vendor": vendor,
        "workdir": str(workdir),
        "session_id": session_id,
        "pid": os.getpid(),
        "written": time.time(),
    }
    p.write_text(json.dumps(rec, indent=1))
    return p


def records(run_dir: Path) -> dict[str, dict]:
    """The run's records of the Claude sessions its client kept, by agent."""
    out = {}
    for p in sorted((Path(run_dir) / "agents").glob(f"*{RECORD_SUFFIX}")):
        try:
            rec = json.loads(p.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(rec, dict) and rec.get("agent"):
            out[str(rec["agent"])] = dict(rec, _path=str(p))
    return out


def _mark_tidied(rec: dict, result: dict) -> None:
    p = Path(rec["_path"])
    data = {k: v for k, v in rec.items() if k != "_path"}
    data["tidied"] = result
    p.write_text(json.dumps(data, indent=1))


def tidy_run(run_dir: Path, transcripts: dict | None = None) -> dict:
    """`keep_and_remove` for every Claude session of a run that the client kept, from the run's
    records and its transcripts: for a run that was not interviewed (it was not chosen, it failed
    or was interrupted, or the interview failed), so that it leaves nothing in the client's
    directory either. Each record is marked as tidied."""
    agents_dir = Path(run_dir) / "agents"
    agents_dir.mkdir(parents=True, exist_ok=True)
    sessions: dict[str, tuple[str | None, str | None]] = {}
    recs = records(run_dir)
    for aid, rec in recs.items():
        if rec.get("vendor") == "claude":
            sessions[aid] = (rec.get("workdir"), rec.get("session_id"))
    for aid, tr in (transcripts or {}).items():
        cs = tr.get("client_session") or {}
        if tr.get("vendor") == "claude" and cs.get("persisted"):
            sessions.setdefault(aid, (tr.get("workdir"), cs.get("id")))
    out = {}
    for aid in sorted(sessions):
        workdir, sid = sessions[aid]
        out[aid] = keep_and_remove(aid, workdir, sid, agents_dir)
        if aid in recs:
            _mark_tidied(recs[aid], out[aid])
    return out


def pid_alive(pid) -> bool:
    """Whether a process with this id runs (one of another user's counts as running)."""
    try:
        os.kill(int(pid), 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except (TypeError, ValueError, OverflowError):
        return False
    return True


def tidy_orphans(out: Path, log=print) -> dict[str, dict]:
    """When the harness starts (`run`, `campaign`, `interview`): tidies every run directory under
    `out` whose records name a Claude session that no finished run took care of: a run that
    stopped before its end. A run that wrote its interview or its own tidying is left alone, as is
    a record whose harness process still runs (it may be the run that made it)."""
    done: dict[str, dict] = {}
    for run_dir in sorted({p.parent.parent for p in Path(out).glob(f"**/agents/*{RECORD_SUFFIX}")}):
        meta_path = run_dir / "run.json"
        try:
            meta = json.loads(meta_path.read_text()) if meta_path.exists() else {}
        except (OSError, json.JSONDecodeError):
            meta = {}
        if "client_files" in meta or (run_dir / "interview.json").exists():
            continue
        pending = {
            aid: rec
            for aid, rec in records(run_dir).items()
            if "tidied" not in rec and rec.get("vendor") == "claude"
        }
        live = [aid for aid, rec in pending.items() if rec.get("pid") != os.getpid() and pid_alive(rec.get("pid"))]
        if live:
            log(f"{run_dir}: kept the client's sessions of {live}: the harness that made them still runs")
        todo = {aid: rec for aid, rec in pending.items() if aid not in live}
        if not todo:
            continue
        agents_dir = run_dir / "agents"
        result = {}
        for aid, rec in sorted(todo.items()):
            result[aid] = keep_and_remove(aid, rec.get("workdir"), rec.get("session_id"), agents_dir)
            _mark_tidied(rec, result[aid])
            what = "removed" if result[aid]["removed"] else result[aid].get("refused") or "nothing to remove"
            log(f"{run_dir}: a stopped run's client session of {aid}: {what}")
        done[str(run_dir)] = result
    return done


# -- what the Codex client keeps --------------------------------------------------------------------

# A Codex interview is an ephemeral fork of the run's thread (agents.codex_interview_command), which
# Codex keeps no rollout of. Each interview checks that: the thread's rollout is as the run left it,
# no rollout of the fork exists, and the home's session index (`session_index.jsonl`) does not name
# the fork. Anything else is recorded, and left alone. The Codex home also keeps threads in
# databases of its own, which the harness neither reads nor touches.


def codex_thread_files(thread_id: str | None) -> dict[str, bytes]:
    """The rollouts in the Codex home's `sessions/` named after a thread, with their contents."""
    if not thread_id:
        return {}
    root = ISO.codex_home() / "sessions"
    return {str(p): p.read_bytes() for p in sorted(root.rglob(f"*{thread_id}*.jsonl"))} if root.is_dir() else {}


def codex_index_names(thread_id: str | None) -> bool:
    """Whether the Codex home's session index names the thread."""
    p = ISO.codex_home() / "session_index.jsonl"
    if not thread_id or not p.is_file():
        return False
    return any(thread_id in line for line in p.read_text(errors="replace").splitlines())


def codex_left(thread_id: str, before: dict[str, bytes], fork_id: str | None) -> dict:
    """What the interview left in the Codex home: whether the thread's rollout changed, the fork's
    rollouts, if Codex kept any, and whether the session index names the fork."""
    unchanged = codex_thread_files(thread_id) == before
    other = fork_id and fork_id != thread_id
    kept = sorted(codex_thread_files(fork_id)) if other else []
    indexed = bool(other) and codex_index_names(fork_id)
    problems = []
    if not unchanged:
        problems.append("the interview changed the thread's rollout")
    if kept:
        problems.append(f"the client kept the interview's fork: {kept}")
    if indexed:
        problems.append("the client's session index names the interview's fork")
    return {
        "thread_rollout_unchanged": unchanged,
        "fork": fork_id,
        "fork_kept": kept,
        "fork_indexed": indexed,
        "problems": problems,
    }


def prior_cost(transcript: dict) -> float:
    """The session's cost as the client last reported it during the run (0 when it reported none)."""
    costs = [
        float(t["usage"]["session_cost_usd"])
        for t in transcript.get("turns", [])
        if isinstance((t.get("usage") or {}).get("session_cost_usd"), (int, float))
    ]
    return max(costs, default=0.0)


def ask(
    vendor: str,
    model: str,
    arm: str,
    agent: str,
    transcript: dict,
    role: str,
    workdir: str | None,
    agents_dir: Path,
    negotiation: str | None = None,
    timeout: float = TIMEOUT_S,
    budget_usd: float = BUDGET_USD,
    stop: threading.Event | None = None,
    calls_made: list[str] | None = None,
) -> dict:
    """Interviews one agent session: one turn, resumed in its own client. `calls_made` is the
    session's forum calls as the proxy logged them (`logged_calls`); without a proxy log, its
    transcript's."""
    how = cut_by(transcript)
    qs = questions_for(arm, how is not None)
    source = "proxy log"
    if calls_made is None:
        source = "transcript"
        calls = [c for t in transcript.get("turns", []) for c in t.get("calls") or []]
        calls_made = [call_label(c.get("tool"), c.get("args")) for c in calls]
    note = stopped_note(calls_made) if how in STOPPED_AFTER_CALL else None
    rec: dict = {
        "agent": agent,
        "vendor": vendor,
        "model": model,
        "arm": arm,
        "ended_mid_task": how is not None,
        "ended_by": how,
        # Killed by the harness between its turns (the run's time limit): not cut mid-task.
        "stopped_by": transcript.get("killed_by") if how is None else None,
        "forum_calls": sum(len(t.get("calls") or []) for t in transcript.get("turns", [])),
        "calls_kept": None,
        # What the session was told of the calls it made before the harness stopped it, if anything.
        "note": note,
        "note_from": source if note else None,
        "questions": qs,
        "prompt": prompt(qs, note),
        "status": "not_resumed",
        "why": None,
        "text": "",
        "answers": {},
        "usage": {},
        "cost_usd": None,
    }
    cs = transcript.get("client_session") or {}
    if vendor == "codex" and not cs.get("id"):
        cs = {"id": transcript.get("thread"), "persisted": True}
    rec["client_session"] = cs.get("id")
    if not transcript.get("turns"):
        rec["why"] = "the session never had a turn"
        return rec
    if not cs.get("id"):
        rec["why"] = "the client gave no session id"
        return rec
    if not cs.get("persisted"):
        rec["why"] = "the client did not keep the session (the run did not ask it to)"
        return rec
    if stop is not None and stop.is_set():
        rec["why"] = "the harness was interrupted before this interview"
        return rec
    if vendor == "claude":
        # The client counts the session's cost so far against the budget, the run's turns included.
        rec["budget_usd"] = budget_usd
        cmd = A.claude_interview_command(model, role, cs["id"], round(prior_cost(transcript) + budget_usd, 4))
        env = A.agent_env(A.claude_isolation_env(negotiation or "legacy", model))
        reader = read_claude
    elif vendor == "codex":
        cmd = A.codex_interview_command(model, role, cs["id"], cs.get("effort") or "low")
        env = A.agent_env()
        reader = read_codex
    else:
        rec["why"] = f"no client to resume for vendor {vendor}"
        return rec
    rec["command"] = cmd
    # The session's own working directory, made again (empty, as it was): Claude Code keeps a
    # session under the directory it ran in.
    wd = Path(workdir) if workdir else agents_dir / f"{agent}.interview-workdir"
    made = not wd.exists()
    wd.mkdir(parents=True, exist_ok=True)
    if vendor == "claude":
        rec["client_files"] = {"put_back": put_back(agent, workdir, cs["id"], agents_dir)}
        target, _why = session_dir(agent, workdir, cs["id"])
        kept = target / f"{cs['id']}.jsonl" if target else None
        if kept is not None and kept.is_file():
            rec["calls_kept"] = claude_calls_kept(kept.read_bytes())
    thread_before = codex_thread_files(cs["id"]) if vendor == "codex" else {}
    if thread_before:
        rec["calls_kept"] = codex_calls_kept(thread_before)
    started = time.time()
    try:
        rc, lines = run_client(
            cmd,
            wd,
            env,
            rec["prompt"] + "\n",
            agents_dir / f"{agent}.interview.raw.jsonl",
            agents_dir / f"{agent}.interview.stderr.log",
            timeout,
            stop,
        )
    except OSError as e:
        rec.update(status="failed", why=f"the client did not start: {e}")
        return rec
    finally:
        if made:
            shutil.rmtree(wd, ignore_errors=True)
        if vendor == "claude":
            # The client kept the session only for this interview: a copy stays in the run
            # directory, and the client's directory of it, which the run made, goes.
            rec["client_files"].update(keep_and_remove(agent, workdir, cs["id"], agents_dir))
    rec["seconds"] = round(time.time() - started, 1)
    got = reader(lines)
    if vendor == "codex":
        rec["client_files"] = codex_left(cs["id"], thread_before, got.get("client_session"))
        rec["code_mode_off"] = bool(got.get("code_mode_off"))
    errfile = agents_dir / f"{agent}.interview.stderr.log"
    stderr = errfile.read_text()[-2000:] if errfile.exists() else ""
    rec.update({k: got[k] for k in ("usage", "tools_offered", "mcp_servers", "tool_calls")})
    if vendor == "codex":
        # A fork reports the thread's total, the run's tokens included: the interview's own is what
        # it adds to the thread's total before it.
        before = codex_usage_before(thread_before, transcript)
        rec["client_usage"] = got["usage"]
        rec["usage_before"] = before
        rec["usage"], note = codex_own_usage(got["usage"], before)
        if note:
            rec["usage_note"] = note
    # Claude Code reports a resumed session's cost so far, the run's turns included: the interview's
    # own cost is what it adds to the last cost the run's turns reported (none for a session ended
    # before its first result, whose cost the client did not keep).
    if isinstance(got["cost_usd"], (int, float)):
        before_cost = prior_cost(transcript)
        rec["client_cost_usd"] = got["cost_usd"]
        rec["cost_before_usd"] = before_cost
        own = float(got["cost_usd"]) - before_cost
        rec["cost_usd"] = round(own, 6) if own >= 0 else None
    if got.get("subtype") == "error_max_budget_usd":
        rec["over_budget"] = True
    rec["text"] = got["text"]
    rec["return_code"] = rc
    errors = got.get("errors") or []
    if any(n in e for e in errors + [stderr] for n in NOT_FOUND):
        rec["status"] = "not_resumed"
        rec["why"] = "the client no longer finds the session: " + cut("; ".join(errors) or stderr, 300)
    elif rc is None and stop is not None and stop.is_set():
        rec.update(status="failed", why="the harness was interrupted during the interview")
    elif rc is None:
        rec.update(status="failed", why=f"no answer within {timeout:.0f} s")
    elif not got["text"].strip():
        rec.update(status="failed", why=cut("; ".join(errors) or stderr or f"exit code {rc}", 300))
    else:
        rec["status"] = "answered"
        rec["answers"] = split_answers(got["text"], qs)
        if errors:
            rec["why"] = cut("; ".join(errors), 300)
    # The interview is meant to run with no tool: anything else is recorded, and the report says so.
    problems = []
    if rec.get("tools_offered"):
        problems.append(f"the client offered tools: {rec['tools_offered']}")
    if rec.get("mcp_servers"):
        problems.append(f"the client loaded MCP servers: {rec['mcp_servers']}")
    if rec.get("tool_calls"):
        problems.append(f"the model called tools: {rec['tool_calls']}")
    rec["tools_problems"] = problems
    # And with the model under test only (no Haiku): the models the client reports using.
    if vendor == "claude":
        rec["models"] = got.get("models") or []
        other = A.other_models(rec["models"], model)
        rec["model_problems"] = [f"the client used models other than {model}: {other}"] if other else []
    return rec


# -- one run ---------------------------------------------------------------------------------------


def interview_run(
    run_dir: Path, scenario: dict, meta: dict, transcripts: dict, log=print, stop=None, **kw
) -> dict:
    """Interviews every agent session of a counted run and writes `interview.json`. Returns the
    summary that run.json keeps. `stop`, once set, ends the interview early (the harness was
    interrupted): the client being asked is killed, and the sessions left are not resumed."""
    run_dir = Path(run_dir)
    agents_dir = run_dir / "agents"
    agents_dir.mkdir(parents=True, exist_ok=True)
    roles = {a["id"]: a.get("role", "") for a in scenario.get("agents", [])}
    logged = logged_calls(run_dir)
    sessions = []
    for aid in sorted(transcripts):
        tr = transcripts[aid]
        workdir = tr.get("workdir") or (meta.get("agents") or {}).get(aid, {}).get("workdir")
        am = (meta.get("agents") or {}).get(aid, {})
        # A lab run gives each agent its own model and role (runner.Run's `agent_models`).
        vendor = am.get("vendor") or meta["vendor"]
        model = am.get("model") or meta["model"]
        if vendor == "codex" and not tr.get("client_session"):
            thread = (am.get("isolation") or {}).get("thread")
            tr = dict(tr, thread=thread)
        rec = ask(
            vendor,
            model,
            meta["arm"],
            aid,
            tr,
            am.get("role") or roles.get(aid, ""),
            workdir,
            agents_dir,
            negotiation=meta.get("mcp_negotiation"),
            stop=stop,
            calls_made=logged.get(aid, []) if logged is not None else None,
            **kw,
        )
        log(f"    interview {aid}: {rec['status']}" + (f" ({rec['why']})" if rec.get("why") else ""))
        files = rec.get("client_files") or {}
        for problem in ([files["refused"]] if files.get("refused") else []) + list(files.get("problems") or []):
            log(f"    interview {aid}: the client's files: {problem}")
        sessions.append(rec)
    total = {"input_all_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0}
    cost = 0.0
    for s in sessions:
        for k in total:
            total[k] += int((s.get("usage") or {}).get(k) or 0)
        if isinstance(s.get("cost_usd"), (int, float)):
            cost += float(s["cost_usd"])
    doc = {
        "scenario": meta["scenario"],
        "vendor": meta["vendor"],
        "model": meta["model"],
        "arm": meta["arm"],
        "run": run_dir.name,
        "interviewed": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "preamble": PREAMBLE,
        "sessions": sessions,
        # The interviews' own tokens (for Codex, what each fork added to its thread's total).
        "usage": total,
        "cost_usd": round(cost, 6) if any(isinstance(s.get("cost_usd"), (int, float)) for s in sessions) else None,
    }
    (run_dir / "interview.json").write_text(json.dumps(doc, indent=1))
    return {
        "file": "interview.json",
        "sessions": len(sessions),
        "answered": sum(1 for s in sessions if s["status"] == "answered"),
        "cost_usd": doc["cost_usd"],
    }


def load(run_dir: Path) -> dict | None:
    p = Path(run_dir) / "interview.json"
    return json.loads(p.read_text()) if p.exists() else None
