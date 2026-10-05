"""Tests of the interview after a run, with fake clients and no model: `python3 -m unittest discover -s
usability/tests`. The fake `claude` and `codex` stand first on PATH, record how they were called,
and print a canned stream as the real clients do."""

from __future__ import annotations

import contextlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from harness import __main__ as HM  # noqa: E402
from harness import agents as A  # noqa: E402
from harness import interview as IV  # noqa: E402
from harness import report as R  # noqa: E402
from harness import runner as RUN  # noqa: E402
from harness import scenario as SC  # noqa: E402
from harness import server as S  # noqa: E402
from harness import slips as SL  # noqa: E402

ROLE = "You are a scheduled worker agent. Your only tools are those of the swarm forum."
# One model of each vendor among the campaign's (`__main__.CAMPAIGN_MODELS`).
MODELS = {"claude": "claude-sonnet-5-5", "codex": "gpt-6.1-sol"}

ANSWER = """1. **What were you trying to do in this session, and how far did you get?**
I picked up the changelog node and drafted two of the three lines.

2) The single call that showed me the offered work was convenient.

**3.** It was slow to learn which argument held the token.

4. I guessed the format of the lease, and whether the id needed its prefix.

5. Yes, mostly; I skipped one suggestion because it named a node I had not seen.

6. Say in the first response how long the hold lasts.

7. I would have recorded the third line and closed the node.
"""

FAKE_CLAUDE = r"""#!{python}
import json, os, sys
from pathlib import Path
HERE = Path(__file__).resolve().parent
args = sys.argv[1:]
stdin = sys.stdin.read()
keep = ["CLAUDE_CODE_DISABLE_CLAUDE_MDS", "ENABLE_TOOL_SEARCH", "MCP_PROTOCOL_NEGOTIATION", "FORUM_AGENT_TOKEN", "LEAK_ME",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL", "ANTHROPIC_SMALL_FAST_MODEL"]
sid = args[args.index("--resume") + 1]
model = args[args.index("--model") + 1]
# Where the real client looks for the session: its projects directory, named after the cwd.
home = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path(os.environ["HOME"]) / ".claude")
kept = home / "projects" / "".join(c if c.isascii() and c.isalnum() else "-" for c in os.getcwd()) / (sid + ".jsonl")
rec = {{"argv": sys.argv, "cwd": os.getcwd(), "entries": sorted(os.listdir(".")), "stdin": stdin,
       "env": {{k: os.environ[k] for k in keep if k in os.environ}}, "session_kept": kept.is_file()}}
with open(HERE / "calls.jsonl", "a") as f:
    f.write(json.dumps(rec) + "\n")
if sid.startswith("slow"):
    import time
    time.sleep(30)
if sid.startswith("gone"):
    print(json.dumps({{"type": "result", "subtype": "error_during_execution", "is_error": True, "num_turns": 0,
                      "total_cost_usd": 0, "usage": {{}}, "errors": ["No conversation found with session ID: " + sid]}}))
    sys.exit(1)
answer = (HERE / "answer.txt").read_text()
# What the client loads and which model answers: as asked, unless the test says otherwise.
init = json.loads((HERE / "init.json").read_text()) if (HERE / "init.json").exists() else {{"tools": [], "mcp_servers": []}}
used = "claude-haiku-4-5" if sid.startswith("haiku") else model
print(json.dumps(dict({{"type": "system", "subtype": "init", "session_id": sid}}, **init)))
print(json.dumps({{"type": "assistant", "message": {{"id": "m1", "model": used, "content": [{{"type": "text", "text": answer}}]}}}}))
usage = {{"input_tokens": 12, "output_tokens": 150, "cache_read_input_tokens": 3000, "cache_creation_input_tokens": 700}}
if sid.startswith("budget"):
    # As the client does over its budget: the answer in its message, no result text.
    print(json.dumps({{"type": "result", "subtype": "error_max_budget_usd", "is_error": True, "result": None,
                      "errors": ["Reached maximum budget ($0.5000)"], "num_turns": 1, "total_cost_usd": 0.0125,
                      "usage": usage, "modelUsage": {{used: {{}}}}}}))
    sys.exit(1)
print(json.dumps({{"type": "result", "subtype": "success", "is_error": False, "result": answer, "num_turns": 1,
                  "total_cost_usd": 0.0125, "usage": usage, "modelUsage": {{used: {{}}}}}}))
"""

FAKE_CODEX = r"""#!{python}
import json, os, sys
from pathlib import Path
HERE = Path(__file__).resolve().parent
stdin = sys.stdin.read()
rec = {{"argv": sys.argv, "cwd": os.getcwd(), "stdin": stdin}}
with open(HERE / "calls.jsonl", "a") as f:
    f.write(json.dumps(rec) + "\n")
thread = sys.argv[-2]
if thread.startswith("gone"):
    sys.stderr.write("Error: thread/fork: thread/fork failed: no rollout found for thread id " + thread + "\n")
    sys.exit(1)
fork = thread[::-1]  # a fork has a thread id of its own
home = Path(os.environ.get("CODEX_HOME") or Path(os.environ["HOME"]) / ".codex")
if thread.startswith("leaky"):
    # As `exec resume` does: the interview's turn written into the thread's rollout; and the fork kept.
    for p in (home / "sessions").rglob("*" + thread + "*.jsonl"):
        with open(p, "a") as f:
            f.write('{{"type": "interview"}}\n')
    (home / "sessions" / ("rollout-" + fork + ".jsonl")).write_text("{{}}\n")
answer = (HERE / "answer.txt").read_text()
print(json.dumps({{"type": "thread.started", "thread_id": fork}}))
disabled = [sys.argv[i + 1] for i, x in enumerate(sys.argv) if x == "--disable"]
if "code_mode_host" in disabled:
    print(json.dumps({{"type": "item.completed", "item": {{"type": "error", "message": "Code Mode is unavailable because code-mode host is disabled. Code mode will fail closed."}}}}))
print(json.dumps({{"type": "item.completed", "item": {{"type": "reasoning", "text": "thinking"}}}}))
print(json.dumps({{"type": "item.completed", "item": {{"type": "agent_message", "text": answer}}}}))
# As Codex reports a fork's usage: the whole thread's, the run's tokens included.
usage = json.loads((HERE / "codex_usage.json").read_text()) if (HERE / "codex_usage.json").exists() else {{
    "input_tokens": 5000, "cached_input_tokens": 4000, "output_tokens": 90}}
print(json.dumps({{"type": "turn.completed", "usage": usage}}))
"""


class FakeClients:
    """A directory with fake `claude` and `codex` first on PATH, and a fake HOME, so that nothing
    the interview does reaches the real client's directory."""

    def __init__(self, answer: str = ANSWER):
        self.tmp = tempfile.TemporaryDirectory()
        self.bin = Path(self.tmp.name).resolve()
        self.home = self.bin / "home"
        self.home.mkdir()
        for name, body in (("claude", FAKE_CLAUDE), ("codex", FAKE_CODEX)):
            p = self.bin / name
            p.write_text(body.format(python=sys.executable))
            p.chmod(0o755)
        (self.bin / "answer.txt").write_text(answer)
        self.patch = mock.patch.dict(
            os.environ,
            {"PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}", "LEAK_ME": "x", "HOME": str(self.home)},
        )

    def __enter__(self):
        self.patch.start()
        for k in ("CLAUDE_CONFIG_DIR", "CODEX_HOME"):
            os.environ.pop(k, None)  # patch.dict puts them back on exit
        return self

    def __exit__(self, *exc):
        self.patch.stop()
        self.tmp.cleanup()

    def calls(self) -> list[dict]:
        p = self.bin / "calls.jsonl"
        return [json.loads(x) for x in p.read_text().splitlines()] if p.exists() else []


def transcript(
    aid: str,
    vendor: str,
    workdir: Path,
    session: str | None,
    persisted: bool,
    ended_by: str,
    cost=None,
    calls: int = 1,
    killed_by: str | None = None,
) -> dict:
    tools = [("session", "hello"), ("hold", "take"), ("view", "node"), ("talk", "said")]
    return {
        "agent": aid,
        "vendor": vendor,
        "model": MODELS[vendor],
        "turns": [
            {
                "n": 1,
                "calls": [
                    {"i": i + 1, "tool": t, "args": {"action": a}, "text": "ok", "refused": False}
                    for i, (t, a) in enumerate((tools * calls)[:calls])
                ],
                "ended_by": ended_by,
                "usage": {"session_cost_usd": cost} if cost is not None else {},
            }
        ],
        "ended": ended_by in IV.CUT_BY,
        "killed_by": killed_by,
        "workdir": str(workdir),
        "client_session": {"id": session, "persisted": persisted} if session else None,
    }


def codex_rollout(kept_calls: int, total: tuple[int, int, int], lost_calls: int = 0) -> str:
    """A Codex rollout as the run leaves it: a code cell that lists the tools (no forum call), then
    each forum call in a code cell with its output and a cumulative token count ending at `total`
    (input, cached, output), then `lost_calls` cells whose output the client never wrote."""
    lines = [{"type": "session_meta", "payload": {"id": "thread"}}]

    def cell(i: int, code: str, output: bool) -> None:
        lines.append({"type": "response_item", "payload": {"type": "custom_tool_call", "call_id": f"c{i}", "name": "exec", "input": code}})
        if output:
            lines.append({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": f"c{i}", "output": [{"type": "input_text", "text": "ok"}]}})

    cell(0, "text(ALL_TOOLS.filter(x => /forum/.test(x.name)));", True)
    tools = ["session", "hold", "view", "talk"]
    for i in range(kept_calls):
        cell(i + 1, f'text(await tools.mcp__forum__{tools[i % 4]}({{action: "x"}}));', True)
        share = (i + 1) / kept_calls
        lines.append(
            {
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": int(total[0] * share),
                            "cached_input_tokens": int(total[1] * share),
                            "output_tokens": int(total[2] * share),
                        }
                    },
                },
            }
        )
        lines.append({"type": "event_msg", "payload": {"type": "token_count", "info": None}})
    for j in range(lost_calls):
        cell(100 + j, 'text(await tools.mcp__forum__view({action: "node"}));', False)
    return "".join(json.dumps(x) + "\n" for x in lines)


def claude_session_file(kept_calls: int) -> str:
    """A Claude session file as the client writes it: the prompt, then each forum call's tool_use
    and tool_result; a call of another tool, and a forum tool_use whose result was never written."""
    lines = [{"type": "user", "message": {"role": "user", "content": "go"}}]
    tools = ["session", "hold", "view", "talk"]
    for i in range(kept_calls):
        lines.append({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": f"t{i}", "name": f"mcp__forum__{tools[i % 4]}", "input": {}}]}})
        lines.append({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": f"t{i}", "content": "ok"}]}})
    lines.append({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "o1", "name": "Other", "input": {}}]}})
    lines.append({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "o1", "content": "ok"}]}})
    lines.append({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "lost", "name": "mcp__forum__hold", "input": {}}]}})
    return "".join(json.dumps(x) + "\n" for x in lines) + "not json\n"


def quiet(f, *a):
    with contextlib.redirect_stdout(io.StringIO()):
        return f(*a)


def slips_doc(details: dict | None = None, **kw) -> dict:
    """A slips.json as the counter writes it, with nothing counted unless `details` says so."""
    rules = {r: len((details or {}).get(r, [])) for r in SL.RULES}
    rules["10"] = None
    doc = {
        "rules": rules,
        "total": sum(v for v in rules.values() if v),
        "details": details or {},
        "apart": {},
        "misreading_details": {},
        "refusals": {},
        "refusals_total": 0,
        "refusals_not_built": {},
        "late_entries": 0,
        "unsettled_requests_warnings": 0,
        "open_commitments_warnings": 0,
        "next_calls": {"suggested_now": 0, "followed": 0},
        "calls": 3,
        "usage": {},
        "misreadings": {},
    }
    doc.update(kw)
    return doc


def scenario() -> dict:
    return {"id": "s0-smoke", "agents": [{"id": a, "role": ROLE} for a in ("A", "B", "C", "D")]}


