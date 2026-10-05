"""The MCP pass-through between every agent's client and the forum.

Every agent reaches the forum through this proxy, which does two things:

1. **It logs every `tools/call`** with the server's full response (the text content, with its status
   line, and the structured content) under the agent that made it, known by its bearer token. The
   slip counter reads this log: it is what the server answered, whatever the client then showed
   its model, and its status lines name the forum session each agent used.

2. **It adapts the input schemas for clients that refuse them**, only for agents configured so.
   §20.2 (1.4, R21) gives each tool an input schema that is a top-level `oneOf`, one branch per
   action. The Anthropic API does not accept a top-level `oneOf`, and Claude Code drops every forum
   tool whose schema has one ("Skipping tool ...: its input schema uses top-level oneOf"), so
   without a change to §20.2 no Claude model can call the forum. For such an agent the proxy
   flattens each schema mechanically and changes nothing else (`flatten`):

   - A branch may itself hold a `oneOf` (or `anyOf`): of forms with their own arguments (`talk
     said` records or confirms; `talk ask` asks a human, a person, or agents), or of alternative
     `required` lists (`view node` takes `path` or `paths`). Each branch is expanded into its forms,
     recursively, each form with the branch's arguments and requirements plus its own.
   - The top level becomes `{type: object, properties, required: [action]}`, with
     `additionalProperties: false` only when the properties hold every argument of every form.
   - `action` becomes an enum of the actions. Its description lists, per action and form, the
     arguments that form requires, so that what each `required` said stays visible:
     `said: at, text, kind (recording ...) | said_id, confirmed (confirming ...)`.
   - Every other argument is the union over the forms. Its schema is the one schema all forms give
     it, or, where they differ (`since` is an integer for `feed` and a string for `user_log`;
     `update` allows null where `create` does not), an `anyOf` of the distinct schemas. Its
     description is the one description all forms give it, or each action's own; it ends with the
     actions that take it, `*` marking an action all of whose forms require it and `+` one some of
     whose forms do: `(actions: take*, next; * required, + required in some forms)`.

   Every run records which agents had flattened schemas and saves the tools/list as the server
   sent it and as each agent was served it; the report says so: such a run cannot confirm R21.

   Optionally (`text_only`), the proxy also drops the structured content and the output schemas,
   so that a client that shows its model only the structured content (Claude Code does) shows the
   server's text instead: the status line and the rendered next calls. It is off by default.

All other requests and responses pass through unchanged.
"""

from __future__ import annotations

import copy
import hashlib
import http.server
import json
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

from .mcp import status_session

PASS_HEADERS = [
    "Content-Type",
    "Accept",
    "Authorization",
    "Mcp-Session-Id",
    "MCP-Protocol-Version",
    "Last-Event-ID",
    # MCP 2026-07-28 (SEP-2243): the server refuses a request of that version without them.
    "Mcp-Method",
    "Mcp-Name",
]
PASS_HEADER_PREFIXES = ["Mcp-Param-"]  # SEP-2243, one per annotated argument
BACK_HEADERS = ["Content-Type", "Mcp-Session-Id", "WWW-Authenticate"]


def _canon(x) -> str:
    return json.dumps(x, sort_keys=True)


def forms(branch: dict, props: dict | None = None, required: frozenset = frozenset(), title=None) -> list[tuple]:
    """The forms of one branch, `(title, properties, required)` each: the branch itself, or, when it
    holds a `oneOf` or `anyOf` of object schemas, each of their forms, recursively, with the
    branch's properties and requirements added to their own."""
    p = dict(props or {})
    p.update(branch.get("properties") or {})
    r = required | frozenset(branch.get("required") or [])
    t = branch.get("title") or title
    subs = branch.get("oneOf") or branch.get("anyOf")
    if not isinstance(subs, list) or not subs:
        return [(t, p, r)]
    out = []
    for sub in subs:
        out += forms(sub, p, r, t)
    return out


def action_of(props: dict) -> str:
    a = props.get("action") or {}
    return a.get("const") or (a.get("enum") or ["?"])[0]


def _members(schema: dict) -> list[dict]:
    """A property's schema without its description, as the alternatives it allows."""
    s = {k: v for k, v in schema.items() if k != "description"}
    if set(s) == {"anyOf"} and isinstance(s["anyOf"], list):
        return [dict(m) for m in s["anyOf"]]
    return [s]


def _form_note(title, tool_action: str) -> str:
    """The part of a form's title that tells it from the action's other forms."""
    t = (title or "").strip()
    for prefix in (tool_action, tool_action.split(" ", 1)[-1]):
        if t.startswith(prefix):
            t = t[len(prefix) :]
    return t.lstrip(" ,:").strip()


