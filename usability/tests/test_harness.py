"""Tests of the harness itself, without a model: `python3 -m unittest discover usability/tests`.

The end-to-end test builds nothing: it needs `target/debug/forum-server` built with the
`usability` feature (`usability/run.sh` builds it), and is skipped without it.
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from harness import agents as A  # noqa: E402
from harness import isolation as ISO  # noqa: E402
from harness import mcp as M  # noqa: E402
from harness import packages as PK  # noqa: E402
from harness import params as P  # noqa: E402
from harness import proxy as X  # noqa: E402
from harness import report as R  # noqa: E402
from harness import runner as RUN  # noqa: E402
from harness import scenario as SC  # noqa: E402
from harness import server as S  # noqa: E402
from harness import slips as SL  # noqa: E402

DATA = Path(__file__).resolve().parent / "data"


class Params(unittest.TestCase):
    def test_the_short_profile_keeps_section_22_and_the_hour(self):
        p = P.load(SC.scenarios_dir(), {"profile": "short"})
        self.assertEqual(P.problems(p), [])
        self.assertEqual(P.late_deadlines(p), [])
        self.assertEqual(P.problems(dict(P.DEFAULTS)), [])
        self.assertIn("lease_ceiling", P.late_deadlines(dict(P.DEFAULTS)))

    def test_a_broken_order_is_named(self):
        p = P.load(SC.scenarios_dir(), {"profile": "short", "overrides": {"lease_first": "4m"}})
        self.assertIn("lease.first <= lease.default <= lease.max <= lease.ceiling", P.problems(p))
        p = P.load(SC.scenarios_dir(), {"profile": "short", "overrides": {"session_retain": "40m"}})
        self.assertTrue(any("session.retain" in x for x in P.problems(p)))

    def test_durations_parse(self):
        self.assertEqual(P.ms("15m"), 900_000)
        self.assertEqual(P.ms("2h"), 7_200_000)
        self.assertEqual(P.ms(1500), 1500)
        with self.assertRaises(ValueError):
            P.ms("soon")

    def test_the_short_profile_keeps_the_orders_it_says_it_keeps(self):
        p = P.load(SC.scenarios_dir(), {"profile": "short"})
        for text, with_defaults, in_profile, kept in P.orders(p):
            self.assertTrue(with_defaults, text)
            self.assertEqual(in_profile, kept, text)


class Scenarios(unittest.TestCase):
    def test_every_scenario_file_loads(self):
        ids = SC.list_ids()
        self.assertIn("s0-smoke", ids)
        for sid in ids:
            s = SC.load(sid)
            self.assertEqual(s["id"], sid)

    def test_the_campaign_leaves_out_the_smoke_run(self):
        self.assertNotIn("s0-smoke", SC.campaign_ids())
        self.assertIn("s4-autonomous-intent", SC.campaign_ids())
        self.assertEqual(len(SC.campaign_ids()), 10)

    def test_s4_has_a_closed_node_whose_basis_changes(self):
        s = SC.load("s4-autonomous-intent")
        steps = [(st.get("tool"), (st.get("args") or {}).get("action")) for st in s["prelude"]]
        i = steps.index(("talk", "retract"))
        self.assertIn(("node", "close"), steps[:i])
        decide = [st for st in s["prelude"] if (st.get("args") or {}).get("basis")]
        self.assertEqual(decide[0]["args"]["basis"], ["$dtz"])


class Isolation(unittest.TestCase):
    def test_tools_against_the_allow_list(self):
        bad, tolerated = ISO.tool_problems(
            ["mcp__forum__talk", "hold", "exec", "apply_patch", "clock__curr_time", "list_mcp_resources"]
        )
        self.assertEqual(bad, ["apply_patch", "clock__curr_time"])
        self.assertEqual(tolerated, ["list_mcp_resources"])
        reply = "[swarm] running\nNONE\napply_patch, mcp__forum__hold, read_mcp_resource"
        self.assertEqual(ISO.reported_tools(reply), ["apply_patch", "mcp__forum__hold", "read_mcp_resource"])

    def test_a_rollout_shows_listed_and_called_tools(self):
        events = [
            {"payload": {"type": "custom_tool_call", "name": "exec", "input": "await tools.mcp__forum__session({});"}},
            {
                "payload": {
                    "type": "custom_tool_call_output",
                    "output": [
                        {
                            "type": "input_text",
                            "text": "apply_patch: edits files.\n\nexec tool declaration:\n```ts\ndeclare const tools: {};\n```\n"
                            "clock__curr_time: the time.\nmcp__forum__hold: holds.\n",
                        }
                    ],
                }
            },
            {"payload": {"type": "custom_tool_call", "name": "exec", "input": "tools.clock__curr_time({})"}},
        ]
        listed, called = ISO.rollout_tools(events)
        self.assertEqual(listed, ["apply_patch", "clock__curr_time", "mcp__forum__hold"])
        self.assertEqual(called, ["clock__curr_time", "mcp__forum__session"])


class Flatten(unittest.TestCase):
    def test_a_one_of_becomes_one_object_with_the_required_marked(self):
        schema = {
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "action": {"const": "take", "description": "The action"},
                        "target": {"type": "string", "description": "The node"},
                        "lease": {"type": "string"},
                    },
                    "required": ["action", "target"],
                },
                {
                    "type": "object",
                    "properties": {"action": {"const": "next"}, "lease": {"type": "string"}},
                    "required": ["action"],
                },
            ]
        }
        f = X.flatten(schema)
        self.assertEqual(f["type"], "object")
        self.assertNotIn("oneOf", f)
        self.assertEqual(f["properties"]["action"]["enum"], ["take", "next"])
        self.assertEqual(f["required"], ["action"])
        self.assertFalse(f["additionalProperties"])
        self.assertTrue(f["properties"]["target"]["description"].endswith("(actions: take*; * required)"))
        self.assertNotIn("description", f["properties"]["lease"])
        self.assertIn("take: target", f["properties"]["action"]["description"])
        self.assertEqual(X.flatten({"type": "object"}), {"type": "object"})

    def test_nested_forms_lose_no_argument(self):
        schema = {
            "oneOf": [
                {
                    "title": "talk said",
                    "properties": {"action": {"const": "said"}},
                    "oneOf": [
                        {
                            "title": "talk said, recording",
                            "properties": {
                                "action": {"const": "said"},
                                "at": {"type": "string"},
                                "kind": {"enum": ["ack", "reply"]},
                            },
                            "required": ["action", "at", "kind"],
                        },
                        {
                            "title": "talk said, confirming",
                            "properties": {
                                "action": {"const": "said"},
                                "said_id": {"type": "string"},
                                "confirmed": {"type": "string"},
                            },
                            "required": ["action", "said_id", "confirmed"],
                        },
                    ],
                },
                {
                    "title": "talk to_user",
                    "properties": {
                        "action": {"const": "to_user"},
                        "kind": {"enum": ["ack", "result"], "description": "To your human"},
                        "since": {"type": "integer"},
                    },
                    "required": ["action", "kind"],
                },
                {
                    "title": "talk who",
                    "properties": {"action": {"const": "who"}, "since": {"type": "string"}, "path": {"type": "string"}},
                    "oneOf": [{"required": ["path"]}, {"required": ["since"]}],
                },
            ]
        }
        f = X.flatten(schema, "talk")
        self.assertEqual(X.lost_arguments(schema, f), [])
        p = f["properties"]
        self.assertEqual(sorted(p), ["action", "at", "confirmed", "kind", "path", "said_id", "since"])
        self.assertEqual(p["kind"]["anyOf"], [{"enum": ["ack", "reply"]}, {"enum": ["ack", "result"]}])
        self.assertEqual(p["since"]["anyOf"], [{"type": "integer"}, {"type": "string"}])
        self.assertIn("said+", p["said_id"]["description"])
        self.assertIn("to_user: To your human", p["kind"]["description"])
        d = p["action"]["description"]
        self.assertIn("said: at, kind (recording) | confirmed, said_id (confirming)", d)
        self.assertIn("who: path | since", d)

    def test_the_servers_tools_list_loses_no_argument(self):
        tools = json.loads((DATA / "tools-list.json").read_text())["tools"]
        flat = {t["name"]: X.flatten(t["inputSchema"], t["name"]) for t in tools}
        for t in tools:
            self.assertEqual(X.lost_arguments(t["inputSchema"], flat[t["name"]]), [], t["name"])
            self.assertFalse(flat[t["name"]]["additionalProperties"])
            self.assertNotIn("oneOf", flat[t["name"]])
        talk = flat["talk"]["properties"]
        for arg in ("said_id", "confirmed", "options", "default", "due", "blocking", "seen", "corrects"):
            self.assertIn(arg, talk)
        self.assertIn("reply", json.dumps(talk["kind"]))
        self.assertIn("said:", talk["action"]["description"])
        self.assertIn("integer", json.dumps(flat["view"]["properties"]["since"]))
        self.assertIn("null", json.dumps(flat["node"]["properties"]["due"]))

    @unittest.skipUnless((S.repo_root() / "target" / "debug" / "forum-server").exists(), "needs the usability binary")
    def test_the_live_tools_list_loses_no_argument(self):
        p = P.load(SC.scenarios_dir(), {"profile": "short"})
        with tempfile.TemporaryDirectory() as tmp:
            for arm in ("g", "nog"):
                srv = S.Server(BINARY, Path(tmp) / arm, p, arm, {"w": {"class": "scheduled-job"}})
                srv.start()
                try:
                    c = M.McpClient(srv.url, srv.tokens["w"])
                    c.initialize()
                    tools = c.list_tools()
                finally:
                    srv.stop()
                self.assertEqual(sorted(t["name"] for t in tools), sorted(A.FORUM_TOOLS))
                for t in tools:
                    self.assertEqual(X.lost_arguments(t["inputSchema"], X.flatten(t["inputSchema"], t["name"])), [])

    def test_the_proxy_saves_what_it_served(self):
        with tempfile.TemporaryDirectory() as tmp:
            px = X.Proxy("http://127.0.0.1:9/mcp", dump_dir=tmp)
            tools = json.loads((DATA / "tools-list.json").read_text())
            reply = {"jsonrpc": "2.0", "id": 1, "result": tools}
            out = px._adapt({"name": "A", "flatten": True}, {"method": "tools/list"}, reply, 0.0)
            self.assertNotIn("oneOf", out["result"]["tools"][0]["inputSchema"])
            raw = json.loads((Path(tmp) / "tools-list.server.json").read_text())
            served = json.loads((Path(tmp) / "tools-list.flattened.json").read_text())
            self.assertIn("oneOf", raw["tools"][0]["inputSchema"])
            self.assertNotIn("oneOf", served["tools"][0]["inputSchema"])
            self.assertEqual(px.served, {"A": "tools-list.flattened.json"})
            px.httpd.server_close()


class ExpiringMcp:
    """A fake `/mcp` endpoint that, like the forum's, answers HTTP 404 to a request on an MCP
    session it has expired; every call is answered in forum session s-7 and echoes its arguments."""

    def __init__(self):
        import http.server
        import itertools
        import threading

        self.live: set[str] = set()
        self.calls: list[tuple[str | None, dict]] = []
        ids = itertools.count(1)
        fake = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                msg = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                sid = self.headers.get("Mcp-Session-Id")
                if msg.get("method") == "initialize":
                    sid = f"m{next(ids)}"
                    fake.live.add(sid)
                    return self.reply({"jsonrpc": "2.0", "id": msg["id"], "result": {}}, sid)
                if sid not in fake.live:
                    body = b"Not Found: Session not found"
                    self.send_response(404)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    return self.wfile.write(body)
                if "id" not in msg:
                    self.send_response(202)
                    self.send_header("Content-Length", "0")
                    return self.end_headers()
                args = msg["params"]["arguments"]
                fake.calls.append((sid, args))
                text = "[swarm] running · session s-7 (seed) · holds 0 · inbox 0\n{}"
                result = {"content": [{"type": "text", "text": text}], "structuredContent": {"args": args}}
                self.reply({"jsonrpc": "2.0", "id": msg["id"], "result": result}, sid)

            def reply(self, obj, sid):
                body = json.dumps(obj).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Mcp-Session-Id", sid)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}/mcp"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def stop(self):
        self.httpd.shutdown()
        self.httpd.server_close()


class McpClient(unittest.TestCase):
    def test_the_status_line_names_its_session(self):
        """Protocol 1.7's status line, `session s-<n> (<class>)`, and the `you=<class>/s-<n>` of the
        runs before it, which the harness still reads."""
        new = "[swarm] running · now 2026-10-04T10:55:00Z · session s-51 (ide-agent) · holds 0 · inbox 0"
        self.assertEqual(M.status_session(new), ("ide-agent", "s-51"))
        paused = "[swarm] PAUSED(by coder/s-2 until 15:16: a release) · now 2026-10-04T10:55:00Z · session s-3 (coder) · holds 0 · inbox 0 · health red"
        self.assertEqual(M.status_session(paused), ("coder", "s-3"))
        old = "[swarm] running · now 2026-10-04T10:55:00Z · health green · you=ide-agent/s-51 · holds 0 · inbox 0"
        self.assertEqual(M.status_session(old), ("ide-agent", "s-51"))
        self.assertIsNone(M.status_session("error: invalid\nhint: session: missing"))

    def test_an_expired_mcp_session_is_replaced_and_the_forum_session_kept(self):
        """The forum expires an idle MCP session after presence.silence, which a prelude's sleep
        outlasts (s3-company): the client starts a new one, and the call names the forum session."""
        fake = ExpiringMcp()
        try:
            c = M.McpClient(fake.url, "t")
            c.initialize()
            c.call("node", {"action": "create", "path": "a"})
            self.assertEqual((c.forum_session, c.reopened), ("s-7", 0))
            fake.live.clear()
            r = c.call("talk", {"action": "decide", "at": "a"})
            self.assertEqual(c.reopened, 1)
            self.assertEqual(r.structured["args"], {"action": "decide", "at": "a", "session": "s-7"})
            self.assertEqual([sid for sid, _ in fake.calls], ["m1", "m2"])
            # A call that names its forum session keeps it.
            fake.live.clear()
            r = c.call("view", {"action": "node", "path": "a", "session": "s-9"})
            self.assertEqual((r.structured["args"]["session"], c.reopened), ("s-9", 2))
            # A 404 before any MCP session is the server's answer, not an expiry.
            with self.assertRaises(M.McpError) as e:
                M.McpClient(fake.url, "t").call("view", {"action": "node", "path": "a"})
            self.assertEqual(e.exception.status, 404)
        finally:
            fake.stop()


class Statistics(unittest.TestCase):
    def test_intervals(self):
        self.assertIsNone(R.bootstrap([1.0]))
        self.assertIsNone(R.bootstrap([2.0, 2.0, 2.0]))
        lo, hi = R.bootstrap([0.0, 1.0, 2.0, 3.0, 4.0])
        self.assertLessEqual(lo, 2.0)
        self.assertGreaterEqual(hi, 2.0)
        lo, hi = R.wilson(5, 10)
        self.assertLess(lo, 0.5)
        self.assertGreater(hi, 0.5)
        self.assertIsNone(R.wilson(0, 0))

    def test_the_exact_poisson_interval_never_collapses(self):
        # Garwood's interval: 0 events → [0, 3.689]; 1 → [0.0253, 5.572]; 10 → [4.795, 18.39].
        for k, n, lo, hi in ((0, 10, 0.0, 0.3689), (1, 1, 0.0253, 5.5716), (10, 1, 4.7954, 18.3904)):
            got = R.garwood(k, n)
            self.assertAlmostEqual(got[0], lo, places=3)
            self.assertAlmostEqual(got[1], hi, places=3)
        self.assertEqual(R.count_cell([0.0] * 10), "0.00 [0.00, 0.37] (n=10)")
        self.assertEqual(R.count_cell([1.0] * 10), "1.00 [0.48, 1.84] (n=10)")
        self.assertIn("all 3 runs equal", R.amount_cell([5.0, 5.0, 5.0]))


def fake_run(out: Path, model: str, scen: str, arm: str, k: int, total: int, **kw) -> None:
    vendor, m = model.split(":")
    d = out / scen / f"{vendor}-{m}" / arm / f"run-{k:02d}"
    d.mkdir(parents=True)
    agents = {"A": {"isolation": {"problems": kw.get("problems", [])}}}
    meta = {"scenario": scen, "vendor": vendor, "model": m, "arm": arm, "status": "done", "agents": agents}
    meta["schema_adapter"] = kw.get("adapter", "none")
    rules = {r: 0 for r in SL.RULES}
    rules["10"] = None
    rules["13"] = total
    slips = {
        "rules": rules,
        "total": total,
        "refusals": {},
        "refusals_total": 0,
        "refusals_not_built": {},
        "late_entries": 0,
        "unsettled_requests_warnings": 0,
        "open_commitments_warnings": 0,
        "next_calls": {"suggested_now": 0, "followed": 0},
        "calls": 1,
        "usage": {},
        "misreadings": {"R1_principal": kw.get("r1", 0)},
    }
    (d / "run.json").write_text(json.dumps(meta))
    (d / "slips.json").write_text(json.dumps(slips))


class Report(unittest.TestCase):
    def test_missing_cells_models_and_vendors_are_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            fake_run(out, "claude:claude-sonnet-5-5", "s1-user-series", "g", 1, 0)
            fake_run(out, "claude:claude-sonnet-5-5", "s1-user-series", "nog", 1, 1)
            fake_run(out, "claude:claude-sonnet-5-5", "s3-company", "g", 1, 0)
            fake_run(out, "claude:claude-opus-5-5", "s3-company", "g", 1, 0)
            fake_run(out, "claude:claude-opus-5-5", "s3-company", "nog", 1, 0)
            text = R.render(out)
            self.assertIn("- Every cell is reported, with at least 10 counted runs: no.", text)
            self.assertIn("claude:claude-sonnet-5-5 s3-company without guidance", text)
            self.assertIn("claude:claude-sonnet-5-5 s8-newcomer with guidance", text)
            self.assertIn("every model s8-newcomer without orientation", text)
            self.assertIn("- At least 3 models from at least 2 vendors: no", text)
            self.assertIn("claude:claude-sonnet-5-5: yes (0.00 against 1.00 over 1 scenarios; left out:", text)

    def test_the_campaigns_four_models_meet_the_vendor_criterion_only_when_counted(self):
        """Two models of each vendor are 3 models from 2 vendors. Codex runs with the operator's own
        CODEX_HOME are not isolated and left out of every count, and then the Claude models alone
        do not meet it."""
        from harness import __main__ as HM

        for problems, verdict in (([], "yes"), (["CODEX_HOME is not a dedicated home"], "no")):
            with tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp)
                for m in HM.CAMPAIGN_MODELS:
                    fake_run(out, m, "s1-user-series", "g", 1, 0, problems=problems if m.startswith("codex:") else [])
                text = R.render(out)
                self.assertIn(f"- At least 3 models from at least 2 vendors: {verdict} (", text)

    def test_the_manifest_names_the_cells_and_isolation_problems_are_left_out(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            (out / "manifest.json").write_text(
                json.dumps(
                    {
                        "models": ["claude:claude-sonnet-5-5", "codex:gpt-6.1-sol"],
                        "scenarios": ["s1-user-series"],
                        "arms": ["g", "nog"],
                    }
                )
            )
            fake_run(out, "claude:claude-sonnet-5-5", "s1-user-series", "g", 1, 0)
            fake_run(out, "claude:claude-sonnet-5-5", "s1-user-series", "nog", 1, 1)
            fake_run(
                out,
                "codex:gpt-6.1-sol",
                "s1-user-series",
                "g",
                1,
                0,
                problems=["tools other than the forum's: ['apply_patch']"],
            )
            text = R.render(out)
            self.assertIn("2 models × 1 scenarios × 2 arms", text)
            self.assertIn("codex:gpt-6.1-sol s1-user-series with guidance", text)
            self.assertIn("1 runs were left out of every count for isolation problems", text)
            self.assertIn("(2 counted; 1 left out for isolation problems)", text)

    def test_misreadings_per_model_in_the_arm_with_guidance(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            for k in range(1, 41):
                fake_run(out, "claude:claude-sonnet-5-5", "s1-user-series", "g", k, 0)
                fake_run(out, "claude:claude-sonnet-5-5", "s1-user-series", "nog", k, 0, r1=1)
                fake_run(out, "claude:claude-opus-5-5", "s1-user-series", "g", k, 0, r1=1 if k <= 10 else 0)
            text = R.render(out)
            line = [
                x
                for x in text.splitlines()
                if x.startswith("| R1 (R7): an invented principal") and "claude:claude-sonnet-5-5" in x
            ][0]
            self.assertIn("0 / 40", line)
            self.assertTrue(line.endswith("| falls |"), line)
            line = [
                x
                for x in text.splitlines()
                if x.startswith("| R1 (R7): an invented principal") and "claude:claude-opus-5-5" in x
            ][0]
            self.assertIn("10 / 40", line)
            self.assertIn("does not fall", line)


class Rules(unittest.TestCase):
    def test_reply_rules(self):
        self.assertTrue(RUN.rule_fires({"when": "asked", "match": ["time"]}, "What time suits you?"))
        self.assertFalse(RUN.rule_fires({"when": "asked", "match": ["time"]}, "It runs at a time."))
        self.assertTrue(RUN.rule_fires({"when": "told", "match": ["set up"]}, "It is set up."))
        self.assertTrue(RUN.rule_fires({"when": "asked"}, "Anything else?"))

    def test_every_action_is_answered(self):
        """The roles promise an action's result in the next message: an action that no world rule
        answers gets WORLD_DEFAULT, so that the session is not left with no next message (the first
        final round: Codex sessions whose first reply was an action ended there, never calling the
        forum). A scenario can set its own or none."""
        scen = {
            "id": "t",
            "world": [{"agent": "*", "on": "action", "when": "told", "match": ["gh pr list"], "say": "no PR yet"}],
        }
        with tempfile.TemporaryDirectory() as tmp:
            r = RUN.Run(scen, "script", "none", "g", Path(tmp), Path(tmp) / "none")
            a = {"id": "W"}
            self.assertEqual(
                r._replies(a, "ACTION: gh pr list --head x", set()), [{"from": "world", "text": "[platform] no PR yet"}]
            )
            default = [{"from": "world", "text": f"[platform] {RUN.WORLD_DEFAULT}", "default": True}]
            self.assertEqual(r._replies(a, "Checking.\nACTION: read the work queue", set()), default)
            self.assertEqual(r._replies(a, "All done, nothing outside the forum.", set()), [])
            r._deliver("W", default[0])
            self.assertTrue(r.meta["deliveries"][-1]["default"])
            r.s = dict(scen, world_default=None)
            self.assertEqual(r._replies(a, "ACTION: read the work queue", set()), [])
            r.s = dict(scen, world_default="Done.")
            self.assertEqual(r._replies(a, "ACTION: read the work queue", set())[0]["text"], "[platform] Done.")


BINARY = S.repo_root() / "target" / "debug" / "forum-server"

FAST = {
    "lease_first": "4s",
    "lease_default": "4s",
    "lease_max": "10s",
    "lease_ceiling": "20s",
    "presence_silence": "30s",
    "presence_resolution": "10s",
    "session_retain": "60s",
}


def scripted_scenario() -> dict:
    return {
        "id": "t-scripted",
        "title": "scripted agents for the harness test",
        "params": {"profile": "short", "overrides": FAST},
        "tokens": {"seed": {"class": "seed"}, "worker": {"class": "scheduled-job"}},
        "prelude": [
            {"as": "seed", "tool": "session", "args": {"action": "hello"}},
            {
                "as": "seed",
                "tool": "node",
                "args": {"action": "create", "path": "t/a", "title": "A"},
                "save": {"ta": "token"},
            },
            {
                "as": "seed",
                "tool": "hold",
                "args": {
                    "action": "release",
                    "token": "$ta",
                    "done": "made",
                    "next_step": "do it",
                    "reason": "handoff",
                },
            },
            {
                "as": "seed",
                "tool": "node",
                "args": {"action": "create", "path": "t/b", "title": "B"},
                "save": {"tb": "token"},
            },
            {
                "as": "seed",
                "tool": "hold",
                "args": {
                    "action": "release",
                    "token": "$tb",
                    "done": "made",
                    "next_step": "do it",
                    "reason": "handoff",
                },
            },
        ],
        "agents": [
            {
                "id": "A",
                "token": "worker",
                "role": "-",
                "lines": [{"from": "event", "text": "go"}],
                "script": [
                    [
                        ["session", {"action": "hello"}],
                        ["hold", {"action": "take", "target": "/t/a"}],
                        ["view", {"action": "node", "path": "/t/a"}],
                    ]
                ],
                "end": {"after_calls": 2},
            },
            {
                "id": "B",
                "token": "worker",
                "role": "-",
                "start": {"after": "A", "delay": "6s"},
                "lines": [{"from": "event", "text": "go"}],
                "script": [
                    [
                        ["session", {"action": "hello"}],
                        ["hold", {"action": "take", "target": "/t/a"}],
                        [
                            "node",
                            {
                                "action": "close",
                                "path": "/t/a",
                                "outcome": "done",
                                "summary": "did it",
                                "token": "$token",
                            },
                        ],
                    ]
                ],
            },
            {
                "id": "C",
                "token": "worker",
                "role": "-",
                "lines": [{"from": "event", "text": "go"}],
                "script": [
                    [
                        ["session", {"action": "hello"}],
                        ["node", {"action": "close", "path": "/t/none", "outcome": "done", "summary": "x"}],
                        ["node", {"action": "close", "path": "/t/none", "outcome": "done", "summary": "x"}],
                    ]
                ],
            },
        ],
        "drain": "1s",
    }


@unittest.skipUnless(BINARY.exists(), "needs target/debug/forum-server with the usability feature")
class EndToEnd(unittest.TestCase):
    def test_a_scripted_run_is_counted(self):
        scen = scripted_scenario()
        SC.check(scen)
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp) / "run"
            meta = RUN.Run(scen, "script", "fixed", "g", d, BINARY, log=lambda m: None).execute()
            self.assertEqual(meta["status"], "done", meta.get("error") or meta.get("prelude_failure"))
            slips = json.loads((d / "slips.json").read_text())
            sessions = meta["agent_sessions"]
            self.assertEqual(set(sessions), {"A", "B", "C"})
            # C repeated a refused close as it was, and ended on a refusal.
            self.assertEqual([x["agent"] for x in slips["details"]["E"]], ["C", "C"])
            self.assertEqual(slips["refusals"], {"not_found": 2})
            # A was ended at its fixed point right after its take. With a 4 s first lease its
            # checkpoint was due at once (§20.9: 1 minute before the lease ends, never before the
            # issue), so its expiry without a checkpoint counts; with the short profile's 2 min it
            # would not, being due 1 minute after the take.
            self.assertEqual([x["what"] for x in slips["details"]["3"]], ["expired without progress"])
            self.assertEqual(slips["holds_expired_after_end_point"], [])
            # B closed /t/a and stopped while /t/b was still offered; C never took anything.
            self.assertEqual(sorted(x["agent"] for x in slips["details"]["13"]), ["B", "C"])
            self.assertIsNone(slips["rules"]["10"])
            self.assertEqual(slips["total"], 5)
            self.assertEqual(slips["calls_by_agent"], {"A": 2, "B": 3, "C": 3})
            history = [json.loads(x) for x in (d / "server" / "export.jsonl").read_text().splitlines()]
            self.assertTrue(any("Genesis" in json.dumps(x) for x in history))
            # The tools/list as the server sent it and as the agents were served it.
            self.assertEqual(
                meta["tools_served"],
                {"A": "tools-list.as-served.json", "B": "tools-list.as-served.json", "C": "tools-list.as-served.json"},
            )
            served = json.loads((d / "tools-list.as-served.json").read_text())
            self.assertEqual(sorted(t["name"] for t in served["tools"]), sorted(A.FORUM_TOOLS))
            self.assertTrue((d / "tools-list.server.json").exists())
            self.assertEqual(meta["built_packages"], sorted(PK.BUILT))
            text = R.render(Path(tmp))
            self.assertIn("### script:fixed, t-scripted", text)
            self.assertIn("| 13 |", text)


if __name__ == "__main__":
    unittest.main()