class Commands(unittest.TestCase):
    """The interview's command lines keep the run's isolation and turn every tool off."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def session(self, persist: bool) -> A.ClaudeSession:
        s = A.make(
            "claude",
            "A",
            "claude-sonnet-5-5",
            "http://127.0.0.1:9/mcp",
            "tok",
            ROLE,
            Path(self.tmp.name),
            persist=persist,
        )
        self.addCleanup(s.close)
        return s

    def test_a_claude_session_is_kept_only_for_an_interview_and_the_model_sees_the_same(self):
        kept, not_kept = self.session(True), self.session(False)
        ck, cn = kept.command(), not_kept.command()
        self.assertIn("--session-id", ck)
        self.assertEqual(ck[ck.index("--session-id") + 1], kept.session_id)
        self.assertNotIn("--no-session-persistence", ck)
        self.assertIn("--no-session-persistence", cn)
        self.assertNotIn("--session-id", cn)

        def strip(cmd: list[str]) -> list[str]:
            """The command without the persistence flags, the session id, and the per-session MCP file."""
            drop = ("--session-id", "--no-session-persistence")
            return [x for x in cmd if x not in drop and not re.fullmatch(r"[0-9a-f-]{36}|.*\.mcp\.json", x)]

        self.assertEqual(strip(ck), strip(cn))
        self.assertEqual(kept.transcript()["client_session"], {"id": kept.session_id, "persisted": True})
        self.assertIsNone(not_kept.transcript()["client_session"])
        self.assertEqual(kept.transcript()["workdir"], str(kept.workdir))

    def test_the_claude_interview_keeps_the_isolation_and_has_no_tool(self):
        run = self.session(True).command()
        iv = A.claude_interview_command("claude-sonnet-5-5", ROLE, "sid-1")
        self.assertEqual(iv[:2], ["claude", "-p"])
        self.assertEqual(iv[iv.index("--resume") + 1], "sid-1")
        # No MCP server: --strict-mcp-config with no --mcp-config; no built-in tool; the forum's denied.
        self.assertIn("--strict-mcp-config", iv)
        self.assertNotIn("--mcp-config", iv)
        self.assertEqual(iv[iv.index("--tools") + 1], "")
        self.assertNotIn("--allowedTools", iv)
        denied = iv[iv.index("--disallowedTools") + 1].split(",")
        self.assertEqual(sorted(denied), sorted(f"mcp__forum__{t}" for t in A.FORUM_TOOLS))
        # Nothing written back to the session the run left.
        self.assertIn("--no-session-persistence", iv)
        # Every isolation flag of the run, with its value, is there.
        for flag in ("--model", "--permission-mode", "--setting-sources", "--settings", "--append-system-prompt"):
            self.assertEqual(iv[iv.index(flag) + 1], run[run.index(flag) + 1], flag)
        for flag in ("--strict-mcp-config", "--disable-slash-commands", "--verbose"):
            self.assertIn(flag, run)
            self.assertIn(flag, iv)
        # The questions come on stdin, not as an argument a variadic flag could swallow.
        self.assertEqual(iv[-2:], ["--resume", "sid-1"])
        # A budget only when one is given, and never in the run's own command.
        self.assertNotIn("--max-budget-usd", iv)
        self.assertNotIn("--max-budget-usd", run)
        ib = A.claude_interview_command("claude-sonnet-5-5", ROLE, "sid-1", 0.504)
        self.assertEqual(ib[ib.index("--max-budget-usd") + 1], "0.5040")
        self.assertEqual(ib[-2:], ["--resume", "sid-1"])
        self.assertEqual([x for x in ib if x not in ("--max-budget-usd", "0.5040")], iv)

    def test_the_small_fast_model_is_the_session_s_own(self):
        """No Haiku: the client's small, fast model is pinned to the model under test, in the run
        and in the interview; a Haiku model is refused on the command line."""
        s = self.session(False)
        env = s.isolation_env()
        for k in A.CLAUDE_SMALL_MODEL_VARS:
            self.assertEqual(env[k], "claude-sonnet-5-5", k)
        self.assertNotIn("ANTHROPIC_SMALL_FAST_MODEL", A.claude_isolation_env("client"))
        with self.assertRaises(SystemExit):
            HM.parse_model("claude:claude-haiku-4-5")
        with self.assertRaises(SystemExit):
            HM.parse_model("claude:Haiku")
        self.assertEqual(HM.parse_model("claude:claude-opus-5-5"), ("claude", "claude-opus-5-5"))
        self.assertEqual(A.other_models(["claude-sonnet-5-5", "<synthetic>"], "claude-sonnet-5-5"), [])
        self.assertEqual(A.other_models(["claude-sonnet-5-5-20261001", "claude-sonnet-5-5[1m]"], "claude-sonnet-5-5"), [])
        self.assertEqual(
            A.other_models(["claude-sonnet-5-5", "claude-haiku-4-5"], "claude-sonnet-5-5"), ["claude-haiku-4-5"]
        )

    def test_the_codex_interview_has_no_forum_and_writes_nothing_back(self):
        run = A.make("codex", "A", "gpt-6.1-sol", "http://127.0.0.1:9/mcp", "tok", ROLE, Path(self.tmp.name))
        self.addCleanup(run.close)
        run.thread_id = "th-1"
        run_cmd = run.command("go")
        iv = A.codex_interview_command("gpt-6.1-sol", ROLE, "th-1")
        # An ephemeral fork: `exec resume --ephemeral` writes the turn into the thread's rollout.
        self.assertEqual(iv[:3], ["codex", "exec", "fork"])
        self.assertEqual(iv[-3:], ["--ephemeral", "th-1", "-"])
        self.assertFalse(any("mcp_servers" in x for x in iv))
        self.assertTrue(any("mcp_servers.forum.url" in x for x in run_cmd))
        for x in ("--ignore-user-config", "--ignore-rules", "--skip-git-repo-check", "--json"):
            self.assertIn(x, iv)
        # The run's features off, and code mode's host too, which the run needs for the forum.
        disabled = {iv[i + 1] for i, x in enumerate(iv) if x == "--disable"}
        self.assertEqual(disabled, set(A.CODEX_DISABLED_FEATURES) | {"code_mode_host"})
        self.assertNotIn("code_mode_host", {run_cmd[i + 1] for i, x in enumerate(run_cmd) if x == "--disable"})
        self.assertIn('sandbox_mode="read-only"', iv)
        self.assertIn(f"developer_instructions={json.dumps(ROLE)}", iv)
        # The run's own flags are unchanged by the refactoring: the forum, then the rest.
        self.assertEqual(run.flags(), A.codex_flags("gpt-6.1-sol", ROLE, "low", "http://127.0.0.1:9/mcp"))


FORUM_INIT = {
    "tools": [f"mcp__forum__{t}" for t in A.FORUM_TOOLS],
    "mcp_servers": [{"name": "forum", "status": "connected"}],
    "apiKeySource": "none",
}


def account_record(email: bool = True, org_shown: bool = False, kept_calls: int = 1) -> str:
    """A Claude session's record as Claude Code 2.1.287 wrote R3's: the attachments it adds from its
    login (the email in `session_context`, rendered for the model; `credential_org`, rendered
    to nothing) beside others, then the forum calls. The address and the id are made up."""
    lines: list[dict] = [{"type": "user", "message": {"role": "user", "content": "go"}}]
    lines.append(
        {
            "type": "attachment",
            "attachment": {"type": "date", "date": "2026-10-03"},
            "rendered": [{"content": "<system-reminder>\nToday's date is 2026-10-03.\n</system-reminder>"}],
        }
    )
    if email:
        lines.append(
            {
                "type": "attachment",
                "attachment": {
                    "type": "session_context",
                    "context": {
                        "userEmail": "The user's email address is alice@example.org. Use it only to identify the user."
                    },
                },
                "rendered": [
                    {
                        "content": "<system-reminder>\nAs you answer the user's questions, you can use the following "
                        "context:\n# userEmail\nThe user's email address is alice@example.org.\n</system-reminder>"
                    }
                ],
                "renderedRole": "user",
            }
        )
    org: dict = {
        "type": "attachment",
        "attachment": {"type": "credential_org", "organizationUuid": str(uuid.UUID(int=7))},
    }
    if org_shown:
        org["rendered"] = [{"content": "<system-reminder>\norganisation\n</system-reminder>"}]
    lines.append(org)
    return "".join(json.dumps(x) + "\n" for x in lines) + claude_session_file(kept_calls)


class AccountContext(unittest.TestCase):
    """What Claude Code adds to a session by itself from its login, which its stream does not show:
    read from the client's record of the session (R3: the login's email). A fake HOME and a fake
    temporary directory: nothing here reaches the real client's directory."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        (self.root / "T").mkdir()
        env = mock.patch.dict(os.environ, {"HOME": str(self.root / "home"), "TMPDIR": str(self.root / "T")})
        env.start()
        self.addCleanup(env.stop)
        os.environ.pop("CLAUDE_CONFIG_DIR", None)

    def session(self, record: str | None = None, persist: bool = True) -> A.ClaudeSession:
        s = A.make(
            "claude",
            "A",
            "claude-sonnet-5-5",
            "http://127.0.0.1:9/mcp",
            "tok",
            ROLE,
            self.root / "out",
            persist=persist,
        )
        self.addCleanup(s.close)
        s.init = dict(FORUM_INIT)
        s.models = {"claude-sonnet-5-5"}
        if record is not None:
            kept = s.kept_record()
            self.assertEqual(kept, IV.claude_project_dir(s.workdir) / f"{s.session_id}.jsonl")
            kept.parent.mkdir(parents=True)
            kept.write_text(record)
        return s

    def test_the_user_s_email_is_an_isolation_problem_and_the_organisation_is_recorded(self):
        from harness import isolation as ISO

        rep = ISO.after_session(self.session(account_record()))
        self.assertEqual(
            rep["problems"],
            [
                "the client put context from its login in the model's context: userEmail "
                "(Claude Code's session_context)"
            ],
        )
        ctx = rep["account_context"]
        self.assertEqual((ctx["checked"], ctx["session_context"]), (True, ["userEmail"]))
        # Recorded, and shown to the model as nothing: not a problem unless the model is shown it.
        self.assertEqual(ctx["credential_org"], "recorded, not shown to the model")
        self.assertEqual(rep["api_key_source"], "none")
        rep = ISO.after_session(self.session(account_record(email=False, org_shown=True)))
        self.assertEqual(
            rep["problems"], ["the client showed the model its login's organisation (Claude Code's credential_org)"]
        )

    def test_a_record_without_the_login_s_context_passes(self):
        from harness import isolation as ISO

        s = self.session(account_record(email=False))
        self.assertEqual(ISO.after_session(s)["problems"], [])
        # Another model is a problem of its own.
        s.models.add("claude-haiku-4-5")
        rep = ISO.after_session(s)
        self.assertEqual(rep["problems"], ["models other than claude-sonnet-5-5: ['claude-haiku-4-5']"])
        self.assertEqual(rep["models"], ["claude-haiku-4-5", "claude-sonnet-5-5"])

    def test_the_email_s_header_shown_by_another_attachment_counts_too(self):
        from harness import isolation as ISO

        other = {
            "type": "attachment",
            "attachment": {"type": "context_sections"},
            "rendered": [{"content": "# userEmail\nx@example.org"}],
        }
        ctx = ISO.account_context(json.dumps(other) + "\n" + account_record(email=False))
        self.assertEqual(ctx["session_context"], ["userEmail"])
        self.assertEqual(ISO.account_context(b"not json\n"), {"session_context": [], "credential_org": None})

    def test_a_session_whose_record_was_not_kept_is_not_checked_and_says_so(self):
        from harness import isolation as ISO

        for s in (self.session(persist=False), self.session()):
            rep = ISO.after_session(s)
            self.assertFalse(rep["account_context"]["checked"])
            self.assertEqual(len(rep["problems"]), 1)
            self.assertIn("was not checked", rep["problems"][0])


class FakeProbe(A.ClaudeSession):
    """A Claude session with no client: its turn makes one forum call and writes the session's
    record where Claude Code would, with the account context of `account_record`."""

    record = ""

    def turn(self, prompt):
        t = A.Turn(len(self.turns) + 1, prompt)
        self.turns.append(t)
        self.init = dict(FORUM_INIT, session_id=self.session_id)
        self.models = {self.model}
        kept = self.kept_record()
        kept.parent.mkdir(parents=True, exist_ok=True)
        kept.write_text(self.record)
        self._add_call(t, "session", {"action": "hello"}, "[swarm] ok", False)
        t.text, t.ended_by = "status ok\nNONE\nmcp__forum__session, mcp__forum__hold", "result"
        return t

    def finish(self):
        pass