def flatten(schema: dict, tool: str = "") -> dict:
    """A top-level `oneOf` of per-action object schemas as one object schema (see the module)."""
    branches = schema.get("oneOf")
    if not isinstance(branches, list) or not branches:
        return schema
    actions: list[str] = []
    per_action: dict[str, list[tuple]] = {}
    for b in branches:
        for f in forms(b):
            name = action_of(f[1])
            if name not in per_action:
                actions.append(name)
                per_action[name] = []
            per_action[name].append(f)
    # Every argument: the schema, description, and requirement each action's forms give it.
    seen: dict[str, dict] = {}
    for name in actions:
        for _title, props, req in per_action[name]:
            for k, v in props.items():
                if k == "action":
                    continue
                arg = seen.setdefault(k, {"members": [], "descs": {}, "takers": {}})
                for m in _members(v):
                    if _canon(m) not in (_canon(x) for x in arg["members"]):
                        arg["members"].append(m)
                d = (v.get("description") or "").strip()
                arg["descs"].setdefault(d, [])
                if name not in arg["descs"][d]:
                    arg["descs"][d].append(name)
                arg["takers"].setdefault(name, []).append(k in req)
    out_props: dict[str, dict] = {}
    for k, arg in seen.items():
        v = dict(arg["members"][0]) if len(arg["members"]) == 1 else {"anyOf": arg["members"]}
        marks = {}
        for name, reqs in arg["takers"].items():
            forms_of = len(per_action[name])
            marks[name] = "*" if len(reqs) == forms_of and all(reqs) else ("+" if any(reqs) else "")
        takers = ", ".join(f"{a}{marks[a]}" for a in arg["takers"])
        legend = []
        if "*" in marks.values():
            legend.append("* required")
        if "+" in marks.values():
            legend.append("+ required in some forms")
        # One description for every action that takes the argument, or each action's own (an
        # action whose form gives none is named in the note only).
        if len(arg["descs"]) == 1:
            text = next(iter(arg["descs"]))
        else:
            text = " | ".join(f"{', '.join(arg['descs'][d])}: {d}" for d in arg["descs"] if d)
        if len(arg["takers"]) < len(actions) or legend or len(arg["descs"]) > 1:
            note = f"actions: {takers}" + (f"; {', '.join(legend)}" if legend else "")
            text = f"{text} ({note})".strip()
        if text:
            v["description"] = text
        out_props[k] = v
    # What each action's forms require, so that no `required` of §20.2 is lost.
    lines = []
    for name in actions:
        alts = []
        for title, props, req in per_action[name]:
            args = sorted(k for k in props if k in req and k != "action")
            note = _form_note(title, f"{tool} {name}".strip()) if len(per_action[name]) > 1 else ""
            alts.append((", ".join(args) or "nothing") + (f" ({note})" if note else ""))
        lines.append(f"{name}: {' | '.join(alts)}")
    # §20.2's own description of `action` speaks of branches, which the flat schema no longer has.
    action_prop = {
        "type": "string",
        "enum": actions,
        "description": "The action. Each argument's description names the actions that take it. "
        f"Required, per action (and form): {'; '.join(lines)}",
    }
    every = set().union(*(set(p) for fs in per_action.values() for _t, p, _r in fs))
    out = {"type": "object", "properties": {"action": action_prop, **out_props}, "required": ["action"]}
    if every <= set(out["properties"]):
        out["additionalProperties"] = False
    return out


def lost_arguments(schema: dict, flat: dict) -> list[str]:
    """The arguments of any form of `schema` that `flat` does not have (none, for `flatten`)."""
    every: set = set()
    for b in schema.get("oneOf") or []:
        for _t, p, _r in forms(b):
            every |= set(p)
    return sorted(every - set((flat.get("properties") or {})))


