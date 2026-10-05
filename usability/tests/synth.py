"""Synthetic runs for the slip counter's tests: a history in `control export`'s shape (each
transaction's header, then its events, with the core's serde shapes, D.7.2), the proxy's log, and
the transcripts, built call by call."""

from __future__ import annotations

T0 = 1_791_000_000_000
MIN = 60_000
SEC = 1_000


class Synth:
    def __init__(self, params: dict | None = None):
        self.lines: list[dict] = []
        self.seq = 0
        self.txn = 0
        self.eid = 0
        self.proxy: list[dict] = []
        self.meta: dict = {"agent_sessions": {}, "deliveries": [], "params": {}}
        self.transcripts: dict = {}
        self.agents: list[dict] = []
        self.humans: dict = {}
        p = {"conversation_schemes": ["chat", "issue", "mail"], "too_big_attempts": 3}
        p.update(params or {})
        self.tx(None, T0, "genesis")
        self.ev(T0, "Genesis", {"params": p})

    # -- the history ------------------------------------------------------------------------------

    def tx(self, session=None, at=T0, kind="cmd") -> None:
        self.txn += 1
        self.lines.append(
            {
                "tx": self.txn,
                "kind": kind,
                "session": f"s-{session}" if session is not None else None,
                "class": None,
                "op_id": None,
                "at": at,
            }
        )

    def ev(self, at: int, name: str, fields) -> None:
        self.seq += 1
        self.lines.append({"event": {"at": at, "ev": {name: fields}}, "seq": self.seq})

    def session(self, s: int, at: int = T0, principal=None, caps=(), horizon=None, cls="worker") -> None:
        self.tx(s, at)
        self.ev(
            at,
            "SessionOpened",
            {
                "session": s,
                "class": cls,
                "caps": list(caps),
                "horizon": horizon,
                "intent": None,
                "silence_at": at + 2 * MIN,
                "retire_at": at + 50 * MIN,
                "principal": principal,
            },
        )

    def principal(self, pid: int, key: str, at: int = T0) -> None:
        self.ev(at, "PrincipalSeen", {"id": pid, "key": key})

    def node(
        self,
        nid: int,
        name: str,
        parent: int = 0,
        at: int = T0,
        by: int | None = 1,
        origin: str = "Agent",
        tags=(),
        capacity: int = 1,
        needs=(),
        est=None,
        keys=(),
        role="Plain",
        due=None,
        depends_on=(),
    ) -> int:
        o = {origin: by} if origin in ("Agent", "Ancestor") else origin
        init = {
            "agreements": "",
            "body": "",
            "capacity": capacity,
            "depends_on": list(depends_on),
            "digest": "",
            "due": due,
            "est": est,
            "fields": {},
            "keys": {k: 1 for k in keys},
            "kind": None,
            "links": [],
            "needs": list(needs),
            "owners": [],
            "priority": 0,
            "role": role,
            "status": "",
            "tags": list(tags),
            "title": name,
        }
        self.ev(at, "NodeCreated", {"id": nid, "parent": parent, "name": name, "origin": o, "init": init})
        return nid

    def take(self, node: int, fence: int, s: int, at: int, lease: int = 2 * MIN) -> None:
        self.ev(
            at,
            "HoldIssued",
            {"token": {"node": node, "fence": fence}, "session": s, "lease_until": at + lease, "terms": {}, "l": lease},
        )

    def checkpoint(self, node: int, fence: int, s: int, at: int, lease: int = 3 * MIN, release=None) -> int:
        e = self.append(node, s, at, {"Checkpoint": {"token": {"node": node, "fence": fence}, "release": release}})
        if release is None:
            self.ev(
                at,
                "HoldExtended",
                {"token": {"node": node, "fence": fence}, "lease_until": at + lease, "by": "Checkpoint"},
            )
        return e

    def end_hold(self, node: int, fence: int, at: int, end="Released") -> None:
        self.ev(at, "HoldEnded", {"token": {"node": node, "fence": fence}, "end": end})

    def release(self, node: int, fence: int, s: int, at: int, reason="Handoff") -> None:
        self.checkpoint(node, fence, s, at, release=reason)
        self.end_hold(node, fence, at, "Released")

    def expire(self, node: int, fence: int, at: int) -> None:
        self.tx(None, at, "settle")
        self.ev(at, "Fired", {"kind": "Lease", "owner": {"Hold": {"node": node, "fence": fence}}})
        self.end_hold(node, fence, at, "Expired")

    def append(self, node: int, s, at: int, body, text: str = "") -> int:
        self.eid += 1
        self.ev(
            at,
            "Appended",
            {
                "eid": self.eid,
                "node": node,
                "seq": self.eid,
                "by": {"Session": s} if s is not None else "Server",
                "body": body,
                "text": {"main": text, "extra": []},
            },
        )
        return self.eid

    def close(self, node: int, s, at: int, outcome="Done", fence=None, merged=None) -> None:
        self.append(node, s, at, {"System": {"Closed": {"outcome": outcome}}})
        if fence is not None:
            self.end_hold(node, fence, at, {"EndedByClose": "Close"})
        self.ev(at, "NodeClosed", {"node": node, "outcome": outcome, "merged_into": merged})

    def request(self, node: int, s, at: int, requester="External", from_user=None, conv="chat:c/t") -> int:
        return self.append(
            node,
            s,
            at,
            {
                "Request": {
                    "origin": {"key": f"{conv}/m{self.eid + 1}", "nv": 1},
                    "conv": conv,
                    "from_user": from_user,
                    "requester": requester,
                }
            },
        )

    def decide(self, node: int, s, at: int, settles=(), basis=(), supersedes=None) -> int:
        return self.append(
            node,
            s,
            at,
            {
                "Decision": {
                    "settles": list(settles),
                    "supersedes": supersedes,
                    "assumes": [],
                    "evidence": None,
                    "basis": list(basis),
                }
            },
        )

    # -- the agents -------------------------------------------------------------------------------

    def agent(self, aid: str, sessions, human=None, conversation=None, ended=False, killed_at=None) -> None:
        spec = {"id": aid, "token": "t", "role": "-"}
        if human:
            spec["human"] = human
        if conversation:
            spec["conversation"] = conversation
        self.agents.append(spec)
        self.meta["agent_sessions"][aid] = [f"s-{s}" for s in sessions]
        turns = []
        if killed_at is not None:
            turns.append({"n": 1, "ended_by": "end_point", "ended": killed_at / 1000})
        self.transcripts[aid] = {
            "agent": aid,
            "vendor": "script",
            "turns": turns,
            "ended": ended or killed_at is not None,
        }

    def call(
        self, agent: str, s: int, tool: str, args: dict, t: int, structured=None, error=None, hint="", text=None
    ) -> None:
        body = text
        if body is None:
            body = f"[swarm] running · session s-{s} (w)"
            if error:
                body += f"\nerror: {error}\nhint: {hint}"
        self.proxy.append(
            {
                "t": t / 1000,
                "t_end": (t + 500) / 1000,
                "agent": agent,
                "tool": tool,
                "args": args,
                "text": body,
                "structured": structured if structured is not None else {},
                "is_error": False,
                "session": f"s-{s}",
            }
        )

    def scenario(self) -> dict:
        return {"id": "synthetic", "agents": self.agents, "humans": self.humans}

    def count(self) -> dict:
        from harness import slips as SL

        return SL.count(self.scenario(), self.meta, self.lines, self.proxy, self.transcripts)