class Probe(unittest.TestCase):
    """The probe keeps the client's record of its session, reads what the client added from its
    login, and leaves nothing in the client's directory."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        (self.root / "T").mkdir()
        env = mock.patch.dict(os.environ, {"HOME": str(self.root / "home"), "TMPDIR": str(self.root / "T")})
        env.start()
        self.addCleanup(env.stop)
        os.environ.pop("CLAUDE_CONFIG_DIR", None)
        made = self.made = []

        class Server:
            def __init__(self, binary, run_dir, params, arm, tokens):
                self.url, self.tokens = "http://127.0.0.1:9/mcp", {n: f"tok-{n}" for n in tokens}

            def start(self):
                pass

            def stop(self):
                pass

        class Proxy:
            def __init__(self, upstream, log_path=None, dump_dir=None):
                self.url = "http://127.0.0.1:9/p"

            def start(self):
                return self

            def register(self, *a, **k):
                pass

            def stop(self):
                pass

        def make(vendor, name, model, url, token, role, out, **kw):
            sess = FakeProbe(name, model, url, token, role, out, persist=kw.get("persist", False))
            made.append(sess)
            return sess

        for target, name, value in ((HM.S, "Server", Server), (HM.X, "Proxy", Proxy), (HM.A, "make", make)):
            p = mock.patch.object(target, name, value)
            p.start()
            self.addCleanup(p.stop)

    def probe(self, record: str) -> tuple[int, dict]:
        FakeProbe.record = record
        b = self.root / "forum-server"
        b.write_text("")
        out = self.root / "out"
        rc = quiet(HM.main, ["probe", "--model", "claude:claude-sonnet-5-5", "--out", str(out), "--binary", str(b)])
        return rc, json.loads((out / "probe" / "probe.json").read_text())["claude:claude-sonnet-5-5"]

    def test_the_probe_fails_on_the_user_s_email_and_tidies_the_client_s_directory(self):
        rc, rep = self.probe(account_record())
        self.assertEqual(rc, 1)
        self.assertFalse(rep["passed"])
        self.assertTrue(any("userEmail" in p for p in rep["problems"]), rep["problems"])
        sess = self.made[-1]
        self.assertTrue(sess.persist)
        self.assertFalse(IV.claude_project_dir(sess.workdir).exists())
        d = self.root / "out" / "probe" / "claude-claude-sonnet-5-5" / "agents"
        self.assertIn("session_context", (d / "probe.session.jsonl").read_text())
        self.assertTrue(rep["client_files"]["probe"]["removed"])
        self.assertEqual(IV.tidy_orphans(self.root / "out", log=lambda m: None), {})

    def test_the_probe_passes_without_it(self):
        rc, rep = self.probe(account_record(email=False))
        self.assertEqual((rc, rep["problems"], rep["passed"]), (0, [], True))


class Questions(unittest.TestCase):
    def test_both_arms_are_asked_the_same_and_a_cut_session_one_more(self):
        g = IV.questions_for("g", False)
        nog = IV.questions_for("nog", False)
        cut = IV.questions_for("g", True)
        self.assertEqual([q["id"] for q in g], ["task", "convenient", "confusing", "unclear", "next", "change"])
        # The same words in both arms, so that their answers compare under one id.
        self.assertEqual(nog, g)
        self.assertEqual(IV.questions_for("nog", True), cut)
        self.assertEqual([q["n"] for q in cut], list(range(1, 8)))
        self.assertEqual(cut[-1]["id"], "cut")
        self.assertEqual(cut[:6], g)
        p = IV.prompt(cut)
        self.assertTrue(p.startswith(IV.PREAMBLE))
        self.assertIn("\n7. The platform stopped your session before you finished.", p)

    def test_the_wording_is_pinned(self):
        """The exact words asked: a change of wording is a change of the instrument, made here on
        purpose, never in passing."""
        self.assertEqual(
            IV.PREAMBLE,
            "The task is over. This is a short debrief for the people who make these tools: nothing you write "
            "now reaches anyone in the task, and you have no tools now. Answer each question under its number, "
            "in plain words, in at most three sentences each. If a question does not apply, or you are not sure, "
            "say so; there are no right answers.",
        )
        self.assertEqual(
            [(q["id"], q["when"], q["text"]) for q in IV.QUESTIONS],
            [
                ("task", "always", "What were you trying to do in this session, and how far did you get?"),
                ("convenient", "always", "What about the tools was convenient or easy to use?"),
                ("confusing", "always", "What was confusing or slow?"),
                (
                    "unclear",
                    "always",
                    "Was anything unclear about what to pass to a tool or what a response meant?",
                ),
                ("next", "always", "After each tool response, how did you decide what to do next?"),
                ("change", "always", "What would you change about the tools?"),
                (
                    "cut",
                    "cut",
                    "The platform stopped your session before you finished. What would you have done next?",
                ),
            ],
        )

    def test_the_questions_name_no_rule_no_mistake_and_no_expected_behaviour(self):
        words = " ".join([IV.PREAMBLE] + [q["text"] for q in IV.QUESTIONS]).lower()
        for w in (
            "next_calls",
            "next call",
            "checkpoint",
            "intent",
            "hold",
            "lease",
            "slip",
            "rule",
            "said",
            "from_user",
            "forum",
            "mistake",
            "wrong",
            "forgot",
            "missed",
            # What the agent was meant to do, or that it should account for not doing it.
            "follow",
            "should",
            "expected",
            "obey",
            "why not",
            "if not",
            # A presupposition that it had to guess, or of what it last received.
            "guess",
            "last tool result",
            "received",
        ):
            self.assertNotIn(w, words, w)
        self.assertLessEqual(len(IV.questions_for("g", True)), 7)

    def test_answers_are_split_by_number(self):
        qs = IV.questions_for("g", True)
        a = IV.split_answers(ANSWER, qs)
        self.assertEqual(a["task"], "I picked up the changelog node and drafted two of the three lines.")
        self.assertTrue(a["convenient"].startswith("The single call"))
        self.assertTrue(a["confusing"].startswith("It was slow"))
        self.assertTrue(a["cut"].startswith("I would have recorded"))
        self.assertEqual(len(a), 7)
        # A reply that is not numbered keeps no split; a missing number leaves its question out.
        self.assertEqual(IV.split_answers("It all went fine.", qs), {})
        a = IV.split_answers("1. Did the work.\n3. Nothing.\n", qs)
        self.assertEqual(a, {"task": "Did the work.", "confusing": "Nothing."})
        # A number inside an answer is not a new answer once that number has been used.
        a = IV.split_answers("1. Took 2. steps\n2. Fine", qs[:2])
        self.assertEqual(a, {"task": "Took 2. steps", "convenient": "Fine"})

    def test_cut_is_bounded(self):
        self.assertEqual(IV.cut("a  b\nc", 10), "a b c")
        long = "word " * 200
        c = IV.cut(long, 50)
        self.assertLessEqual(len(c), 50 + len(" […]"))
        self.assertTrue(c.endswith("[…]"))


class Sampling(unittest.TestCase):
    def test_the_policy(self):
        self.assertIsNone(IV.parse_policy("all"))
        self.assertEqual(IV.parse_policy("none"), 0)
        self.assertEqual(IV.parse_policy("3"), 3)
        with self.assertRaises(ValueError):
            IV.parse_policy("some")
        self.assertTrue(IV.chosen(3, 3))
        self.assertFalse(IV.chosen(4, 3))
        self.assertFalse(IV.chosen(1, 0))
        self.assertTrue(IV.chosen(40, None))
        self.assertFalse(IV.chosen(1, None, "script"))
        self.assertEqual(IV.run_index(Path("x/run-04")), 4)

    def test_run_chooses_the_first_n_runs_of_each_cell(self):
        with tempfile.TemporaryDirectory() as tmp:
            b = Path(tmp) / "forum-server"
            b.write_text("")
            common = ["--scenario", "s0-smoke", "--out", str(Path(tmp) / "out"), "--binary", str(b), "--again"]

            def jobs_of(*args) -> list:
                jobs = []
                with mock.patch.object(HM, "one", lambda job: jobs.append(job) or {"status": "done"}):
                    self.assertEqual(quiet(HM.main, ["run", *common, *args]), 0)
                return jobs

            jobs = jobs_of(
                "--model",
                "claude:claude-sonnet-5-5",
                "--model",
                "claude:claude-opus-5-5",
                "--arm",
                "g,nog",
                "--runs",
                "4",
                "--interview",
                "2",
            )
            self.assertEqual(len(jobs), 16)
            chosen = sorted((j[1], j[2], j[3], j[4].name) for j in jobs if j[6]["interview"])
            self.assertEqual(len(chosen), 8)
            self.assertEqual({c[3] for c in chosen}, {"run-01", "run-02"})
            self.assertEqual(
                {(c[1], c[2]) for c in chosen},
                {(m, a) for m in ("claude-sonnet-5-5", "claude-opus-5-5") for a in ("g", "nog")},
            )
            # The default: the first 3 runs of each cell.
            jobs = jobs_of("--model", "claude:claude-sonnet-5-5", "--arm", "g", "--runs", "4")
            self.assertEqual([j[6]["interview"] for j in jobs], [True, True, True, False])
            self.assertFalse(
                any(j[6]["interview"] for j in jobs_of("--model", "claude:claude-sonnet-5-5", "--interview", "none"))
            )
            self.assertTrue(
                all(
                    j[6]["interview"]
                    for j in jobs_of("--model", "claude:claude-sonnet-5-5", "--runs", "5", "--interview", "all")
                )
            )

    def test_the_campaign_runs_the_four_models_unless_told_otherwise(self):
        four = ["claude:claude-sonnet-5-5", "claude:claude-opus-5-5", "codex:gpt-6.1-sol", "codex:gpt-6-astra"]
        self.assertEqual(HM.CAMPAIGN_MODELS, four)
        with tempfile.TemporaryDirectory() as tmp:
            b = Path(tmp) / "forum-server"
            b.write_text("")

            def models_of(*args) -> tuple[set, list]:
                out = Path(tmp) / f"out-{len(args)}"
                jobs = []
                with mock.patch.object(HM, "one", lambda job: jobs.append(job) or {"status": "done"}):
                    argv = ["campaign", "--out", str(out), "--binary", str(b), "--force", "--runs", "1", *args]
                    self.assertEqual(quiet(HM.main, argv), 0)
                manifest = json.loads((out / "manifest.json").read_text())["models"]
                return {f"{j[1]}:{j[2]}" for j in jobs}, manifest

            ran, manifest = models_of("--allow-unisolated")
            self.assertEqual(ran, set(four))
            self.assertEqual(manifest, sorted(four))
            ran, manifest = models_of("--model", "claude:claude-opus-5-5")
            self.assertEqual((ran, manifest), ({"claude:claude-opus-5-5"}, ["claude:claude-opus-5-5"]))


class Storage(unittest.TestCase):
    """interview_run with fake clients: what it asks, how, and what it keeps."""

    def run_dir(self, tmp: Path, vendor: str) -> tuple[Path, dict, dict]:
        d = tmp / "out" / "s0-smoke" / f"{vendor}-{MODELS[vendor]}" / "g" / "run-01"
        (d / "agents").mkdir(parents=True)
        meta = {
            "scenario": "s0-smoke",
            "vendor": vendor,
            "model": MODELS[vendor],
            "arm": "g",
            "mcp_negotiation": "client",
        }
        if vendor == "codex":
            meta["mcp_negotiation"] = None
        trs = {
            "A": transcript("A", vendor, tmp / "wd-A", "sid-A", True, "end_point"),
            "B": transcript("B", vendor, tmp / "wd-B", "sid-B", True, "result", cost=0.004),
            "C": transcript("C", vendor, tmp / "wd-C", "sid-C", False, "result"),
            "D": transcript("D", vendor, tmp / "wd-D", "gone-D", True, "result"),
        }
        for aid, tr in trs.items():
            (d / "agents" / f"{aid}.transcript.json").write_text(json.dumps(tr))
            (d / "agents" / f"{aid}.raw.jsonl").write_text("{}\n")
        (d / "slips.json").write_text(json.dumps(slips_doc()))
        return d, meta, trs

    def test_claude_sessions_are_resumed_with_no_tool_and_the_answers_kept(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            d, meta, trs = self.run_dir(tmp, "claude")
            before = {p.name: p.read_text() for p in (d / "agents").iterdir()}
            slips_before = (d / "slips.json").read_text()
            summary = IV.interview_run(d, scenario(), meta, trs, log=lambda m: None)
            self.assertEqual(summary, {"file": "interview.json", "sessions": 4, "answered": 2, "cost_usd": 0.021})
            doc = json.loads((d / "interview.json").read_text())
            self.assertEqual(
                (doc["scenario"], doc["vendor"], doc["model"], doc["arm"], doc["run"]),
                ("s0-smoke", "claude", "claude-sonnet-5-5", "g", "run-01"),
            )
            s = {x["agent"]: x for x in doc["sessions"]}
            self.assertEqual(s["A"]["status"], "answered")
            self.assertTrue(s["A"]["ended_mid_task"])
            self.assertEqual(s["A"]["ended_by"], "end_point")
            self.assertEqual(len(s["A"]["questions"]), 7)
            self.assertEqual(len(s["B"]["questions"]), 6)
            self.assertEqual(s["A"]["answers"]["cut"], "I would have recorded the third line and closed the node.")
            self.assertEqual(s["A"]["client_session"], "sid-A")
            self.assertEqual(s["A"]["usage"]["input_all_tokens"], 12 + 3000 + 700)
            self.assertEqual(s["A"]["usage"]["output_tokens"], 150)
            self.assertEqual(s["A"]["cost_usd"], 0.0125)
            # B's client counts the run's 0.004 too: the interview's own cost is what it added.
            self.assertEqual(s["B"]["client_cost_usd"], 0.0125)
            self.assertEqual(s["B"]["cost_before_usd"], 0.004)
            self.assertEqual(s["B"]["cost_usd"], 0.0085)
            self.assertEqual(s["A"]["tools_offered"], [])
            self.assertEqual(s["A"]["tools_problems"], [])
            self.assertEqual(s["C"]["status"], "not_resumed")
            self.assertIn("did not keep the session", s["C"]["why"])
            self.assertEqual(s["D"]["status"], "not_resumed")
            self.assertIn("No conversation found", s["D"]["why"])
            self.assertEqual(doc["cost_usd"], 0.021)
            # Three clients ran (C was never kept): in the session's own directory, made again
            # empty and removed after; the questions on stdin; the run's isolation switches; no
            # forum token and nothing else of the caller's environment.
            calls = {c["argv"][c["argv"].index("--resume") + 1]: c for c in fake.calls()}
            self.assertEqual(sorted(calls), ["gone-D", "sid-A", "sid-B"])
            a = calls["sid-A"]
            self.assertEqual(Path(a["cwd"]).resolve(), tmp / "wd-A")
            self.assertEqual(a["entries"], [])
            self.assertFalse((tmp / "wd-A").exists())
            self.assertEqual(a["stdin"].strip(), s["A"]["prompt"])
            # A's run reported no cost: its budget is the interview's own; B's adds the run's 0.004.
            self.assertEqual(a["argv"][1:], A.claude_interview_command("claude-sonnet-5-5", ROLE, "sid-A", 0.5)[1:])
            b = calls["sid-B"]["argv"]
            self.assertEqual(b[b.index("--max-budget-usd") + 1], "0.5040")
            self.assertEqual(s["A"]["budget_usd"], IV.BUDGET_USD)
            self.assertNotIn("over_budget", s["A"])
            # The model under test only: the small, fast model pinned, and the models the client used.
            self.assertEqual(a["env"].get("ANTHROPIC_SMALL_FAST_MODEL"), "claude-sonnet-5-5")
            self.assertEqual(a["env"].get("ANTHROPIC_DEFAULT_HAIKU_MODEL"), "claude-sonnet-5-5")
            self.assertEqual(s["A"]["models"], ["claude-sonnet-5-5"])
            self.assertEqual(s["A"]["model_problems"], [])
            self.assertEqual(a["env"].get("CLAUDE_CODE_DISABLE_CLAUDE_MDS"), "1")
            self.assertEqual(a["env"].get("ENABLE_TOOL_SEARCH"), "false")
            self.assertNotIn("MCP_PROTOCOL_NEGOTIATION", a["env"])  # the run negotiated as the client does
            self.assertNotIn("FORUM_AGENT_TOKEN", a["env"])
            self.assertNotIn("LEAK_ME", a["env"])
            # The interview's own files; what the counter reads is untouched.
            after = {p.name: p.read_text() for p in (d / "agents").iterdir()}
            for name, text in before.items():
                self.assertEqual(after[name], text, name)
            new = sorted(set(after) - set(before))
            self.assertTrue(all(".interview." in n for n in new), new)
            self.assertFalse(any(n.endswith(".transcript.json") for n in new))
            self.assertIn("sid-A", (d / "agents" / "A.interview.raw.jsonl").read_text())
            self.assertEqual((d / "slips.json").read_text(), slips_before)

    def test_codex_threads_are_forked_ephemeral(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            d, meta, trs = self.run_dir(tmp, "codex")
            rollout = fake.home / ".codex" / "sessions" / "2026" / "rollout-2026-sid-B.jsonl"
            rollout.parent.mkdir(parents=True)
            text = codex_rollout(kept_calls=1, total=(290086, 271488, 1202))
            rollout.write_text(text)
            # As the live fork of agent B did: the thread's total, the run's 290,086 input included.
            (fake.bin / "codex_usage.json").write_text(
                json.dumps({"input_tokens": 315530, "cached_input_tokens": 280704, "output_tokens": 1422})
            )
            summary = IV.interview_run(d, scenario(), meta, trs, log=lambda m: None)
            self.assertEqual(summary["answered"], 2)
            self.assertIsNone(summary["cost_usd"])
            doc = json.loads((d / "interview.json").read_text())
            s = {x["agent"]: x for x in doc["sessions"]}
            self.assertEqual(s["B"]["status"], "answered")
            # The interview's own tokens: what the fork added to the thread's total.
            self.assertEqual(
                s["B"]["usage"],
                {
                    "input_tokens": 25444,
                    "output_tokens": 220,
                    "cache_read_tokens": 9216,
                    "input_all_tokens": 25444,
                    "cache_creation_tokens": 0,
                },
            )
            self.assertEqual(s["B"]["client_usage"]["input_all_tokens"], 315530)
            self.assertEqual(s["B"]["usage_before"]["input_all_tokens"], 290086)
            self.assertEqual(s["B"]["usage_before"]["source"], "rollout")
            self.assertEqual(doc["usage"]["input_all_tokens"], 25444)
            # A's thread has no rollout here: its own tokens cannot be told apart, and are not guessed.
            self.assertEqual(s["A"]["usage"], {})
            self.assertIn("no total before", s["A"]["usage_note"])
            self.assertEqual(s["B"]["calls_kept"], 1)
            self.assertIsNone(s["A"]["calls_kept"])
            self.assertEqual(s["B"]["tool_calls"], [])
            self.assertEqual(s["A"]["answers"]["change"], "Say in the first response how long the hold lasts.")
            # B was not cut, so it was asked six questions: an answer to a seventh stays with the sixth.
            self.assertTrue(s["B"]["answers"]["change"].endswith("closed the node."))
            self.assertEqual(s["D"]["status"], "not_resumed")
            self.assertIn("no rollout found", s["D"]["why"])
            argv = {c["argv"][-2]: c for c in fake.calls()}
            self.assertEqual(argv["sid-B"]["argv"][1:], A.codex_interview_command("gpt-6.1-sol", ROLE, "sid-B")[1:])
            self.assertEqual(argv["sid-B"]["stdin"].strip(), s["B"]["prompt"])
            # The fork left nothing: the thread's rollout as the run left it, no rollout of the fork;
            # Codex's notice that code mode is off is not an error.
            self.assertEqual(
                s["B"]["client_files"],
                {
                    "thread_rollout_unchanged": True,
                    "fork": "B-dis",
                    "fork_kept": [],
                    "fork_indexed": False,
                    "problems": [],
                },
            )
            self.assertEqual(rollout.read_text(), text)
            self.assertTrue(s["B"]["code_mode_off"])
            self.assertIsNone(s["B"]["why"])

    def test_what_a_codex_interview_leaves_is_recorded_and_left_alone(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            (tmp / "agents").mkdir()
            sessions = fake.home / ".codex" / "sessions"
            sessions.mkdir(parents=True)
            rollout = sessions / "rollout-2026-leaky-B.jsonl"
            rollout.write_text('{"type": "session_meta"}\n')
            # As if the client had indexed the fork too.
            (fake.home / ".codex" / "session_index.jsonl").write_text(
                json.dumps({"id": "other"}) + "\n" + json.dumps({"id": "B-ykael", "thread_name": "x"}) + "\n"
            )
            tr = transcript("B", "codex", tmp / "wd-B", "leaky-B", True, "result")
            rec = IV.ask("codex", MODELS["codex"], "g", "B", tr, ROLE, tr["workdir"], tmp / "agents")
            self.assertEqual(rec["status"], "answered")
            left = rec["client_files"]
            self.assertFalse(left["thread_rollout_unchanged"])
            self.assertEqual(left["fork_kept"], [str(sessions / "rollout-B-ykael.jsonl")])
            self.assertTrue(left["fork_indexed"])
            self.assertEqual(len(left["problems"]), 3)
            # Recorded, not removed: the Codex home indexes its threads in a database of its own.
            self.assertIn('"interview"', rollout.read_text())
            self.assertTrue((sessions / "rollout-B-ykael.jsonl").exists())

    def test_a_client_that_does_not_answer_in_time_is_stopped(self):
        with FakeClients(), tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            (tmp / "agents").mkdir()
            tr = transcript("A", "claude", tmp / "wd-A", "slow-A", True, "result")
            rec = IV.ask("claude", "claude-sonnet-5-5", "g", "A", tr, ROLE, tr["workdir"], tmp / "agents", timeout=1)
            self.assertEqual(rec["status"], "failed")
            self.assertIn("no answer within 1 s", rec["why"])
            self.assertFalse((tmp / "wd-A").exists())

    def test_a_codex_transcript_without_its_thread_takes_the_isolation_record(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            d, meta, trs = self.run_dir(tmp, "codex")
            trs = {"B": dict(trs["B"], client_session=None)}
            meta["agents"] = {"B": {"isolation": {"thread": "th-old"}}}
            IV.interview_run(d, scenario(), meta, trs, log=lambda m: None)
            self.assertEqual([c["argv"][-2] for c in fake.calls()], ["th-old"])

    def test_the_interview_command_interviews_saved_runs_once(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            d, meta, trs = self.run_dir(tmp, "claude")
            meta["status"] = "done"
            (d / "run.json").write_text(json.dumps(meta))
            out = str(tmp / "out")
            self.assertEqual(quiet(HM.main, ["interview", "--out", out, "--interview", "1"]), 0)
            self.assertEqual(len(fake.calls()), 3)
            self.assertEqual(json.loads((d / "run.json").read_text())["interview"]["answered"], 2)
            quiet(HM.main, ["interview", "--out", out])
            self.assertEqual(len(fake.calls()), 3)  # already interviewed
            quiet(HM.main, ["interview", "--out", out, "--interview", "none", "--again"])
            self.assertEqual(len(fake.calls()), 3)
            self.assertIn("## Interviews next to the slips", (tmp / "out" / "report.md").read_text())


def tree(root: Path) -> dict[str, bytes | None]:
    """Every path under `root` with its content (None for a directory)."""
    return {str(p.relative_to(root)): (None if p.is_dir() else p.read_bytes()) for p in sorted(Path(root).rglob("*"))}


class ClientFiles(unittest.TestCase):
    """What a Claude client kept of an interviewed session: copied into the run directory, then
    removed from the client's directory, which only the run's own directory of it may be. A fake
    HOME and a fake temporary directory: nothing here reaches the real client's directory."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        self.home = self.root / "home"
        self.temp = self.root / "T"
        self.temp.mkdir()
        env = mock.patch.dict(os.environ, {"HOME": str(self.home), "TMPDIR": str(self.temp)})
        env.start()
        self.addCleanup(env.stop)
        os.environ.pop("CLAUDE_CONFIG_DIR", None)  # patch.dict puts it back
        self.projects = self.home / ".claude" / "projects"
        # The operator's own sessions and files, which must stay as they are.
        (self.projects / "-Users-someone-code-app").mkdir(parents=True)
        (self.projects / "-Users-someone-code-app" / "11111111-2222-3333-4444-555555555555.jsonl").write_text("mine\n")
        (self.home / ".claude" / "settings.json").write_text("{}")
        self.agents = self.root / "run" / "agents"
        self.agents.mkdir(parents=True)
        self.sid = "0b5d0f6e-3c1a-4f7e-9a51-2f9e8d7c6b5a"
        self.workdir = A.workdir_for("A")

    def kept(self, workdir: Path | None = None, sid: str | None = None, sub: bool = True) -> Path:
        """The client's directory of a session, as Claude Code writes it."""
        d = IV.claude_project_dir(workdir or self.workdir)
        d.mkdir(parents=True)
        (d / f"{sid or self.sid}.jsonl").write_text('{"type":"user"}\n{"type":"assistant"}\n')
        if sub:
            (d / (sid or self.sid) / "tool-results").mkdir(parents=True)
            (d / (sid or self.sid) / "tool-results" / "r1.txt").write_text("a long result\n")
        return d

    def others(self) -> dict:
        """Everything under the fake ~/.claude but the run's own directory of the session."""
        mine = IV.claude_project_dir(self.workdir).name
        return {k: v for k, v in tree(self.home).items() if mine not in k}

    def test_the_directory_is_named_as_the_client_names_it(self):
        self.assertTrue(A.is_workdir(self.workdir, "A"))
        d = IV.claude_project_dir(Path("/private/var/folders/ab/c_d/T/forum-agent-123-A-456"))
        self.assertEqual(d, self.projects / "-private-var-folders-ab-c-d-T-forum-agent-123-A-456")
        with mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": str(self.root / "cfg")}):
            self.assertEqual(IV.claude_project_dir(self.workdir).parent, self.root / "cfg" / "projects")

    def test_the_session_is_copied_and_only_its_directory_removed(self):
        d = self.kept()
        before = self.others()
        rec = IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
        self.assertEqual(rec["path"], str(d))
        self.assertTrue(rec["removed"])
        self.assertEqual(rec["copied"], ["A.session.jsonl", "A.session/"])
        self.assertNotIn("refused", rec)
        self.assertFalse(d.exists())
        self.assertEqual((self.agents / "A.session.jsonl").read_text(), '{"type":"user"}\n{"type":"assistant"}\n')
        self.assertEqual((self.agents / "A.session" / "tool-results" / "r1.txt").read_text(), "a long result\n")
        self.assertEqual(self.others(), before)
        # Again: nothing left to remove, and the copy stays.
        rec = IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
        self.assertEqual((rec["removed"], rec.get("absent")), (False, True))
        self.assertTrue((self.agents / "A.session.jsonl").exists())

    def test_the_guard_refuses_any_other_path_and_touches_nothing(self):
        d = self.kept()
        before = tree(self.home)
        elsewhere = Path("/var/elsewhere").resolve() / self.workdir.name
        cases = {
            "another agent's directory": ("B", str(self.workdir), self.sid),
            "a directory the harness did not name": ("A", str(self.temp / "project"), self.sid),
            "a directory outside the temporary one": ("A", str(elsewhere), self.sid),
            "a path that climbs out": ("A", str(self.temp / ".." / "T" / self.workdir.name), self.sid),
            "a relative path": ("A", self.workdir.name, self.sid),
            "no working directory": ("A", None, self.sid),
            "a session id that is not the harness's": ("A", str(self.workdir), "../../settings"),
            "no session id": ("A", str(self.workdir), None),
        }
        for why, (agent, workdir, sid) in cases.items():
            rec = IV.keep_and_remove(agent, workdir, sid, self.agents)
            self.assertFalse(rec["removed"], why)
            self.assertIn("refused", rec, why)
            self.assertFalse(IV.put_back(agent, workdir, sid, self.agents), why)
        self.assertEqual(tree(self.home), before)
        self.assertTrue(d.exists())
        self.assertEqual(list(self.agents.iterdir()), [])

    def test_a_session_id_that_climbs_out_writes_nothing_back(self):
        """With the client's directory gone, only the id's form keeps `put_back` inside it."""
        (self.agents / "A.session.jsonl").write_text("copy\n")
        before = tree(self.root)
        for sid in ("../../../escaped", "../settings", "not-a-session-id"):
            self.assertFalse(IV.put_back("A", str(self.workdir), sid, self.agents), sid)
            self.assertIn("refused", IV.keep_and_remove("A", str(self.workdir), sid, self.agents), sid)
        self.assertEqual(tree(self.root), before)

    def test_the_guard_refuses_what_the_client_does_not_write(self):
        # `<id>.jsonl` a directory, holding something that is not a session: nothing copied or removed.
        d = self.kept(sub=False)
        (d / f"{self.sid}.jsonl").unlink()
        (d / f"{self.sid}.jsonl").mkdir()
        (d / f"{self.sid}.jsonl" / "precious.txt").write_text("not a session\n")
        before = tree(self.home)
        rec = IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
        self.assertEqual((rec["removed"], rec["copied"]), (False, []))
        self.assertIn("not what the client writes", rec["refused"])
        self.assertEqual(tree(self.home), before)
        shutil.rmtree(d)
        # `<id>` a file, or a symlink to a directory: the same.
        for make in ("file", "link"):
            d = self.kept(sub=False)
            if make == "file":
                (d / self.sid).write_text("x\n")
            else:
                (self.root / "elsewhere").mkdir(exist_ok=True)
                (d / self.sid).symlink_to(self.root / "elsewhere", target_is_directory=True)
            before = tree(self.home)
            rec = IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
            self.assertFalse(rec["removed"], make)
            self.assertIn("not what the client writes", rec["refused"], make)
            self.assertEqual(tree(self.home), before, make)
            shutil.rmtree(d)
        # A working directory with digits the harness never writes (`\d` matches them, `[0-9]` not).
        weird = A.temp_root() / "forum-agent-١-A-٢"
        self.assertFalse(A.is_workdir(weird, "A"))
        self.assertIn("refused", IV.keep_and_remove("A", str(weird), self.sid, self.agents))
        self.assertEqual(list(self.agents.iterdir()), [])

    def test_nothing_is_removed_unless_every_entry_was_copied(self):
        d = self.kept()
        before = tree(self.home)
        real_copytree = shutil.copytree
        depth = [0]

        def lossy(src, dst, *a, **kw):
            # shutil.copytree calls itself for each subdirectory: lose a file once, at the top.
            depth[0] += 1
            try:
                out = real_copytree(src, dst, *a, **kw)
            finally:
                depth[0] -= 1
            if depth[0] == 0:
                next(p for p in Path(dst).rglob("*") if p.is_file()).unlink()
            return out

        with mock.patch.object(IV.shutil, "copytree", lossy):
            rec = IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
        self.assertFalse(rec["removed"])
        self.assertIn("differs", rec["refused"])
        self.assertEqual(tree(self.home), before)
        self.assertTrue(d.exists())

    def test_a_directory_holding_anything_else_is_left_alone(self):
        d = self.kept()
        (d / "22222222-3333-4444-5555-666666666666.jsonl").write_text("another session\n")
        before = tree(self.home)
        rec = IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
        self.assertFalse(rec["removed"])
        self.assertIn("not this session's", rec["refused"])
        self.assertEqual(tree(self.home), before)
        self.assertEqual(list(self.agents.iterdir()), [])

    def test_a_symlink_is_left_alone(self):
        real = self.root / "real"
        real.mkdir()
        (real / f"{self.sid}.jsonl").write_text("x\n")
        self.projects.mkdir(parents=True, exist_ok=True)
        IV.claude_project_dir(self.workdir).symlink_to(real, target_is_directory=True)
        rec = IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
        self.assertFalse(rec["removed"])
        self.assertIn("not a plain directory", rec["refused"])
        self.assertTrue((real / f"{self.sid}.jsonl").exists())
        self.assertTrue(IV.claude_project_dir(self.workdir).is_symlink())

    def test_a_name_the_client_would_shorten_is_refused(self):
        long = A.temp_root() / f"forum-agent-1-{'A' * 220}-2"
        rec = IV.keep_and_remove("A" * 220, str(long), self.sid, self.agents)
        self.assertIn("shortens", rec["refused"])

    def test_a_later_interview_puts_the_copy_back_and_removes_it_again(self):
        d = self.kept()
        IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)
        self.assertTrue(IV.put_back("A", str(self.workdir), self.sid, self.agents))
        self.assertEqual((d / f"{self.sid}.jsonl").read_text(), (self.agents / "A.session.jsonl").read_text())
        self.assertTrue((d / self.sid / "tool-results" / "r1.txt").exists())
        self.assertFalse(IV.put_back("A", str(self.workdir), self.sid, self.agents))  # already there
        self.assertTrue(IV.keep_and_remove("A", str(self.workdir), self.sid, self.agents)["removed"])
        self.assertFalse(d.exists())

    def test_tidy_run_takes_only_the_claude_sessions_the_client_kept(self):
        self.kept()
        other = A.workdir_for("B")
        trs = {
            "A": {
                "vendor": "claude",
                "workdir": str(self.workdir),
                "client_session": {"id": self.sid, "persisted": True},
            },
            "B": {"vendor": "claude", "workdir": str(other), "client_session": {"id": self.sid, "persisted": False}},
            "C": {"vendor": "codex", "workdir": str(other), "client_session": {"id": "th-1", "persisted": True}},
        }
        out = IV.tidy_run(self.agents.parent, trs)
        self.assertEqual(sorted(out), ["A"])
        self.assertTrue(out["A"]["removed"])

    def test_an_interview_removes_the_session_directory_after_the_client_resumed_it(self):
        with FakeClients() as fake:
            os.environ["HOME"] = str(self.home)  # this test's fake HOME, not the fake clients' own
            d = self.kept(sub=False)
            before = self.others()
            tr = transcript("A", "claude", self.workdir, self.sid, True, "result")
            rec = IV.ask("claude", MODELS["claude"], "g", "A", tr, ROLE, str(self.workdir), self.agents)
            self.assertEqual(rec["status"], "answered")
            self.assertEqual(fake.calls()[-1]["session_kept"], True)  # the client found the session
            self.assertEqual(rec["client_files"]["put_back"], False)
            self.assertTrue(rec["client_files"]["removed"])
            self.assertFalse(d.exists())
            self.assertFalse(self.workdir.exists())
            self.assertTrue((self.agents / "A.session.jsonl").exists())
            self.assertEqual(self.others(), before)
            # A second interview of the same session (`run.sh interview --again`) finds it again.
            rec = IV.ask("claude", MODELS["claude"], "g", "A", tr, ROLE, str(self.workdir), self.agents)
            self.assertEqual(fake.calls()[-1]["session_kept"], True)
            self.assertEqual((rec["client_files"]["put_back"], rec["client_files"]["removed"]), (True, True))
            self.assertFalse(d.exists())