def token_hash(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()[:16]


class Proxy:
    """A local HTTP server that forwards `/mcp` to the forum, logging and adapting per agent."""

    def __init__(self, upstream: str, log_path=None, dump_dir=None):
        self.upstream = upstream
        self.agents: dict[str, dict] = {}  # token hash → {"name", "flatten", "text_only"}
        self.lock = threading.Lock()
        self.log = open(log_path, "a") if log_path else None
        # Where the tools/list is saved: as the server sent it, and as each kind of agent was served it.
        self.dump_dir = Path(dump_dir) if dump_dir else None
        self.served: dict[str, str] = {}  # agent name → the file of the tools/list it was served
        self.sessions: dict[str, set] = {}  # agent name → forum sessions seen
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):
                pass

            def _forward(self, method: str):
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length) if length else None
                msgs = []
                if body:
                    try:
                        m = json.loads(body)
                        msgs = m if isinstance(m, list) else [m]
                    except json.JSONDecodeError:
                        msgs = []
                auth = self.headers.get("Authorization", "")
                agent = proxy.agents.get(token_hash(auth.removeprefix("Bearer ").strip()), {})
                headers = {h: self.headers[h] for h in PASS_HEADERS if self.headers.get(h)}
                for h, v in self.headers.items():
                    if any(h.lower().startswith(p.lower()) for p in PASS_HEADER_PREFIXES):
                        headers[h] = v
                req = urllib.request.Request(proxy.upstream, data=body, headers=headers, method=method)
                t0 = time.time()
                try:
                    resp = urllib.request.urlopen(req, timeout=600)
                    status, rh, data = resp.status, resp.headers, resp.read()
                except urllib.error.HTTPError as e:
                    status, rh, data = e.code, e.headers, e.read()
                except (urllib.error.URLError, OSError) as e:
                    self.send_error(502, str(e))
                    return
                if status == 200 and msgs:
                    data = proxy._handle(
                        agent, msgs, data, rh.get("Content-Type", ""), t0, self.headers.get("Mcp-Session-Id")
                    )
                self.send_response(status)
                for h in BACK_HEADERS:
                    if rh.get(h):
                        self.send_header(h, rh[h])
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_POST(self):
                self._forward("POST")

            def do_DELETE(self):
                self._forward("DELETE")

            def do_GET(self):
                # The forum is pull-only and sends nothing unasked: no listening stream.
                self.send_response(405)
                self.send_header("Allow", "POST, DELETE")
                self.send_header("Content-Length", "0")
                self.end_headers()

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}/mcp"
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    def register(self, token: str, name: str, flatten_schemas: bool = False, text_only: bool = False) -> None:
        self.agents[token_hash(token)] = {"name": name, "flatten": flatten_schemas, "text_only": text_only}

    def _handle(
        self, agent: dict, msgs: list[dict], data: bytes, ctype: str, t0: float, mcp_session: str | None = None
    ) -> bytes:
        by_id = {m.get("id"): m for m in msgs if "id" in m}
        sse = "text/event-stream" in ctype
        text = data.decode("utf-8")
        events = text.split("\n\n") if sse else [text]
        out = []
        for event in events:
            if sse:
                lines = event.split("\n")
                payload = "\n".join(x[5:].lstrip() for x in lines if x.startswith("data:"))
                rest = [x for x in lines if not x.startswith("data:")]
            else:
                payload, rest = event, []
            try:
                reply = json.loads(payload) if payload.strip() else None
            except json.JSONDecodeError:
                reply = None
            if not isinstance(reply, dict) or reply.get("id") not in by_id:
                out.append(event)
                continue
            request = by_id[reply["id"]]
            reply = self._adapt(agent, request, reply, t0, mcp_session)
            body = json.dumps(reply)
            out.append("\n".join(rest + ["data: " + body]) if sse else body)
        return ("\n\n".join(out) if sse else out[0]).encode()

    def _adapt(self, agent: dict, request: dict, reply: dict, t0: float, mcp_session: str | None = None) -> dict:
        method = request.get("method")
        result = reply.get("result")
        if not isinstance(result, dict):
            return reply
        if method == "tools/list":
            raw = copy.deepcopy(result)
            for t in result.get("tools", []):
                if agent.get("flatten") and isinstance(t.get("inputSchema"), dict):
                    t["inputSchema"] = flatten(t["inputSchema"], t.get("name", ""))
                if agent.get("text_only"):
                    t.pop("outputSchema", None)
            self._dump(agent, raw, result)
        elif method == "tools/call":
            params = request.get("params") or {}
            text = "\n".join(c.get("text", "") for c in result.get("content", []) if c.get("type") == "text")
            found = status_session(text.split("\n", 1)[0]) if text else None
            name = agent.get("name", "?")
            if found:
                with self.lock:
                    self.sessions.setdefault(name, set()).add(found[1])
            if self.log:
                rec = {
                    "t": t0,
                    "t_end": time.time(),
                    "agent": name,
                    "tool": params.get("name"),
                    "args": params.get("arguments"),
                    "text": text,
                    "structured": result.get("structuredContent"),
                    "is_error": bool(result.get("isError")),
                    "session": found[1] if found else None,
                    # The MCP session the call came on (none for a client that keeps none): what the server's
                    # session binding (§3) can use when the call names no session.
                    "mcp_session": mcp_session,
                }
                with self.lock:
                    self.log.write(json.dumps(rec) + "\n")
                    self.log.flush()
            if agent.get("text_only"):
                result.pop("structuredContent", None)
        return reply

    def _dump(self, agent: dict, raw: dict, served: dict) -> None:
        """Saves the tools/list as the server sent it (`tools-list.server.json`) and as this kind of
        agent was served it (`tools-list.<flattened|as-served>[.text-only].json`), once each."""
        if not self.dump_dir:
            return
        variant = ("flattened" if agent.get("flatten") else "as-served") + (
            ".text-only" if agent.get("text_only") else ""
        )
        name = f"tools-list.{variant}.json"
        with self.lock:
            self.dump_dir.mkdir(parents=True, exist_ok=True)
            for fname, data in (("tools-list.server.json", raw), (name, served)):
                p = self.dump_dir / fname
                if not p.exists():
                    p.write_text(json.dumps(data, indent=1, sort_keys=True, ensure_ascii=False))
            self.served[agent.get("name", "?")] = name

    def start(self) -> "Proxy":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()
        if self.log:
            self.log.close()
