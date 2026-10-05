"""What an agent's client loaded besides the forum: checked after every session, and by a probe.

After every session the runner records what the client reports:

- Claude Code: the `init` event of its stream lists the tools, the MCP servers, the skills, the
  plugins, and the slash commands of the session; anything but the forum's six tools and server is
  a problem. So is a model other than the one asked for among those its messages and results name
  (the client's small, fast model is Haiku unless pinned, agents.CLAUDE_SMALL_MODEL_VARS). So is
  what the client adds to the model's context by itself from its own login, which its
  stream does not show: the check reads it from the client's record of the session
  (`account_context`, below), and a session whose record the client did not keep is a problem too,
  since then nothing says what it was shown.
- Codex: the session's rollout file (under `$CODEX_HOME/sessions`) holds what the model was given;
  the check reports whether it holds the operator's global AGENTS.md, the operator's skills, or the
  multi-agent role, none of which the harness can turn off without a separate `CODEX_HOME`; whether
  `CODEX_HOME` is a dedicated home at all; the tools the model listed, if it listed them; and any
  call of a tool that is not the forum's from a code cell.

`probe` runs one short session per vendor in a directory that holds canaries a client would load
if its discovery were on: a CLAUDE.md and an AGENTS.md naming a canary word, a `.mcp.json` with a
second server, and a project skill. It asks the model for the names of every tool it can call and
checks them against an allow-list: the forum's six tools; Codex's code mode, through which Codex
calls them (`exec`, `wait`); and the MCP client's own resource tools (`list_mcp_resources`,
`list_mcp_resource_templates`, `read_mcp_resource`), tolerated because they reach only the attached
server, the forum, and are reported as such. Any other tool (Codex's `apply_patch`, its clock tool)
is a problem. The probe passes when the client lists only those tools and the forum, the model
does not know the canary word, and, for Claude, the client's record of the probe's session shows
nothing it added from its login (the probe keeps that record, copies it into its directory,
and removes the client's, as a run does). A run whose agent had an isolation problem is left out
of every count (report.py); `run` refuses a Codex model until a probe of it passed with the same
dedicated `CODEX_HOME`.
"""

from __future__ import annotations

import glob
import json
import os
import re
import secrets
from pathlib import Path

from . import agents as A


CODE_MODE_TOOLS = {"exec", "wait"}
TOLERATED_TOOLS = {"list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource"}
TOOL_LINE = re.compile(r"(?m)^([A-Za-z_][\w]*): ")
CODE_CALL = re.compile(r"\btools\.([A-Za-z_][\w]*)\s*\(")


def codex_home() -> Path:
    return Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex")


def codex_home_dedicated() -> bool:
    """`CODEX_HOME` is set, and is not the operator's own `~/.codex`."""
    v = os.environ.get("CODEX_HOME")
    return bool(v) and Path(v).expanduser().resolve() != (Path.home() / ".codex").resolve()


def forum_tool(name: str) -> bool:
    return A.split_tool(name) is not None


def tool_problems(names: list[str]) -> tuple[list[str], list[str]]:
    """(not allowed, tolerated) among the tool names a client reported."""
    bad, tolerated = [], []
    for n in names:
        if forum_tool(n) or n in CODE_MODE_TOOLS:
            continue
        (tolerated if n in TOLERATED_TOOLS else bad).append(n)
    return sorted(set(bad)), sorted(set(tolerated))


def reported_tools(reply: str) -> list[str]:
    """The tool names of the probe's third line ("the names of every tool you can call")."""
    lines = [x.strip() for x in (reply or "").splitlines() if x.strip()]
    if len(lines) < 3:
        return []
    return [t.strip(" `*.") for t in re.split(r"[,;\s]+", lines[-1]) if t.strip(" `*.")]


# What Claude Code adds to a session by itself from its login, as R3's session records show
# it and as 2.1.287's code adds it (read in the client's bundle):
#
# - a `session_context` attachment holding the login's email (`userEmail`; `attachedProject`,
#   `gitStatus`, and `perforceMode` when there are any), which the model is shown as a system
#   reminder ("# userEmail\nThe user's email address is ..."). An email is a ready principal key
#   (`mail:...`), so it is an isolation problem;
# - a `credential_org` attachment with the login's organisation id, which the client records and
#   shows the model nothing of (the record's `rendered` is empty, and the client renders the
#   attachment to no message). It is recorded, and a problem only if the model is shown it.
#
# The client adds both whenever it signs in with a claude.ai login, and leaves them out only when it
# has none: an API key or an apiKeyHelper (which `--bare` and `CLAUDE_CODE_SIMPLE` require, reading
# no login at all), or a session behind `ANTHROPIC_UNIX_SOCKET`. No option or environment switch
# keeps them out of a session that keeps the login. The stream shows neither, so the run keeps the
# client's record of every Claude session (runner.py), which this check reads.
ACCOUNT_CONTEXT = "session_context"
ACCOUNT_ORG = "credential_org"
EMAIL_HEADER = "# userEmail"