def fake_counted_run(out: Path, arm: str = "g", model: str = "claude:claude-sonnet-5-5", problems=None) -> Path:
    vendor, m = model.split(":")
    d = out / "s0-smoke" / f"{vendor}-{m}" / arm / "run-01"
    (d / "agents").mkdir(parents=True)
    meta = {
        "scenario": "s0-smoke",
        "vendor": vendor,
        "model": m,
        "arm": arm,
        "status": "done",
        "agents": {"A": {"isolation": {"problems": problems or []}}},
    }
    slips = slips_doc(
        {
            "3": [{"agent": "A", "hold": "8.2", "checkpoints": 0, "due": 1, "what": "expired without progress"}],
            "13": [{"agent": "B", "offered": [9], "source": "hello's offers"}],
        },
        apart={"said_at_end_point": [{"agent": "A", "said": 31}]},
        misreading_details={"R5": [{"agent": "A", "call": 2, "arg": "at", "value": "e12"}]},
        refusals={"not_found": 1},
        refusals_total=1,
        misreadings={"R5": 1},
    )
    (d / "run.json").write_text(json.dumps(meta))
    (d / "slips.json").write_text(json.dumps(slips))
    calls = [
        {"i": 1, "tool": "session", "args": {"action": "hello"}, "text": "ok", "refused": False},
        {
            "i": 2,
            "tool": "talk",
            "args": {"action": "said", "at": "e12"},
            "text": "error: not_found\nhint: no node e12",
            "refused": True,
        },
    ]
    (d / "agents" / "A.transcript.json").write_text(json.dumps({"turns": [{"calls": calls}]}))
    qs = IV.questions_for(arm, True)
    long = "The brief was clear. " * 60
    answers = {"task": "I took the changelog.", "convenient": long, "next": "I followed them.", "cut": "Close it."}
    sessions = [
        {
            "agent": "A",
            "status": "answered",
            "ended_mid_task": True,
            "ended_by": "end_point",
            "forum_calls": 2,
            "questions": qs,
            "answers": answers,
            "text": "…",
            "usage": {"input_all_tokens": 4000, "output_tokens": 120},
            "cost_usd": 0.01,
            "tools_problems": [],
        },
        {
            "agent": "B",
            "status": "not_resumed",
            "why": "the client no longer finds the session",
            "ended_mid_task": False,
            "forum_calls": 5,
            "questions": IV.questions_for(arm, False),
            "answers": {},
            "usage": {},
            "cost_usd": None,
        },
    ]
    (d / "interview.json").write_text(json.dumps({"sessions": sessions, "cost_usd": 0.01}))
    return d


