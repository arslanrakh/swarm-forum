"""A minimal MCP client over streamable HTTP, standard library only.

The harness uses it for the scenario's prelude (the scripted sessions that set the forum up before
any agent arrives) and for its own checks; the agents use their vendors' clients, never this one.

The server expires an idle MCP session after `presence.silence` (CS-17), 2 minutes in the short
profile, and answers a later request on it with HTTP 404. A prelude that waits for a deadline (a
`sleep` step) outlasts that, so the client does what the MCP specification asks of a client on a
404: it starts a new MCP session and sends the call again. The new MCP session is bound to no forum
session, so the call names the forum session the client was last shown (`session <session>
(<class>)` in the status line), unless it names one itself: the scripted session goes on in the
same forum session, as an agent passes `session` after a restart (D.11.4).
"""

from __future__ import annotations

import itertools
import json
import re
import urllib.error
import urllib.request

PROTOCOL_VERSION = "2025-06-18"

# The session a status line names: `session s-<n> (<class>)` since protocol 1.7, `you=<class>/s-<n>`
# before it (the runs of earlier rounds logged that form).
STATUS_SESSION = re.compile(r"· session (s-\d+) \(([^)\s]+)\)|you=([^/\s]+)/(s-\d+)")


def status_session(text: str) -> tuple[str, str] | None:
    """`(class, session)` that the first status line in `text` names, or None without one."""
    m = STATUS_SESSION.search(text or "")
    if not m:
        return None
    if m.group(1):
        return m.group(2), m.group(1)
    return m.group(3), m.group(4)


class McpError(Exception):
    """A transport or JSON-RPC failure (not a forum refusal, which is a normal tool result)."""

    def __init__(self, message: str, status: int | None = None):
        super().__init__(message)
        self.status = status


class ToolResult:
    """One `tools/call` result: the text content, the structured content, and the error flag."""

    def __init__(self, raw: dict):
        self.raw = raw
        self.text = "\n".join(c.get("text", "") for c in raw.get("content", []) if c.get("type") == "text")
        self.structured = raw.get("structuredContent")
        self.is_error = bool(raw.get("isError"))

    def __repr__(self) -> str:
        return f"ToolResult(is_error={self.is_error}, text={self.text[:80]!r})"


def _read_messages(resp) -> list[dict]:
    """The JSON-RPC messages of a response: one JSON body, or the `data:` events of an SSE stream."""
    ctype = resp.headers.get("Content-Type", "")
    body = resp.read().decode("utf-8")
    if "text/event-stream" not in ctype:
        return [json.loads(body)] if body.strip() else []
    out, data = [], []
    for line in body.splitlines() + [""]:
        if line.startswith("data:"):
            data.append(line[5:].lstrip())
        elif line == "":
            if data:
                text = "\n".join(data).strip()
                if text:
                    try:
                        out.append(json.loads(text))
                    except json.JSONDecodeError:
                        pass
                data = []
    return out


class McpClient:
    """One MCP session with the forum's `/mcp` endpoint, authenticated by a bearer token."""

    def __init__(self, url: str, token: str, timeout: float = 120.0):
        self.url = url
        self.token = token
        self.timeout = timeout
        self.session_id: str | None = None
        self.ids = itertools.count(1)
        self.instructions: str | None = None
        # The forum session the last status line showed, and how many times an expired MCP session
        # was replaced by a new one (the module's docstring).
        self.forum_session: str | None = None
        self.reopened = 0

    def _post(self, message: dict) -> list[dict]:
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "Authorization": f"Bearer {self.token}",
            "MCP-Protocol-Version": PROTOCOL_VERSION,
        }
        if self.session_id:
            headers["Mcp-Session-Id"] = self.session_id
        req = urllib.request.Request(self.url, data=json.dumps(message).encode(), headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                sid = resp.headers.get("Mcp-Session-Id")
                if sid:
                    self.session_id = sid
                return _read_messages(resp)
        except urllib.error.HTTPError as e:
            with e:
                body = e.read()[:300]
            raise McpError(f"HTTP {e.code}: {body!r}", status=e.code) from e
        except urllib.error.URLError as e:
            raise McpError(str(e)) from e

    def _request(self, method: str, params: dict) -> dict:
        rid = next(self.ids)
        for m in self._post({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}):
            if m.get("id") == rid:
                if "error" in m:
                    raise McpError(f"{method}: {m['error']}")
                return m.get("result", {})
        raise McpError(f"{method}: no response")

    def initialize(self) -> dict:
        result = self._request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "forum-usability-harness", "version": "1"},
            },
        )
        self.instructions = result.get("instructions")
        self._post({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return result

    def list_tools(self) -> list[dict]:
        return self._request("tools/list", {}).get("tools", [])

    def call(self, tool: str, args: dict) -> ToolResult:
        try:
            r = ToolResult(self._request("tools/call", {"name": tool, "arguments": args}))
        except McpError as e:
            if e.status != 404 or not self.session_id:
                raise
            # The server expired the idle MCP session before the call reached a tool: start a new
            # one and send the call again, in the same forum session (the module's docstring).
            self.session_id = None
            self.initialize()
            self.reopened += 1
            if self.forum_session and "session" not in args:
                args = dict(args, session=self.forum_session)
            r = ToolResult(self._request("tools/call", {"name": tool, "arguments": args}))
        found = status_session(r.text)
        if found:
            self.forum_session = found[1]
        return r

    def close(self) -> None:
        if not self.session_id:
            return
        req = urllib.request.Request(
            self.url,
            headers={"Authorization": f"Bearer {self.token}", "Mcp-Session-Id": self.session_id},
            method="DELETE",
        )
        try:
            urllib.request.urlopen(req, timeout=10).close()
        except (urllib.error.URLError, OSError):
            pass
        self.session_id = None