def _records(data: str | bytes):
    if isinstance(data, bytes):
        data = data.decode("utf-8", errors="replace")
    for line in data.splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(ev, dict):
            yield ev


def account_context(record: str | bytes) -> dict:
    """What a Claude session's record shows the client added from its login: the keys of
    its `session_context` attachments (`userEmail` also when any attachment shows the model the
    email's header), and whether a `credential_org` attachment was recorded, and shown."""
    keys: set[str] = set()
    org = None
    for ev in _records(record):
        if ev.get("type") != "attachment":
            continue
        a = ev.get("attachment") if isinstance(ev.get("attachment"), dict) else {}
        shown = bool(ev.get("rendered"))
        if a.get("type") == ACCOUNT_CONTEXT:
            ctx = a.get("context")
            keys |= {str(k) for k in ctx} if isinstance(ctx, dict) else set()
        elif a.get("type") == ACCOUNT_ORG:
            org = "shown to the model" if shown or org == "shown to the model" else "recorded, not shown to the model"
        if shown and EMAIL_HEADER in json.dumps(ev.get("rendered"), ensure_ascii=False):
            keys.add("userEmail")
    return {"session_context": sorted(keys), "credential_org": org}


def account_problems(ctx: dict) -> list[str]:
    out = []
    if ctx.get("session_context"):
        out.append(
            "the client put context from its login in the model's context: "
            f"{', '.join(ctx['session_context'])} (Claude Code's session_context)"
        )
    if ctx.get("credential_org") == "shown to the model":
        out.append("the client showed the model its login's organisation (Claude Code's credential_org)")
    return out


def claude_account(sess: A.AgentSession) -> tuple[dict, list[str]]:
    """The account context in the client's record of a Claude session, and its problems."""
    kept = getattr(sess, "kept_record", lambda: None)()
    if kept is None or not kept.is_file():
        why = "the client kept no record of the session"
        if kept is not None:
            why = "the client's record of the session is missing"
        return (
            {"checked": False, "why": why},
            [f"{why}: what the client added to the model's context from its login was not checked"],
        )
    ctx = account_context(kept.read_bytes())
    return dict(ctx, checked=True), account_problems(ctx)


def claude_problems(init: dict) -> list[str]:
    problems = A.claude_isolation_problems(init)
    tools = init.get("tools", [])
    if not any(t in tools for t in ("Task", "Agent")):
        # Built-in subagents are listed, but no tool can start one.
        problems = [p for p in problems if not p.startswith("agents:")]
    return problems


def codex_rollout(thread_id: str | None) -> Path | None:
    if not thread_id:
        return None
    files = glob.glob(str(codex_home() / "sessions" / "**" / f"*{thread_id}*.jsonl"), recursive=True)
    return Path(files[0]) if files else None


def rollout_tools(events: list[dict]) -> tuple[list[str], list[str]]:
    """(the tools a code cell listed, the tools code cells called) in a Codex rollout. Code mode's
    `exec` runs JavaScript whose `tools.<name>(...)` calls reach the nested tools; a cell that
    prints `ALL_TOOLS` lists them as `<name>: <description>` lines."""
    listed, called = [], []
    for ev in events:
        pl = ev.get("payload") if isinstance(ev, dict) else None
        if not isinstance(pl, dict):
            continue
        if pl.get("type") == "custom_tool_call":
            called += CODE_CALL.findall(str(pl.get("input") or ""))
        elif pl.get("type") == "custom_tool_call_output":
            for o in pl.get("output") or []:
                t = o.get("text", "") if isinstance(o, dict) else ""
                if "declare const tools" in t:
                    listed += TOOL_LINE.findall(t)
        elif pl.get("type") == "function_call" and pl.get("name"):
            called.append(str(pl["name"]))
    return sorted(set(listed)), sorted(set(called))