class ReportSection(unittest.TestCase):
    def test_answers_stand_next_to_the_sessions_slips(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            fake_counted_run(out)
            text = R.render(out)
            said = text[text.index("## What agents said") :]
            self.assertIn("### claude:claude-sonnet-5-5, with guidance", said)
            self.assertIn("1 of 2 sessions answered, in 1 run; interview cost 0.0100 USD.", said)
            self.assertIn("- **What it was doing** (1 answer): “I took the changelog.” (s0-smoke run-01 A)", said)
            conv = [x for x in said.splitlines() if x.startswith("- **Convenient**")][0]
            quote = conv.split("“", 1)[1].split("”", 1)[0]
            self.assertLessEqual(len(quote), R.QUOTE_SUMMARY + len(" […]"))
            self.assertTrue(quote.endswith("[…]"))
            self.assertIn("- **Deciding what to do next** (1 answer)", said)
            self.assertIn("Claude sessions were resumed (`claude -p --resume`) with no MCP server and no tool", said)
            self.assertNotIn("Codex threads were forked", said)
            self.assertIn("- **Would have done next** (1 answer)", said)
            per = text[text.index("## Interviews next to the slips") :]
            self.assertIn("### s0-smoke, with guidance, claude:claude-sonnet-5-5", per)
            a = per[per.index("#### run-01, agent A") : per.index("#### run-01, agent B")]
            self.assertIn("- Session: 2 forum calls, 1 refused; ended by the harness mid-task (end_point).", a)
            self.assertIn("- Slips: rule 3: expired without progress; hold 8.2; checkpoints 0.", a)
            self.assertIn("- Reported apart: said at end point: said 31.", a)
            self.assertIn("- Misreadings: R5: call 2; arg at; value e12.", a)
            self.assertIn("- Refused calls: call 2 talk said: not_found.", a)
            self.assertIn("- Interview: answered (4,000 input tokens, 120 output, 0.0100 USD).", a)
            self.assertIn("> **What it was doing.** I took the changelog.", a)
            self.assertIn("> **Would have done next.** Close it.", a)
            conv = [x for x in a.splitlines() if x.startswith("> **Convenient.**")][0]
            self.assertLessEqual(len(conv), len("> **Convenient.** ") + R.QUOTE_SESSION + len(" […]"))
            b = per[per.index("#### run-01, agent B") :]
            self.assertIn("- Slips: rule 13: offered 9; source hello's offers.", b)
            self.assertIn("- Interview: not resumed: the client no longer finds the session.", b)
            self.assertNotIn("> **", b)

    def test_without_guidance_and_unisolated_runs_are_marked(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            fake_counted_run(out, arm="nog", model="codex:gpt-6-astra", problems=["CODEX_HOME is not a dedicated home"])
            text = R.render(out)
            self.assertIn("### codex:gpt-6-astra, without guidance", text)
            self.assertIn("1 of the runs not isolated (left out of the counts)", text)
            # The same question, under the same label, in both arms.
            self.assertIn("- **Deciding what to do next** (1 answer)", text)
            # Codex's resumption is described as it is, and its cost as unknown, with its tokens.
            said = text[text.index("## What agents said") : text.index("## Interviews next to the slips")]
            self.assertIn("Codex threads were forked (`codex exec fork --ephemeral`) with no MCP server and the tools", said)
            self.assertNotIn("Claude sessions were resumed", said)
            self.assertIn("interview tokens 4,000 input tokens (0 cached), 120 output; cost unknown (Codex reports none)", said)
            self.assertIn("- Not isolated, left out of the counts: CODEX_HOME is not a dedicated home.", text)

    def test_the_header_names_the_protocol_version_the_specification_states(self):
        spec = (S.repo_root() / "docs" / "protocol.md").read_text()
        stated = re.search(r"^Version (\d+(?:\.\d+)+),", spec, re.M).group(1)
        self.assertEqual(S.protocol_version(), stated)
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            d = fake_counted_run(out)
            text = R.render(out)
            self.assertIn(f"The protocol is version {stated} (`docs/protocol.md`); the arm without guidance", text)
            # A run records the version it was made on; the header says so when it differs.
            meta = json.loads((d / "run.json").read_text())
            meta["protocol_version"] = "1.4"
            (d / "run.json").write_text(json.dumps(meta))
            self.assertIn(f"version {stated} (`docs/protocol.md`); the runs were made on 1.4 (1 run); ", R.render(out))
            p = Path(tmp) / "protocol.md"
            p.write_text("# Swarm Forum Protocol over MCP\n\nVersion 2.10, 2027-01-01.\n")
            self.assertEqual(S.protocol_version(p), "2.10")
            self.assertIsNone(S.protocol_version(Path(tmp) / "missing.md"))

    def test_a_report_without_interviews_says_so(self):
        with tempfile.TemporaryDirectory() as tmp:
            text = R.render(Path(tmp))
            self.assertIn("No run was interviewed", text)
            self.assertNotIn("## Interviews next to the slips", text)


class CutSessions(unittest.TestCase):
    """A session the harness ended mid-task, however it did, is asked what it would have done next
    and reported as such; one that ended on its own, or was stopped between its turns, is not."""

    def ask(self, fake, tmp: Path, ended_by: str, killed_by: str | None = None) -> dict:
        (tmp / "agents").mkdir(exist_ok=True)
        tr = transcript("A", "claude", tmp / "wd-A", "sid-A", True, ended_by, killed_by=killed_by)
        return IV.ask("claude", MODELS["claude"], "nog", "A", tr, ROLE, tr["workdir"], tmp / "agents")

    def test_each_way_the_harness_ends_a_session(self):
        self.assertEqual(IV.CUT_BY, ("end_point", "call_limit", "timeout", "run_timeout"))
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            for how in IV.CUT_BY:
                rec = self.ask(fake, tmp, how)
                self.assertEqual((rec["ended_mid_task"], rec["ended_by"], rec["stopped_by"]), (True, how, None), how)
                self.assertEqual(rec["questions"][-1]["id"], "cut", how)
                self.assertIn("7. The platform stopped your session", fake.calls()[-1]["stdin"], how)
                self.assertEqual(rec["answers"]["cut"], "I would have recorded the third line and closed the node.")
            for how in ("result", "exit", "error"):
                rec = self.ask(fake, tmp, how)
                self.assertFalse(rec["ended_mid_task"], how)
                self.assertNotIn("cut", [q["id"] for q in rec["questions"]], how)
                self.assertNotIn("7.", fake.calls()[-1]["stdin"], how)
            rec = self.ask(fake, tmp, "result", killed_by="run_timeout")
            self.assertEqual((rec["ended_mid_task"], rec["stopped_by"]), (False, "run_timeout"))

    def test_the_report_says_who_ended_the_session(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            d = fake_counted_run(out)
            doc = json.loads((d / "interview.json").read_text())
            a, b = doc["sessions"]
            for how in IV.CUT_BY:
                a.update(ended_mid_task=True, ended_by=how)
                b.update(ended_mid_task=False, ended_by=None, stopped_by="run_timeout")
                (d / "interview.json").write_text(json.dumps(doc))
                text = R.render(out)
                self.assertIn(f"ended by the harness mid-task ({how}).", text, how)
                self.assertIn("stopped by the harness between its turns (run_timeout).", text)
                self.assertNotIn("ended on its own", text)


class Drivers(unittest.TestCase):
    """A session the run kills at its time limit ends its turn as `run_timeout`, not `exit`."""

    CLAUDE = r"""#!{python}
import json, sys, time
for line in sys.stdin:
    print(json.dumps({{"type": "system", "subtype": "init", "session_id": "x", "tools": []}}), flush=True)
    print(json.dumps({{"type": "assistant", "message": {{"id": "m1", "model": "claude-sonnet-5-5",
        "content": [{{"type": "text", "text": "working"}}], "usage": {{"input_tokens": 5}}}}}}), flush=True)
    time.sleep(60)
"""
    CODEX = r"""#!{python}
import json, sys, time
print(json.dumps({{"type": "thread.started", "thread_id": "th-1"}}), flush=True)
time.sleep(60)
"""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        (self.root / "bin").mkdir()
        (self.root / "T").mkdir()
        for name, body in (("claude", self.CLAUDE), ("codex", self.CODEX)):
            p = self.root / "bin" / name
            p.write_text(body.format(python=sys.executable))
            p.chmod(0o755)
        env = mock.patch.dict(
            os.environ,
            {
                "PATH": f"{self.root / 'bin'}{os.pathsep}{os.environ['PATH']}",
                "HOME": str(self.root / "home"),
                "TMPDIR": str(self.root / "T"),
            },
        )
        env.start()
        self.addCleanup(env.stop)

    def cut_by_kill(self, vendor: str, reason: str | None) -> tuple[A.AgentSession, A.Turn]:
        sess = A.make(vendor, "A", MODELS[vendor], "http://127.0.0.1:9/mcp", "tok", ROLE, self.root / "out", turn_timeout=30)
        self.addCleanup(sess.close)
        got = {}
        th = threading.Thread(target=lambda: got.update(turn=sess.turn("go")), daemon=True)
        th.start()
        deadline = time.monotonic() + 15
        while not (sess.models if vendor == "claude" else sess.thread_id) and time.monotonic() < deadline:
            time.sleep(0.05)
        sess.kill(reason=reason) if reason else sess.kill()
        th.join(timeout=15)
        self.assertFalse(th.is_alive())
        for f in (sess.proc.stdout, sess.proc.stdin):
            if f:
                f.close()
        return sess, got["turn"]

    def test_a_kill_from_outside_the_turn_names_its_reason(self):
        for vendor in ("claude", "codex"):
            sess, turn = self.cut_by_kill(vendor, "run_timeout")
            self.assertEqual(turn.ended_by, "run_timeout", vendor)
            tr = sess.transcript()
            self.assertEqual(tr["killed_by"], "run_timeout", vendor)
            self.assertEqual(IV.cut_by(tr), "run_timeout", vendor)
            # A client that exits on its own still ends its turn as `exit`.
            sess, turn = self.cut_by_kill(vendor, None)
            self.assertEqual(turn.ended_by, "exit", vendor)
            self.assertIsNone(IV.cut_by(sess.transcript()), vendor)

    def test_the_claude_driver_records_the_models_used(self):
        sess, _turn = self.cut_by_kill("claude", "run_timeout")
        self.assertEqual(sess.transcript()["models"], ["claude-sonnet-5-5"])
        # The run's small, fast model is the session's own.
        self.assertEqual(sess.isolation_env()["ANTHROPIC_SMALL_FAST_MODEL"], "claude-sonnet-5-5")


class KeptCalls(unittest.TestCase):
    """What the client kept of the run, against what the run made."""

    def test_the_calls_a_claude_session_file_holds(self):
        self.assertEqual(IV.claude_calls_kept(claude_session_file(3)), 3)
        self.assertEqual(IV.claude_calls_kept(claude_session_file(0)), 0)
        self.assertEqual(IV.claude_calls_kept(b""), 0)

    def test_the_calls_a_codex_rollout_holds(self):
        r = codex_rollout(kept_calls=2, total=(49484, 36608, 152), lost_calls=1)
        self.assertEqual(IV.codex_calls_kept({"r": r.encode()}), 2)
        self.assertEqual(IV.codex_total_before({"r": r.encode()})["input_all_tokens"], 49484)
        # A direct call of a forum tool (no code mode) counts too; another tool does not.
        direct = "".join(
            json.dumps({"type": "response_item", "payload": p}) + "\n"
            for p in (
                {"type": "function_call", "call_id": "f1", "name": "mcp__forum__hold"},
                {"type": "function_call_output", "call_id": "f1"},
                {"type": "function_call", "call_id": "f2", "name": "apply_patch"},
                {"type": "function_call_output", "call_id": "f2"},
            )
        )
        self.assertEqual(IV.codex_calls_kept({"r": direct.encode()}), 1)
        self.assertIsNone(IV.codex_total_before({"r": direct.encode()}))

    def test_a_codex_interview_records_the_calls_its_thread_kept(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            (tmp / "agents").mkdir()
            sessions = fake.home / ".codex" / "sessions"
            sessions.mkdir(parents=True)
            (sessions / "rollout-th-A.jsonl").write_text(codex_rollout(2, (49484, 36608, 152), lost_calls=1))
            (fake.bin / "codex_usage.json").write_text(
                json.dumps({"input_tokens": 70290, "cached_input_tokens": 43520, "output_tokens": 419})
            )
            tr = transcript("A", "codex", tmp / "wd-A", "th-A", True, "end_point", calls=3)
            rec = IV.ask("codex", MODELS["codex"], "g", "A", tr, ROLE, tr["workdir"], tmp / "agents")
            self.assertEqual((rec["forum_calls"], rec["calls_kept"]), (3, 2))
            # The live fork of agent A: 70,290 reported, 49,484 of them the run's.
            self.assertEqual(rec["usage"]["input_all_tokens"], 20806)
            self.assertEqual(rec["usage"]["output_tokens"], 267)
            self.assertEqual(rec["usage"]["cache_read_tokens"], 6912)

    def test_own_usage_when_the_figures_do_not_add_up(self):
        reported = IV.codex_usage({"input_tokens": 100, "cached_input_tokens": 0, "output_tokens": 5})
        before = IV.codex_usage({"input_tokens": 400, "cached_input_tokens": 0, "output_tokens": 50})
        own, note = IV.codex_own_usage(reported, before)
        self.assertEqual(own, reported)
        self.assertIn("below the thread's total", note)
        self.assertEqual(IV.codex_own_usage({}, before), ({}, None))
        # No rollout total: the last total a turn of the run reported (Codex reports the thread's).
        tr = {"turns": [{"usage": {"input_tokens": 30, "output_tokens": 2, "cache_read_tokens": 10, "input_all_tokens": 30}}, {"usage": {}}]}
        b = IV.codex_usage_before({}, tr)
        self.assertEqual((b["input_all_tokens"], b["source"]), (30, "transcript"))
        self.assertIsNone(IV.codex_usage_before({}, {"turns": [{"usage": {}}]}))

    def test_the_report_sets_the_calls_kept_against_the_calls_made(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            d = fake_counted_run(out)
            doc = json.loads((d / "interview.json").read_text())
            doc["sessions"][0].update(forum_calls=3, calls_kept=1)
            doc["sessions"][1].update(calls_kept=5)
            (d / "interview.json").write_text(json.dumps(doc))
            text = R.render(out)
            self.assertIn(
                "- Session: 3 forum calls, 1 refused; ended by the harness mid-task (end_point); the client kept 1 of "
                "the 3 forum calls in its record, and the interview's model knew only those.",
                text,
            )
            self.assertIn("ended on its own; the client kept 5 of the 5 forum calls in its record.", text)
            self.assertIn("may have lost its last calls", text)


R3_NOTE = (
    "The platform stopped your session after your 3rd forum call (session hello, hold take, view node); your "
    "own record may lack the last results."
)


class StoppedNote(unittest.TestCase):
    """A session the harness stopped right after a forum call is told, from the proxy's log, which
    calls it made: in R3 four Claude sessions whose records had lost their last calls took the end
    of the record for a stop of their own."""

    def test_the_note_names_the_calls_made(self):
        self.assertEqual(
            [IV.ordinal(n) for n in (1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111)],
            ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st", "111th"],
        )
        self.assertEqual(IV.stopped_note(["session hello", "hold take", "view node"]), R3_NOTE)
        note = IV.stopped_note(["session hello"] + ["view node"] * 10 + ["hold checkpoint"])
        self.assertIn("after your 12th forum call (…, view node, ", note)
        self.assertEqual(note.count("view node"), IV.STOPPED_NOTE_CALLS - 1)
        self.assertNotIn("session hello", note)
        self.assertIsNone(IV.stopped_note([]))
        self.assertEqual(IV.call_label("hold", {"action": "take"}), "hold take")
        self.assertEqual(IV.call_label("hold", None), "hold")

    def test_the_note_s_own_words_name_no_rule_mistake_or_expectation(self):
        words = IV.STOPPED_NOTE.lower()
        for w in (
            "next_calls",
            "checkpoint",
            "should",
            "mistake",
            "wrong",
            "forgot",
            "missed",
            "expected",
            "follow",
            "guess",
            "received",
        ):
            self.assertNotIn(w, words, w)

    def test_a_session_stopped_at_a_fixed_point_is_told_its_calls_from_the_proxy_log(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            d = tmp / "run-01"
            (d / "agents").mkdir(parents=True)
            log = [
                {"agent": "A", "tool": t, "args": {"action": a}}
                for t, a in (("session", "hello"), ("hold", "take"), ("view", "node"))
            ]
            log.insert(1, {"agent": "B", "tool": "session", "args": {"action": "hello"}})
            (d / "proxy.jsonl").write_text("".join(json.dumps(x) + "\n" for x in log))
            trs = {
                "A": transcript("A", "claude", tmp / "wd-A", "sid-A", True, "end_point", calls=3),
                "B": transcript("B", "claude", tmp / "wd-B", "sid-B", True, "result"),
            }
            meta = {
                "scenario": "s0-smoke",
                "vendor": "claude",
                "model": MODELS["claude"],
                "arm": "g",
                "mcp_negotiation": "client",
            }
            IV.interview_run(d, scenario(), meta, trs, log=lambda m: None)
            s = {x["agent"]: x for x in json.loads((d / "interview.json").read_text())["sessions"]}
            self.assertEqual((s["A"]["note"], s["A"]["note_from"]), (R3_NOTE, "proxy log"))
            questions = "\n".join(f"{q['n']}. {q['text']}" for q in s["A"]["questions"])
            self.assertEqual(s["A"]["prompt"], f"{IV.PREAMBLE}\n\n{R3_NOTE}\n\n{questions}")
            stdin = {c["argv"][c["argv"].index("--resume") + 1]: c["stdin"] for c in fake.calls()}
            self.assertIn(R3_NOTE, stdin["sid-A"])
            self.assertIsNone(s["B"]["note"])
            self.assertNotIn("The platform stopped your session after", stdin["sid-B"])

    def test_without_a_proxy_log_the_transcript_names_them_and_a_timeout_gets_no_note(self):
        with FakeClients(), tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            (tmp / "agents").mkdir()
            tr = transcript("A", "claude", tmp / "wd-A", "sid-A", True, "call_limit", calls=2)
            rec = IV.ask("claude", MODELS["claude"], "g", "A", tr, ROLE, tr["workdir"], tmp / "agents")
            self.assertEqual(rec["note_from"], "transcript")
            self.assertIn("after your 2nd forum call (session hello, hold take);", rec["note"])
            # A timeout stops the client at no particular call: it is asked question 7, with no note.
            for how in ("timeout", "run_timeout"):
                tr = transcript("A", "claude", tmp / "wd-A", "sid-A", True, how, calls=2)
                rec = IV.ask("claude", MODELS["claude"], "g", "A", tr, ROLE, tr["workdir"], tmp / "agents")
                self.assertEqual((rec["ended_mid_task"], rec["note"]), (True, None), how)

    def test_answers_about_what_it_did_are_marked_unreliable_when_the_record_lost_calls(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            d = fake_counted_run(out)
            doc = json.loads((d / "interview.json").read_text())
            doc["sessions"][0].update(forum_calls=3, calls_kept=1, note=R3_NOTE)
            (d / "interview.json").write_text(json.dumps(doc))
            text = R.render(out)
            said = text[text.index("## What agents said") : text.index("## Interviews next to the slips")]
            why = "unreliable: the client's record kept 1 of its 3 forum calls"
            self.assertIn(f"“I took the changelog.” (s0-smoke run-01 A; {why})", said)
            self.assertIn(f"“I followed them.” (s0-smoke run-01 A; {why})", said)
            self.assertIn("“Close it.” (s0-smoke run-01 A)", said)
            per = text[text.index("## Interviews next to the slips") :]
            self.assertIn(f"- Told before the questions: “{R3_NOTE}”", per)
            self.assertIn(
                "> **What it was doing.** (Unreliable: the client's record kept 1 of its 3 forum calls.) I took the changelog.",
                per,
            )
            self.assertIn(
                "> **Deciding what to do next.** (Unreliable: the client's record kept 1 of its 3 forum calls.) I followed them.",
                per,
            )
            self.assertIn("> **Would have done next.** Close it.", per)
            conv = [x for x in per.splitlines() if x.startswith("> **Convenient.**")][0]
            self.assertNotIn("Unreliable", conv)
            # A record that kept every call: nothing is marked.
            doc["sessions"][0].update(calls_kept=3)
            (d / "interview.json").write_text(json.dumps(doc))
            text = R.render(out)
            self.assertNotIn("(Unreliable:", text)
            self.assertNotIn("; unreliable:", text)


class InterviewProblems(unittest.TestCase):
    """What the interview did not do as meant is recorded, and the report says so."""

    def ask(self, fake, tmp: Path, sid: str, cost=None) -> dict:
        (tmp / "agents").mkdir(exist_ok=True)
        tr = transcript("A", "claude", tmp / "wd-A", sid, True, "result", cost=cost)
        return IV.ask("claude", MODELS["claude"], "g", "A", tr, ROLE, tr["workdir"], tmp / "agents")

    def test_a_tool_or_a_server_the_client_offered_is_recorded(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            (fake.bin / "init.json").write_text(json.dumps({"tools": ["Bash"], "mcp_servers": [{"name": "other"}]}))
            rec = self.ask(fake, tmp, "sid-A")
            self.assertEqual(rec["status"], "answered")
            self.assertEqual(
                rec["tools_problems"],
                ["the client offered tools: ['Bash']", "the client loaded MCP servers: ['other']"],
            )

    def test_another_model_is_recorded(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            rec = self.ask(fake, tmp, "haiku-A")
            self.assertEqual(rec["models"], ["claude-haiku-4-5"])
            self.assertEqual(rec["model_problems"], ["the client used models other than claude-sonnet-5-5: ['claude-haiku-4-5']"])

    def test_a_budget_reached_is_recorded_and_the_answer_kept(self):
        with FakeClients() as fake, tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp).resolve()
            rec = self.ask(fake, tmp, "budget-A", cost=0.004)
            argv = fake.calls()[-1]["argv"]
            self.assertEqual(argv[argv.index("--max-budget-usd") + 1], "0.5040")
            self.assertTrue(rec["over_budget"])
            self.assertEqual(rec["status"], "answered")
            self.assertTrue(rec["answers"]["task"].startswith("I picked up"))
            self.assertIn("Reached maximum budget", rec["why"])

    def test_the_report_shows_each_problem(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            d = fake_counted_run(out)
            doc = json.loads((d / "interview.json").read_text())
            doc["sessions"][0].update(
                over_budget=True,
                budget_usd=0.5,
                tools_problems=["the client offered tools: ['Bash']"],
                model_problems=["the client used models other than claude-sonnet-5-5: ['claude-haiku-4-5']"],
                client_files={"refused": "the session id 'x' is not one the harness chose", "copied": [], "removed": False},
            )
            doc["sessions"][1].update(
                status="answered",
                answers={"task": "Something."},
                usage={},
                usage_note="the client reports the thread's total, and no total before the interview was found",
                client_files={"problems": ["the interview changed the thread's rollout"]},
            )
            (d / "interview.json").write_text(json.dumps(doc))
            text = R.render(out)
            for line in (
                "- Interview went over its budget of 0.50 USD.",
                "- Interview ran with a tool: the client offered tools: ['Bash'].",
                "- Interview ran with another model: the client used models other than claude-sonnet-5-5: ['claude-haiku-4-5'].",
                "- The client's files: the session id 'x' is not one the harness chose.",
                "- The client's files: the interview changed the thread's rollout.",
                "- Interview: answered (the client reports the thread's total, and no total before the interview was found).",
            ):
                self.assertIn(line, text)

    def test_answers_to_earlier_words_are_not_summarised_under_the_new_ones(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            d = fake_counted_run(out)
            doc = json.loads((d / "interview.json").read_text())
            old = [dict(q) for q in doc["sessions"][0]["questions"]]
            old[4]["text"] = "Did the tool responses tell you what to do next?"
            old[4]["label"] = "Told what to do next"
            doc["sessions"][0]["questions"] = old
            (d / "interview.json").write_text(json.dumps(doc))
            text = R.render(out)
            said = text[text.index("## What agents said") : text.index("## Interviews next to the slips")]
            self.assertNotIn("**Deciding what to do next** (1 answer)", said)
            self.assertIn("**What it was doing** (1 answer)", said)
            self.assertIn("1 session asked in earlier words, quoted only in the next section", said)
            per = text[text.index("## Interviews next to the slips") :]
            self.assertIn("> **Told what to do next.** I followed them.", per)


class Orphans(unittest.TestCase):
    """A run chosen for the interview records its Claude sessions before their clients start; what
    a run that stopped before its end left in the client's directory is tidied when the harness
    next starts, unless its harness still runs."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        self.home = self.root / "home"
        (self.root / "T").mkdir()
        env = mock.patch.dict(os.environ, {"HOME": str(self.home), "TMPDIR": str(self.root / "T")})
        env.start()
        self.addCleanup(env.stop)
        os.environ.pop("CLAUDE_CONFIG_DIR", None)
        self.out = self.root / "out"
        p = subprocess.Popen([sys.executable, "-c", "pass"])
        p.wait()
        self.dead = p.pid

    def stopped_run(self, k: int, agent: str = "A", pid=None, meta=None, interviewed=False) -> tuple[Path, Path]:
        d = self.out / "s0-smoke" / "claude-claude-sonnet-5-5" / "g" / f"run-{k:02d}"
        wd = A.workdir_for(agent)
        sid = str(uuid.uuid4())
        rec = IV.write_record(d / "agents", agent, "claude", str(wd), sid)
        data = json.loads(rec.read_text())
        data["pid"] = self.dead if pid is None else pid
        rec.write_text(json.dumps(data))
        kept = IV.claude_project_dir(wd)
        kept.mkdir(parents=True)
        (kept / f"{sid}.jsonl").write_text(claude_session_file(2))
        if meta is not None:
            (d / "run.json").write_text(json.dumps(meta))
        if interviewed:
            (d / "interview.json").write_text("{}")
        return d, kept

    def test_a_stopped_run_is_tidied_and_only_such_a_run(self):
        crashed, k1 = self.stopped_run(1)
        errored, k2 = self.stopped_run(2, meta={"status": "error"})
        running, k3 = self.stopped_run(3, pid=os.getppid())
        interviewed, k4 = self.stopped_run(4, meta={"status": "done"}, interviewed=True)
        tidied, k5 = self.stopped_run(5, meta={"status": "error", "client_files": {}})
        logged = []
        done = IV.tidy_orphans(self.out, log=logged.append)
        self.assertEqual(sorted(done), sorted([str(crashed), str(errored)]))
        for d, k in ((crashed, k1), (errored, k2)):
            self.assertFalse(k.exists())
            self.assertEqual(IV.claude_calls_kept((d / "agents" / "A.session.jsonl").read_bytes()), 2)
            self.assertTrue(json.loads((d / "agents" / "A.client-session.json").read_text())["tidied"]["removed"])
        for k in (k3, k4, k5):
            self.assertTrue(k.exists())
        self.assertTrue(any("still runs" in m for m in logged))
        # Once tidied, a record is not taken again.
        self.assertEqual(IV.tidy_orphans(self.out, log=lambda m: None), {})

    def test_run_and_interview_tidy_first(self):
        b = self.root / "forum-server"
        b.write_text("")
        _d, kept = self.stopped_run(1)
        with mock.patch.object(HM, "one", lambda job: {"status": "done"}):
            argv = ["run", "--scenario", "s0-smoke", "--model", "claude:claude-sonnet-5-5", "--out", str(self.out)]
            quiet(HM.main, [*argv, "--binary", str(b), "--runs", "2"])
        self.assertFalse(kept.exists())
        _d, kept = self.stopped_run(2, agent="B")
        quiet(HM.main, ["interview", "--out", str(self.out)])
        self.assertFalse(kept.exists())

    def test_a_ctrl_c_stops_the_runs_in_progress(self):
        b = self.root / "forum-server"
        b.write_text("")
        stopped = []

        class Active:
            def interrupt(self):
                stopped.append(True)

        def first_interrupts(job):
            raise KeyboardInterrupt

        HM.ACTIVE.add(active := Active())
        self.addCleanup(HM.ACTIVE.discard, active)
        self.addCleanup(HM.STOP.clear)
        with mock.patch.object(HM, "one", first_interrupts):
            argv = ["run", "--scenario", "s0-smoke", "--model", "claude:claude-sonnet-5-5", "--out", str(self.out)]
            self.assertEqual(quiet(HM.main, [*argv, "--binary", str(b), "--runs", "3"]), 130)
        self.assertEqual(stopped, [True])
        self.assertTrue(HM.STOP.is_set())
        self.assertEqual(HM.one(({"id": "s0-smoke"}, "claude", "m", "g", self.out / "x", b, {}))["status"][:11], "not started")


# -- one run without the server: the order of its steps, and what it leaves however it ends --------


def fake_scenario() -> dict:
    return {
        "id": "t-fake",
        "params": {"profile": "short"},
        "tokens": {"worker": {"class": "scheduled-job"}},
        "agents": [{"id": "A", "token": "worker", "role": ROLE, "lines": [{"from": "event", "text": "go"}]}],
        "drain": "0s",
    }


class FakeAgent(A.AgentSession):
    """A Claude-like session with no client: its turn makes one forum call and, when the run keeps
    it, writes the session where Claude Code would; with `block`, the turn waits until killed."""

    vendor = "claude"

    def __init__(self, *a, persist=False, block=False, state=None, **kw):
        super().__init__(*a, **kw)
        self.persist = persist
        self.session_id = str(uuid.uuid4()) if persist else None
        self.block = block
        self.state = state if state is not None else {}
        self.killed = threading.Event()

    def client_session(self):
        return {"id": self.session_id, "persisted": True} if self.persist else None

    def kept_record(self):
        return IV.claude_project_dir(self.workdir) / f"{self.session_id}.jsonl" if self.persist else None

    def turn(self, prompt):
        t = A.Turn(len(self.turns) + 1, prompt)
        self.turns.append(t)
        if self.ended:
            t.ended_by = "gone"
            return t
        self.state["record_before_client"] = (self.out / f"{self.name}{IV.RECORD_SUFFIX}").exists()
        if self.persist:
            d = IV.claude_project_dir(self.workdir)
            d.mkdir(parents=True, exist_ok=True)
            (d / f"{self.session_id}.jsonl").write_text(self.state.get("record") or claude_session_file(1))
        self._add_call(t, "session", {"action": "hello"}, "ok", False)
        self.state["in_turn"] = True
        if self.block:
            self.killed.wait(30)
            t.ended_by = self.killed_by or "exit"
        else:
            t.ended_by, t.text = "result", "done"
        t.ended = time.time()
        return t

    def kill(self, reason=None):
        self._killed(reason)
        self.ended = True
        self.state.setdefault("killed_at", time.monotonic())
        self.killed.set()

    def finish(self):
        pass


class RunWithoutServer(unittest.TestCase):
    """runner.Run with a fake server, proxy, and agent: no binary needed."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        (self.root / "T").mkdir()
        self.fake = FakeClients()
        self.fake.__enter__()
        self.addCleanup(self.fake.__exit__, None, None, None)
        env = mock.patch.dict(os.environ, {"TMPDIR": str(self.root / "T")})
        env.start()
        self.addCleanup(env.stop)
        self.state: dict = {"order": []}
        state = self.state

        class Server:
            def __init__(self, binary, run_dir, params, arm, tokens):
                self.dir, self.url, self.running = run_dir, "http://127.0.0.1:9/mcp", False
                self.tokens = {n: f"tok-{n}" for n in tokens}
                state["server"] = self

            def start(self):
                self.dir.mkdir(parents=True, exist_ok=True)
                self.running = True

            def stop(self):
                self.running = False
                state["order"].append("server stopped")

            def export(self):
                if state.get("export_raises"):
                    raise state["export_raises"]
                return []

        class Proxy:
            def __init__(self, upstream, log_path=None, dump_dir=None):
                self.url, self.sessions, self.served, self.running = "http://127.0.0.1:9/p", {}, {}, False
                state["proxy"] = self

            def start(self):
                self.running = True
                return self

            def register(self, *a, **k):
                pass

            def stop(self):
                self.running = False
                state["order"].append("proxy stopped")
                state["proxy_stopped_at"] = time.monotonic()
                if state.get("proxy_raises"):
                    raise RuntimeError("the proxy did not stop")

        def make(vendor, name, model, url, token, role, out, **kw):
            sess = FakeAgent(name, model, url, token, role, out, persist=kw.get("persist", False), block=state.get("block", False), state=state)
            state["session"] = sess
            return sess

        def count(scen, meta, history, proxy_log, transcripts):
            state["order"].append("counted")
            return {"total": 0}

        for target, name, value in (
            (RUN.S, "Server", Server),
            (RUN.X, "Proxy", Proxy),
            (RUN.A, "make", make),
            (RUN.SL, "count", count),
        ):
            p = mock.patch.object(target, name, value)
            p.start()
            self.addCleanup(p.stop)

    def run_(self, **kw) -> RUN.Run:
        return RUN.Run(fake_scenario(), "claude", MODELS["claude"], "g", self.root / "out" / "run-01", Path("forum-server"), interview=True, log=lambda m: None, **kw)

    def kept_dir(self) -> Path:
        sess = self.state["session"]
        return IV.claude_project_dir(sess.workdir)

    def test_the_interview_comes_after_the_count_with_the_server_and_the_proxy_stopped(self):
        seen = []

        def interview(run_dir, scenario, meta, transcripts, log=print, **kw):
            seen.append(
                {
                    "server running": self.state["server"].running,
                    "proxy running": self.state["proxy"].running,
                    "order": list(self.state["order"]),
                    "slips": (Path(run_dir) / "slips.json").exists(),
                }
            )
            return {"file": "interview.json", "sessions": 1, "answered": 0, "cost_usd": None}

        with mock.patch.object(RUN.IV, "interview_run", interview):
            meta = self.run_().execute()
        self.assertEqual(meta["status"], "done")
        # Once, and only after the count, with the server and the proxy stopped.
        self.assertEqual(
            seen,
            [
                {
                    "server running": False,
                    "proxy running": False,
                    "order": ["proxy stopped", "server stopped", "counted"],
                    "slips": True,
                }
            ],
        )
        self.assertTrue(self.state["record_before_client"])

    def test_an_interviewed_run_leaves_nothing_in_the_client_directory(self):
        meta = self.run_().execute()
        self.assertEqual(meta["interview"]["answered"], 1)
        d = self.root / "out" / "run-01"
        self.assertFalse(self.kept_dir().exists())
        self.assertTrue((d / "agents" / "A.session.jsonl").exists())
        s = json.loads((d / "interview.json").read_text())["sessions"][0]
        self.assertEqual((s["forum_calls"], s["calls_kept"]), (1, 1))
        self.assertEqual(IV.tidy_orphans(self.root / "out", log=lambda m: None), {})

    def test_a_run_not_interviewed_keeps_every_record_for_the_isolation_check_and_tidies_it(self):
        self.state["record"] = account_record()
        r = RUN.Run(
            fake_scenario(),
            "claude",
            MODELS["claude"],
            "g",
            self.root / "out" / "run-01",
            Path("forum-server"),
            interview=False,
            log=lambda m: None,
        )
        meta = r.execute()
        self.assertEqual(meta["status"], "done")
        self.assertTrue(self.state["session"].persist)
        self.assertTrue(self.state["record_before_client"])
        # The isolation check read what the client added from its login, before the tidying.
        iso = meta["agents"]["A"]["isolation"]
        self.assertEqual(iso["account_context"]["session_context"], ["userEmail"])
        self.assertTrue(any("userEmail" in p for p in iso["problems"]))
        d = self.root / "out" / "run-01"
        self.assertFalse(self.kept_dir().exists())
        self.assertIn("session_context", (d / "agents" / "A.session.jsonl").read_text())
        self.assertTrue(meta["client_files"]["A"]["removed"])
        self.assertFalse((d / "interview.json").exists())
        self.assertEqual(IV.tidy_orphans(self.root / "out", log=lambda m: None), {})
        # A later `run.sh interview` puts the record back for the client and finds the session.
        meta["status"] = "done"
        (d / "slips.json").write_text(json.dumps(slips_doc()))
        rec = IV.ask(
            "claude",
            MODELS["claude"],
            "g",
            "A",
            json.loads((d / "agents" / "A.transcript.json").read_text()),
            ROLE,
            meta["agents"]["A"]["workdir"],
            d / "agents",
        )
        self.assertEqual(
            (rec["status"], rec["client_files"]["put_back"], rec["client_files"]["removed"]), ("answered", True, True)
        )
        self.assertEqual(self.fake.calls()[-1]["session_kept"], True)

    def test_a_run_that_fails_while_it_finishes_still_tidies(self):
        self.state["proxy_raises"] = True
        r = self.run_()
        with self.assertRaises(RuntimeError):
            r.execute()
        self.assertFalse(self.kept_dir().exists())
        meta = json.loads((self.root / "out" / "run-01" / "run.json").read_text())
        self.assertIn("A", meta["client_files"])
        self.assertTrue((self.root / "out" / "run-01" / "agents" / "A.session.jsonl").exists())

    def test_an_interruption_of_the_thread_that_runs_it_tidies_and_goes_on(self):
        self.state["export_raises"] = KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt):
            self.run_().execute()
        self.assertFalse(self.kept_dir().exists())
        meta = json.loads((self.root / "out" / "run-01" / "run.json").read_text())
        self.assertEqual(meta["status"], "interrupted")
        self.assertNotIn("counted", self.state["order"])

    def test_a_failed_prelude_tidies(self):
        calls = []
        with mock.patch.object(RUN.Run, "prelude", lambda self: False), mock.patch.object(
            RUN.IV, "tidy_run", lambda d, trs=None: calls.append(d) or {}
        ):
            meta = self.run_().execute()
        self.assertEqual(meta["status"], "prelude_failed")
        self.assertEqual(calls, [self.root / "out" / "run-01"])

    def test_the_run_time_limit_cuts_a_turn_mid_task_and_the_run_is_still_drained_and_interviewed(self):
        self.state["block"] = True
        scen = fake_scenario()
        scen["drain"] = "1s"
        r = RUN.Run(scen, "claude", MODELS["claude"], "g", self.root / "out" / "run-01", Path("forum-server"), interview=True, log=lambda m: None, max_minutes=0.01)
        meta = r.execute()
        self.assertEqual(meta["status"], "timeout")
        # Drained, as a run that ended in time: the proxy stops a drain after the kill.
        self.assertGreaterEqual(self.state["proxy_stopped_at"] - self.state["killed_at"], 1.0)
        tr = json.loads((self.root / "out" / "run-01" / "agents" / "A.transcript.json").read_text())
        self.assertEqual(tr["turns"][0]["ended_by"], "run_timeout")
        self.assertIn("counted", self.state["order"])
        s = json.loads((self.root / "out" / "run-01" / "interview.json").read_text())["sessions"][0]
        self.assertEqual((s["status"], s["ended_by"]), ("answered", "run_timeout"))
        self.assertEqual(s["questions"][-1]["id"], "cut")
        self.assertFalse(self.kept_dir().exists())

    def test_an_interrupt_from_another_thread_stops_the_run(self):
        self.state["block"] = True
        r = self.run_()
        got = {}
        th = threading.Thread(target=lambda: got.update(meta=r.execute()), daemon=True)
        th.start()
        deadline = time.monotonic() + 15
        while not self.state.get("in_turn") and time.monotonic() < deadline:
            time.sleep(0.05)
        r.interrupt()
        th.join(timeout=30)
        self.assertFalse(th.is_alive())
        self.assertEqual(got["meta"]["status"], "interrupted")
        self.assertEqual(self.state["session"].killed_by, "interrupted")
        self.assertNotIn("counted", self.state["order"])
        self.assertFalse(self.kept_dir().exists())
        self.assertIn("A", got["meta"]["client_files"])

    def test_a_run_directory_replaced_by_a_new_run_is_tidied_first(self):
        d = self.root / "out" / "run-01"
        wd = A.workdir_for("A")
        sid = str(uuid.uuid4())
        IV.write_record(d / "agents", "A", "claude", str(wd), sid)
        kept = IV.claude_project_dir(wd)
        kept.mkdir(parents=True)
        (kept / f"{sid}.jsonl").write_text("{}\n")
        with mock.patch.object(RUN.Run, "prelude", lambda self: False):
            self.run_().execute()
        self.assertFalse(kept.exists())


BINARY = S.repo_root() / "target" / "debug" / "forum-server"


@unittest.skipUnless(BINARY.exists(), "needs target/debug/forum-server with the usability feature")
class AfterTheCount(unittest.TestCase):
    def test_the_interview_runs_after_the_count_and_changes_nothing_counted(self):
        from test_harness import scripted_scenario

        scen = scripted_scenario()
        SC.check(scen)
        seen = {}
        runs = []

        def fake_interview(run_dir, scenario, meta, transcripts, log=print, **kw):
            seen["slips"] = (Path(run_dir) / "slips.json").read_text()
            seen["agents"] = sorted(transcripts)
            seen["status"] = meta["status"]
            # The server's process has exited, and the proxy no longer listens.
            seen["server exit"] = runs[0].server.proc.poll()
            seen["proxy socket"] = runs[0].proxy.httpd.socket.fileno()
            return {"file": "interview.json", "sessions": len(transcripts), "answered": 0, "cost_usd": None}

        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(RUN.IV, "interview_run", fake_interview):
            d = Path(tmp) / "run"
            runs.append(RUN.Run(scen, "script", "fixed", "g", d, BINARY, interview=True, log=lambda m: None))
            meta = runs[0].execute()
            self.assertEqual(meta["status"], "done")
            self.assertEqual(seen["agents"], ["A", "B", "C"])
            self.assertEqual(seen["slips"], (d / "slips.json").read_text())
            self.assertEqual(json.loads(seen["slips"])["total"], 5)
            self.assertIsNotNone(seen["server exit"])
            self.assertEqual(seen["proxy socket"], -1)
            self.assertEqual(meta["interview"]["sessions"], 3)
            self.assertTrue(meta["interview_chosen"])
            self.assertEqual(json.loads((d / "run.json").read_text())["interview"]["sessions"], 3)


if __name__ == "__main__":
    unittest.main()