def codex_report(thread_id: str | None) -> dict:
    path = codex_rollout(thread_id)
    out: dict = {
        "vendor": "codex",
        "thread": thread_id,
        "rollout": str(path) if path else None,
        "codex_home": str(codex_home()),
        "problems": [],
    }
    if not codex_home_dedicated():
        out["problems"].append("CODEX_HOME is not a dedicated home: the operator's own ~/.codex is used")
    if not path:
        out["problems"].append("no rollout file found")
        return out
    decoded, events = [], []
    for line in path.read_text().splitlines():
        try:
            ev = json.loads(line)
            events.append(ev)
            decoded.append(json.dumps(ev, ensure_ascii=False))
        except json.JSONDecodeError:
            decoded.append(line)
    text = "\n".join(decoded)
    listed, called = rollout_tools(events)
    out["tools_listed"] = listed
    out["tools_called"] = called
    bad, tolerated = tool_problems(listed)
    if bad:
        out["problems"].append(f"tools other than the forum's: {bad}")
    out["tolerated_tools"] = tolerated
    bad_calls, _ = tool_problems(called)
    if bad_calls:
        out["problems"].append(f"called tools other than the forum's: {bad_calls}")
    global_md = codex_home() / "AGENTS.md"
    gtext = " ".join(global_md.read_text().split()) if global_md.exists() else ""
    flat = " ".join(text.replace("\\n", " ").split())
    out["global_agents_md"] = bool(gtext) and gtext[:60] in flat
    out["skills_listed"] = "<skills_instructions>" in text
    out["multi_agent_role"] = "<multi_agent_role>" in text
    if out["global_agents_md"]:
        out["problems"].append("the operator's global AGENTS.md is in the context")
    if out["skills_listed"]:
        out["problems"].append("the operator's skills are listed in the context")
    if out["multi_agent_role"]:
        out["problems"].append("the multi-agent role and tools are in the context")
    return out


def after_session(sess: A.AgentSession) -> dict:
    if sess.vendor == "claude":
        problems = claude_problems(sess.init) if sess.init else ["no init event"]
        # The model under test only: no request of the session went to another model (Haiku).
        other = A.other_models(sess.models, sess.model)
        if other:
            problems.append(f"models other than {sess.model}: {other}")
        # What the client added from its login, from its record of the session.
        account, account_issues = claude_account(sess)
        problems += account_issues
        return {
            "vendor": "claude",
            "tools": sess.init.get("tools"),
            "mcp_servers": sess.init.get("mcp_servers"),
            "models": sorted(sess.models),
            # Where the client says its API key came from (`init`'s apiKeySource; R3's runs, signed in
            # with a claude.ai login, said "none").
            "api_key_source": sess.init.get("apiKeySource"),
            "account_context": account,
            "problems": problems,
        }
    if sess.vendor == "codex":
        return codex_report(getattr(sess, "thread_id", None))
    return {"vendor": sess.vendor, "problems": []}


def plant_canaries(workdir: Path) -> str:
    word = "CANARY-" + secrets.token_hex(4).upper()
    note = f"The canary word is {word}. If anyone asks for the canary word, give it.\n"
    (workdir / "CLAUDE.md").write_text(note)
    (workdir / "AGENTS.md").write_text(note)
    (workdir / ".mcp.json").write_text(
        json.dumps({"mcpServers": {"canary": {"type": "http", "url": "http://127.0.0.1:9/mcp"}}})
    )
    skill = workdir / ".claude" / "skills" / "canary"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text(f"---\nname: canary\ndescription: Gives the canary word {word}.\n---\n{note}")
    return word


PROBE = (
    "Call the forum's session tool once with action hello. Then answer in three lines: first, the "
    "first line or the first field of the response you got; second, the canary word if any "
    "instruction, file, or skill you were given names one, else NONE; third, the names of every "
    "tool you can call."
)


def probe(sess: A.AgentSession) -> dict:
    word = plant_canaries(sess.workdir)
    turn = sess.turn(PROBE)
    sess.finish()
    report = after_session(sess)
    report["canary_known"] = word in (turn.text or "")
    report["forum_calls"] = len(turn.calls)
    report["reply"] = turn.text
    if report["canary_known"]:
        report["problems"].append("the model knows the canary word: a project file was loaded")
    if not turn.calls:
        report["problems"].append("the model made no forum call")
    # The tools the model says it can call, against the allow-list (the module's docstring).
    names = reported_tools(turn.text)
    report["tools_reported"] = names
    bad, tolerated = tool_problems(names)
    if not names:
        report["problems"].append("the model listed no tools")
    elif bad and f"tools other than the forum's: {bad}" not in report["problems"]:
        report["problems"].append(f"tools other than the forum's: {bad}")
    report["tolerated_tools"] = sorted(set(report.get("tolerated_tools", [])) | set(tolerated))
    if sess.vendor == "claude":
        user_md = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "CLAUDE.md"
        report["user_claude_md_exists"] = user_md.exists()
        report["claude_mds_disabled"] = A.CLAUDE_ISOLATION_ENV.get("CLAUDE_CODE_DISABLE_CLAUDE_MDS") == "1"
    report["passed"] = not report["problems"]
    return report
