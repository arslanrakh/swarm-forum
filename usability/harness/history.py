"""The server's history replayed into the tables the slip counter needs.

`History(lines, until)` reads `control export`'s JSON lines (`forum-server usability-export`): each
transaction's header (`{tx, kind, session, class, at}`) and its events (`{event: {at, ev}, seq}`), in
order, up to the time `until` (all of them without it). The tables mirror the core's state where the
counter needs it, from the events alone (D.7.2): nodes and their places, holds and their leases,
waits, intents, requests, `said` entries, decisions and their basis, objections, notes and agent
questions and their readers, questions for the user and their stages, user entries and their
routing, digest entries, commitments, marks, pauses, refusals, and each session's principals.

Some state the core derives without an event (the offer tag a hold's end adds, the attempts an
expiry counts, a wait's resolution, the flags of §14) is derived here by the same rules. The flags
are those the history can show; `flags` names which. This is the counter's model, approximate where
it says so, and never the core's: the counter reports what it counted and why, so that a reader can
check a slip against the history.
"""

from __future__ import annotations

ROOT, USER, USER_DECISIONS, USER_DIGEST, SYS, SIGNALS, PEOPLE, TEAMS = range(8)
RESERVED = {
    ROOT: (None, ""),
    USER: (ROOT, "@user"),
    USER_DECISIONS: (USER, "decisions"),
    USER_DIGEST: (USER, "digest"),
    SYS: (ROOT, "sys"),
    SIGNALS: (SYS, "signals"),
    PEOPLE: (ROOT, "@people"),
    TEAMS: (ROOT, "@teams"),
}
USER_PRINCIPAL = 0
OFFER = "offer"
MINUTE = 60_000
# §20.9: a hold's checkpoint is due at the earlier of 5 minutes after its issue, last checkpoint, or
# renewal, and 1 minute before its lease ends, but never before that issue, checkpoint, or renewal.
CHECKPOINT_EVERY = 5 * MINUTE
LEASE_MARGIN = MINUTE
# The flags of §14 that the history shows, and those that make a closed node a candidate (§10.4).
CLOSED_CANDIDATE_FLAGS = {
    "unanswered_request",
    "open_commitment",
    "objection",
    "open_intent",
    "unconfirmed_said",
    "basis_changed",
}
# The flags a close would leave on a node (§10.1: a take reopens a closed node that carries one).
REOPENING_FLAGS = CLOSED_CANDIDATE_FLAGS


def variant(x):
    """`{"Name": {...}}`, `{"Name": value}`, or `"Name"` → (name, fields); a value is `{"_": value}`."""
    if isinstance(x, str):
        return x, {}
    if isinstance(x, dict) and len(x) == 1:
        ((k, v),) = x.items()
        return k, v if isinstance(v, dict) else {"_": v}
    return "?", {}


def sid(s) -> int | None:
    """`3`, or `"s-3"` → 3."""
    if isinstance(s, int):
        return s
    if isinstance(s, str) and s.startswith("s-") and s[2:].isdigit():
        return int(s[2:])
    return None


def tok(t: dict) -> tuple:
    return (t.get("node"), t.get("fence"))


def change(c, old):
    """A `Change<T>` of a patch: `"Keep"`, `"Clear"`, or `{"Set": v}`."""
    name, f = variant(c) if c is not None else ("Keep", {})
    if name == "Set":
        return f.get("_", f)
    if name == "Clear":
        return None
    return old


def outcome_name(o) -> str:
    name, f = variant(o)
    return f"other:{f.get('_')}" if name == "Other" else name.lower()


class History:
    """The history up to `until` (ms), replayed (see the module)."""

    def __init__(self, lines: list[dict], until: int | None = None):
        self.until = until
        self.events: list[tuple] = []  # (seq, at, name, fields, tx)
        self.now = 0
        self.params: dict = {}
        self.nodes: dict[int, dict] = {}
        for n, (parent, name) in RESERVED.items():
            self.nodes[n] = self._node(n, parent, name, "Plain", "Reserved", None, {"capacity": 0}, 0)
        self.holds: dict[tuple, dict] = {}
        self.intent_open: dict[int, dict] = {}
        self.intents: list[dict] = []
        self.entries: dict[int, dict] = {}
        self.requests: dict[int, dict] = {}
        self.saids: dict[int, dict] = {}
        self.decisions: dict[int, dict] = {}
        self.objections: dict[int, dict] = {}
        self.notes: dict[int, dict] = {}  # notes and agent questions: addressed messages
        self.answers: dict[int, dict] = {}
        self.pquestions: dict[int, dict] = {}
        self.uquestions: dict[int, dict] = {}
        self.from_user: dict[int, dict] = {}
        self.to_user: dict[int, dict] = {}
        self.digests: dict[int, dict] = {}
        self.commitments: dict[int, dict] = {}
        self.marks: dict[int, set] = {}
        self.retracted: set = set()  # ("Node", id) or ("Entry", eid)
        self.sessions: dict[int, dict] = {}
        self.principal_key: dict[int, str] = {USER_PRINCIPAL: "user"}
        self.pauses: dict[int, dict] = {}
        self.refusals: dict[tuple, list] = {}
        self.shown: list[tuple] = []  # (at, session, node, seq)
        self.inbox_read: dict[int, dict] = {}  # session → {note: at}
        self.series_missed: dict[int, bool] = {}
        self.closes: list[dict] = []  # every close: the node, who, when, and its requests then
        self.reopens: list[dict] = []
        self.late = 0
        self._tx: dict = {}
        self._tx_hooks: list = []
        for line in lines:
            if "event" not in line:
                if "tx" in line:
                    self._end_tx()
                    self._tx = {
                        "tx": line.get("tx"),
                        "kind": line.get("kind"),
                        "session": sid(line.get("session")),
                        "class": line.get("class"),
                    }
                continue
            e = line["event"]
            at = e.get("at") or 0
            if until is not None and at > until:
                break
            name, f = variant(e.get("ev"))
            self.now = max(self.now, at)
            self.events.append((line.get("seq"), at, name, f, dict(self._tx)))
            getattr(self, "_" + name, self._other)(at, f)
        self._end_tx()
        if until is not None:
            self.now = max(self.now, until)

    # -- the replay -------------------------------------------------------------------------------

    def _end_tx(self) -> None:
        for hook in self._tx_hooks:
            hook()
        self._tx_hooks = []

    def _other(self, at, f):
        pass

    @staticmethod
    def _node(n, parent, name, role, origin, by, init, at) -> dict:
        role_name, role_f = variant(role)
        return {
            "id": n,
            "parent": parent,
            "name": name,
            "role": role_name,
            "role_f": role_f,
            "origin": origin,
            "by": by,
            "capacity": init.get("capacity", 1),
            "priority": init.get("priority", 0),
            "tags": set(init.get("tags") or []),
            "needs": set(init.get("needs") or []),
            "est": init.get("est"),
            "due": init.get("due"),
            "depends_on": set(init.get("depends_on") or []),
            "keys": set((init.get("keys") or {}).keys()),
            "owners": set(init.get("owners") or []),
            "created": at,
            "open": True,
            "outcome": None,
            "merged_into": None,
            "attempts": 0,
            "available": role_name != "Occurrence" or variant(role_f.get("phase", "Waiting"))[0] == "Available",
            "wait": None,
        }

    def _Genesis(self, at, f):
        self.params = dict(f.get("params") or {})

    def _ParamsChanged(self, at, f):
        self.params = dict(f.get("params") or {})

    def _SessionOpened(self, at, f):
        s = f["session"]
        self.sessions[s] = {
            "class": f.get("class"),
            "caps": set(f.get("caps") or []),
            "horizon": f.get("horizon"),
            "opened": at,
            "principals": {},
            "last": at,
        }
        if f.get("principal") is not None:
            self.sessions[s]["principals"].setdefault(f["principal"], at)

    def _session(self, s, at) -> dict:
        x = self.sessions.setdefault(
            s, {"class": None, "caps": set(), "horizon": None, "opened": at, "principals": {}, "last": at}
        )
        x["last"] = max(x["last"], at)
        return x

    def _SessionHello(self, at, f):
        s = self._session(f["session"], at)
        if f.get("caps") is not None:
            s["caps"] = set(f["caps"])
        if f.get("horizon") is not None:
            s["horizon"] = f["horizon"]
        if f.get("principal") is not None:
            s["principals"].setdefault(f["principal"], at)

    def _Touched(self, at, f):
        self._session(f["session"], at)

    def _PrincipalSeen(self, at, f):
        self.principal_key[f["id"]] = f.get("key")

    def _InboxRead(self, at, f):
        self._session(f["session"], at)
        r = self.inbox_read.setdefault(f["session"], {})
        for x in f.get("notes") or []:
            r.setdefault(x, at)

    def _Shown(self, at, f):
        self._session(f["session"], at)
        self.shown.append((at, f["session"], f["node"], f["seq"]))

    def _NodeCreated(self, at, f):
        origin, ov = variant(f.get("origin"))
        by = ov.get("_") if origin in ("Agent", "Ancestor") else None
        init = f.get("init") or {}
        self.nodes[f["id"]] = self._node(
            f["id"], f.get("parent"), f.get("name", ""), init.get("role", "Plain"), origin, by, init, at
        )

    def _NodeUpdated(self, at, f):
        n = self.nodes.get(f["node"])
        if not n:
            return
        p = f.get("patch") or {}
        for k in ("capacity", "priority"):
            if p.get(k) is not None:
                n[k] = p[k]
        for k in ("tags", "needs", "depends_on", "owners"):
            if p.get(k) is not None:
                n[k] = set(p[k])
        n["est"] = change(p.get("est"), n["est"])
        n["due"] = change(p.get("due"), n["due"])

    def _KeyAdded(self, at, f):
        n = self.nodes.get(f["node"])
        if n:
            n["keys"].add((f.get("key") or {}).get("key"))

    def _KeyRemoved(self, at, f):
        n = self.nodes.get(f["node"])
        if n:
            n["keys"].discard(f.get("key"))

    def _NodeMoved(self, at, f):
        n = self.nodes.get(f["node"])
        if n:
            n["parent"] = f.get("parent")
            n["name"] = f.get("name", n["name"])

    def _NodeClosed(self, at, f):
        n = self.nodes.get(f["node"])
        if not n:
            return
        n["open"] = False
        n["outcome"] = outcome_name(f.get("outcome"))
        n["merged_into"] = f.get("merged_into")
        n["tags"].discard(OFFER)
        n["wait"] = None
        tx = dict(self._tx)
        node = f["node"]
        merged = f.get("merged_into") is not None

        def snapshot():
            # The requests on the node when the close's transaction ended, and what each lacked.
            reqs = []
            for r, q in self.requests.items():
                if q["node"] != node or q["settled_at"] is not None:
                    continue
                reqs.append({"request": r, "answered": q["answered_at"] is not None, "decided": self.decided(r)})
            # The closer: the transaction's session, or else the author of the close's entry.
            who = tx.get("session")
            if who is None:
                who = next(
                    (
                        e["by"]
                        for e in self.entries.values()
                        if e["node"] == node and e["at"] == at and e["type"] == "System" and "Closed" in e["body"]
                    ),
                    None,
                )
            self.closes.append({"node": node, "at": at, "session": who, "merged": merged, "unsettled": reqs})

        self._tx_hooks.append(snapshot)

    def _NodeReopened(self, at, f):
        n = self.nodes.get(f["node"])
        if not n:
            return
        n["open"] = True
        n["outcome"] = None
        tx = dict(self._tx)
        node = f["node"]

        def record():
            # A take reopens a node in the same transaction as its hold (§10.1).
            by_take = any(
                h["node"] == node and h["issued"] == at and h["session"] == tx.get("session")
                for h in self.holds.values()
            )
            self.reopens.append({"node": node, "at": at, "session": tx.get("session"), "by_take": by_take})

        self._tx_hooks.append(record)

    def _OfferSet(self, at, f):
        n = self.nodes.get(f["node"])
        if n:
            (n["tags"].add if f.get("on") else n["tags"].discard)(OFFER)

    def _WaitSet(self, at, f):
        n = self.nodes.get(f["node"])
        if n and n["open"]:
            w = f.get("wait")
            n["wait"] = None if w is None else {"on": variant(w), "set_at": at}

    def _DependsRedirected(self, at, f):
        n = self.nodes.get(f["node"])
        if n and f.get("from") in n["depends_on"]:
            n["depends_on"].discard(f["from"])
            if f.get("to") is not None:
                n["depends_on"].add(f["to"])

    def _MarkSet(self, at, f):
        m = (variant(f.get("mark"))[0], f.get("question"))
        s = self.marks.setdefault(f["node"], set())
        (s.add if f.get("on") else s.discard)(m)

    def _IntentOpened(self, at, f):
        it = f.get("intent") or {}
        rec = {
            "node": f["node"],
            "eid": it.get("eid"),
            "session": it.get("session"),
            "opened": at,
            "closed": None,
            "end": None,
            "closed_by": None,
        }
        self.intent_open[f["node"]] = rec
        self.intents.append(rec)

    def _IntentClosed(self, at, f):
        rec = self.intent_open.pop(f["node"], None)
        if rec:
            rec["closed"] = at
            rec["end"] = variant(f.get("end"))[0]
            rec["closed_by"] = self._tx.get("session")

    def _IntentMoved(self, at, f):
        rec = self.intent_open.pop(f.get("from"), None)
        if rec:
            rec["node"] = f.get("to")
            self.intent_open[f.get("to")] = rec

    def _SeriesOutcome(self, at, f):
        self.series_missed[f["series"]] = bool(f.get("missed"))

    def _OccurrenceAvailable(self, at, f):
        n = self.nodes.get(f["node"])
        if n:
            n["available"] = True

    def _OccurrenceDetached(self, at, f):
        n = self.nodes.get(f["node"])
        if n:
            n["role"], n["available"] = "Plain", True

    def _SignalDetached(self, at, f):
        n = self.nodes.get(f["node"])
        if n:
            n["role"] = "Plain"

    def _HoldIssued(self, at, f):
        node, fence = tok(f["token"])
        self.holds[(node, fence)] = {
            "node": node,
            "fence": fence,
            "session": f["session"],
            "issued": at,
            "lease_until": f.get("lease_until"),
            "last_ext": at,
            "checkpoints": 0,
            "end": None,
            "how": None,
            "ended": None,
        }
        self._session(f["session"], at)

    def _HoldExtended(self, at, f):
        h = self.holds.get(tok(f["token"]))
        if h is None:
            return
        h["lease_until"] = f.get("lease_until", h["lease_until"])
        h["last_ext"] = at
        if variant(f.get("by"))[0] == "Checkpoint":
            h["checkpoints"] += 1

    def _HoldEnded(self, at, f):
        h = self.holds.get(tok(f["token"]))
        end, how = variant(f.get("end"))
        cause = how.get("_") if end == "EndedByClose" else None
        if h is not None:
            h["end"], h["how"], h["ended"] = end, cause, at
        n = self.nodes.get(tok(f["token"])[0])
        if not n:
            return
        # evolve (D.7.4): an expiry or another session's move without a checkpoint is an attempt;
        # a release, an expiry, or a move tags the node `offer`, if it is open and no container.
        if end == "Expired" or cause == "Move":
            if h is not None and h["checkpoints"] == 0:
                n["attempts"] += 1
        if (end in ("Released", "Expired") or cause == "Move") and n["open"] and not self.is_container(n["id"]):
            n["tags"].add(OFFER)

    def _Appended(self, at, f):
        body, b = variant(f.get("body"))
        by_kind, bv = variant(f.get("by"))
        by = bv.get("_") if by_kind == "Session" else None
        eid, node = f["eid"], f["node"]
        e = {
            "eid": eid,
            "node": node,
            "seq": f.get("seq"),
            "at": at,
            "type": body,
            "body": b,
            "by": by,
            "text": (f.get("text") or {}).get("main", ""),
            "tx_session": self._tx.get("session"),
        }
        self.entries[eid] = e
        if by is not None:
            self._session(by, at)
        handler = getattr(self, "_entry_" + body, None)
        if handler:
            handler(e, b)

    def _entry_Late(self, e, b):
        self.late += 1

    def _entry_Checkpoint(self, e, b):
        h = self.holds.get(tok(b.get("token") or {}))
        if h is not None and h["checkpoints"] == 0:
            h["checkpoints"] = 1  # a release counts as a checkpoint (A-9)

    def _entry_Request(self, e, b):
        who, wv = variant(b.get("requester"))
        self.requests[e["eid"]] = {
            "eid": e["eid"],
            "node": e["node"],
            "at": e["at"],
            "by": e["by"],
            "requester": wv.get("_") if who == "User" else None,
            "from_user": b.get("from_user"),
            "conv": b.get("conv"),
            "answered_at": None,
            "settled_at": None,
        }

    def _entry_Said(self, e, b):
        self.saids[e["eid"]] = {
            "eid": e["eid"],
            "node": e["node"],
            "by": e["by"],
            "at": e["at"],
            "conv": b.get("conv"),
            "kind": variant(b.get("kind"))[0],
            "confirmed_at": None,
            "unconfirmed_at": None,
        }

    def _entry_Decision(self, e, b):
        self.decisions[e["eid"]] = {
            "eid": e["eid"],
            "node": e["node"],
            "by": e["by"],
            "at": e["at"],
            "settles": set(b.get("settles") or []),
            "supersedes": b.get("supersedes"),
            "basis": list(b.get("basis") or []),
            "active": True,
        }

    def _entry_Object(self, e, b):
        self.objections[e["eid"]] = {"eid": e["eid"], "node": e["node"], "by": e["by"], "at": e["at"], "open": True}

    def _entry_Retract(self, e, b):
        kind, tv = variant(b.get("target"))
        self.retracted.add((kind, tv.get("_")))

    def _addressed(self, e, b, kind: str) -> None:
        to = b.get("to")
        self.notes[e["eid"]] = {
            "eid": e["eid"],
            "node": e["node"],
            "by": e["by"],
            "at": e["at"],
            "kind": kind,
            "to": variant(to) if to is not None else None,
            "until": b.get("until"),
            "recipients": set(b.get("recipients") or []),
            "pending_holder": bool(b.get("pending_holder")),
            "bound": {},
            "answered_at": None,
        }

    def _entry_Note(self, e, b):
        self._addressed(e, b, "note")

    def _entry_Question(self, e, b):
        kind, q = variant(b)
        if kind == "Agent":
            self._addressed(e, q, "question")
        elif kind == "People":
            self.pquestions[e["eid"]] = {
                "eid": e["eid"],
                "node": e["node"],
                "by": e["by"],
                "at": e["at"],
                "conv": q.get("conv"),
                "state": "open",
                "rechecks": 0,
            }
        elif kind == "User":
            self.uquestions[e["eid"]] = {
                "eid": e["eid"],
                "node": e["node"],
                "about": q.get("about"),
                "by": e["by"],
                "at": e["at"],
                "chain": list(q.get("to") or []),
                "stages": [{"stage": 0, "start": e["at"], "due": q.get("due")}],
                "ended": None,
                "ended_at": None,
            }

    def _entry_Answer(self, e, b):
        self.answers[e["eid"]] = {
            "eid": e["eid"],
            "question": b.get("question"),
            "recipient": b.get("recipient"),
            "until": b.get("until"),
            "at": e["at"],
            "by": e["by"],
        }

    def _entry_FromUser(self, e, b):
        p = b.get("principal")
        self.from_user[e["eid"]] = {
            "eid": e["eid"],
            "by": e["by"],
            "principal": p,
            "at": e["at"],
            "text": e["text"],
            "routed": None,
        }
        # §3: a session serves the principals of the from_user entries it records.
        if e["by"] is not None and p is not None:
            self._session(e["by"], e["at"])["principals"].setdefault(p, e["at"])

    def _entry_ToUser(self, e, b):
        self.to_user[e["eid"]] = {
            "eid": e["eid"],
            "by": e["by"],
            "principal": b.get("principal"),
            "kind": variant(b.get("kind"))[0],
            "re": set(b.get("re") or []),
            "at": e["at"],
            "node": e["node"],
        }

    def _entry_System(self, e, b):
        kind, s = variant(b)
        if kind == "Digest":
            self.digests[e["eid"]] = {"eid": e["eid"], "to": s.get("to"), "at": e["at"], "shown_at": None}

    def _RequestAnswered(self, at, f):
        r = self.requests.get(f["request"])
        if r and r["answered_at"] is None:
            r["answered_at"] = at

    def _RequestSettled(self, at, f):
        r = self.requests.get(f["request"])
        if r and r["settled_at"] is None:
            r["settled_at"] = at

    def _RequestMoved(self, at, f):
        r = self.requests.get(f["request"])
        if r:
            r["node"] = f.get("to")

    def _PqRechecked(self, at, f):
        q = self.pquestions.get(f["q"])
        if q:
            q["rechecks"] = f.get("rechecks", q["rechecks"] + 1)

    def _pq_end(self, f, state):
        q = self.pquestions.get(f["q"])
        if q and q["state"] == "open":
            q["state"] = state

    def _PqExpired(self, at, f):
        self._pq_end(f, "expired")

    def _PqAnswered(self, at, f):
        self._pq_end(f, "answered")

    def _PqSuperseded(self, at, f):
        self._pq_end(f, "superseded")

    def _PqMoved(self, at, f):
        q = self.pquestions.get(f["q"])
        if q:
            q["node"] = f.get("to")

    def _AqAnswered(self, at, f):
        n = self.notes.get(f["q"])
        if n and n["answered_at"] is None:
            n["answered_at"] = at

    def _uq_end(self, at, f, how):
        q = self.uquestions.get(f["q"])
        if q and q["ended"] is None:
            q["ended"], q["ended_at"] = how, at

    def _UqAnswered(self, at, f):
        self._uq_end(at, f, "answered")

    def _UqDefaulted(self, at, f):
        self._uq_end(at, f, "defaulted")

    def _UqLapsed(self, at, f):
        self._uq_end(at, f, "lapsed")

    def _UqReaddressed(self, at, f):
        q = self.uquestions.get(f["q"])
        if q:
            q["stages"].append({"stage": f.get("stage"), "start": at, "due": f.get("due")})

    def _CommitmentOpened(self, at, f):
        self.commitments[f["c"]] = {
            "c": f["c"],
            "node": f.get("node"),
            "conv": f.get("conv"),
            "followup_at": f.get("followup_at"),
            "opened": at,
            "fulfilled_at": None,
        }

    def _CommitmentRefreshed(self, at, f):
        c = self.commitments.get(f["c"])
        if c:
            c["followup_at"] = f.get("followup_at")

    def _CommitmentFulfilled(self, at, f):
        c = self.commitments.get(f["c"])
        if c and c["fulfilled_at"] is None:
            c["fulfilled_at"] = at

    def _CommitmentMoved(self, at, f):
        c = self.commitments.get(f["c"])
        if c:
            c["node"] = f.get("to")

    def _SaidConfirmed(self, at, f):
        s = self.saids.get(f["said"])
        if s and s["confirmed_at"] is None:
            s["confirmed_at"] = at

    def _SaidUnconfirmed(self, at, f):
        s = self.saids.get(f["said"])
        if s and s["unconfirmed_at"] is None:
            s["unconfirmed_at"] = at

    def _SaidMoved(self, at, f):
        s = self.saids.get(f["said"])
        if s:
            s["node"] = f.get("to")

    def _EntryRouted(self, at, f):
        u = self.from_user.get(f["entry"])
        if u is not None and u["routed"] is None:
            u["routed"] = {"at": at, "session": self._tx.get("session"), "how": variant(f.get("routing"))[0]}

    def _DecisionSuperseded(self, at, f):
        d = self.decisions.get(f["d"])
        if d:
            d["active"] = False

    def _DecisionRetracted(self, at, f):
        d = self.decisions.get(f["d"])
        if d:
            d["active"] = False
        self.retracted.add(("Entry", f["d"]))

    def _ObjectionResolved(self, at, f):
        o = self.objections.get(f["o"])
        if o:
            o["open"] = False

    def _ObjectionMoved(self, at, f):
        o = self.objections.get(f["o"])
        if o:
            o["node"] = f.get("to")

    def _DigestShown(self, at, f):
        d = self.digests.get(f["entry"])
        if d and d["shown_at"] is None:
            d["shown_at"] = at

    def _Refused(self, at, f):
        self.refusals.setdefault((f["node"], f.get("class")), []).append(
            {
                "session": f.get("session"),
                "backoff_until": f.get("backoff_until"),
                "expires_at": f.get("expires_at"),
                "quorum": f.get("quorum") or 1,
            }
        )

    def _RefusalExpired(self, at, f):
        key = (f["node"], f.get("class"))
        self.refusals[key] = [r for r in self.refusals.get(key, []) if (r["expires_at"] or 0) > at]

    def _RefusalsCleared(self, at, f):
        for key in [k for k in self.refusals if k[0] == f["node"]]:
            del self.refusals[key]

    def _Paused(self, at, f):
        self.pauses[f["scope"]] = {"until": f.get("until"), "active": True}

    def _Unpaused(self, at, f):
        self.pauses.pop(f["scope"], None)

    def _PauseExpired(self, at, f):
        self.pauses.pop(f["scope"], None)

    def _NoteBound(self, at, f):
        n = self.notes.get(f["note"])
        if n:
            n["bound"].setdefault(f["session"], at)

    # -- the tree ---------------------------------------------------------------------------------

    def path(self, n: int) -> str:
        parts = []
        seen = set()
        while n is not None and n != ROOT and n not in seen:
            seen.add(n)
            x = self.nodes.get(n)
            if not x:
                break
            parts.append(x["name"])
            n = x["parent"]
        return "/" + "/".join(reversed(parts))

    def node_by_ref(self, ref) -> int | None:
        """`#12`, `12`, or a path → the node's id (by its current path)."""
        if isinstance(ref, int):
            return ref
        if not isinstance(ref, str):
            return None
        if ref.startswith("#") and ref[1:].isdigit():
            return int(ref[1:])
        want = "/" + ref.strip("/")
        for n in self.nodes:
            if self.path(n) == want:
                return n
        return None

    def ancestors(self, n: int) -> list[int]:
        """`n`'s ancestors, nearest first (not `n`)."""
        out, seen = [], {n}
        x = self.nodes.get(n)
        while x and x["parent"] is not None and x["parent"] not in seen:
            seen.add(x["parent"])
            out.append(x["parent"])
            x = self.nodes.get(x["parent"])
        return out

    def children(self, n: int) -> list[int]:
        return [m for m, x in self.nodes.items() if x["parent"] == n and m != n]

    def descendants(self, n: int) -> set[int]:
        out, todo = set(), [n]
        while todo:
            for c in self.children(todo.pop()):
                if c not in out:
                    out.add(c)
                    todo.append(c)
        return out

    def is_reserved(self, n: int) -> bool:
        return n in RESERVED

    def is_container(self, n: int) -> bool:
        """A series, or a person or team of the directory (§4)."""
        x = self.nodes.get(n)
        return bool(x) and (x["role"] == "Series" or x["parent"] in (PEOPLE, TEAMS))

    def conversation_node(self, n: int) -> int | None:
        """The nearest node from `n` up that carries a conversation key (§9.1)."""
        schemes = set(self.params.get("conversation_schemes") or ["chat", "issue", "mail"])
        for m in [n] + self.ancestors(n):
            if self.is_reserved(m):
                continue
            if any((k or "").split(":", 1)[0] in schemes for k in self.nodes.get(m, {}).get("keys", ())):
                return m
        return None

    # -- holds and waits --------------------------------------------------------------------------

    def live_holds(self, n: int, at: int | None = None) -> list[dict]:
        t = self.now if at is None else at
        return [
            h
            for h in self.holds.values()
            if h["node"] == n and h["issued"] <= t and (h["ended"] is None or h["ended"] > t)
        ]

    def held_by(self, session: int, at: int | None = None) -> list[dict]:
        t = self.now if at is None else at
        return [
            h
            for h in self.holds.values()
            if h["session"] == session and h["issued"] <= t and (h["ended"] is None or h["ended"] > t)
        ]

    def unattended(self, n: int) -> bool:
        return not any(self.live_holds(m) for m in {n} | self.descendants(n))

    def waiting(self, n: int) -> bool:
        """The node has an unresolved wait now (§10.3, §14)."""
        x = self.nodes.get(n)
        w = x and x["wait"]
        if not w or not x["open"]:
            return False
        kind, f = w["on"]
        v = f.get("_")
        if kind in ("Time", "External"):
            return v is None or self.now < v
        if kind == "Node":
            target = self.nodes.get(v)
            return bool(target) and target["open"]
        if kind == "UserQuestion":
            q = self.uquestions.get(v)
            return bool(q) and q["ended"] is None
        if kind == "Question":
            q = self.pquestions.get(f.get("q"))
            return bool(q) and q["state"] == "open" and q["rechecks"] <= (f.get("rechecks_at_set") or 0)
        return True

    # -- requests, decisions, and the basis -------------------------------------------------------

    def decided(self, r: int) -> bool:
        return any(r in d["settles"] for d in self.decisions.values() if d["active"])

    def request_of_entry(self, e: int) -> int | None:
        """A request named directly, or through its user entry (§20.9's "in reach")."""
        if e in self.requests:
            return e
        for r, q in self.requests.items():
            if q["from_user"] == e:
                return r
        return None

    def stale(self, d: dict) -> bool:
        """An active decision with a stale basis reference (§14 `basis_changed`, §15)."""
        if not d["active"]:
            return False
        for ref in d["basis"]:
            kind, f = variant(ref)
            if kind == "Node":
                n = f.get("id")
                x = self.nodes.get(n)
                if ("Node", n) in self.retracted:
                    return True
                if x and not x["open"] and x["merged_into"] is None:
                    then = f.get("life_at")
                    was = outcome_name(then) if then is not None else "done"
                    if x["outcome"] != was:
                        return True
            elif kind == "Entry":
                e = f.get("_")
                cited = self.decisions.get(e)
                if ("Entry", e) in self.retracted or (cited is not None and not cited["active"]):
                    return True
        return False

    # -- flags (§14), as far as the history shows them --------------------------------------------

    def flags(self, n: int) -> set[str]:
        """The flags of §14 on `n` now that the history shows: all but `too_big`'s attempts from
        before the run, `open_commitment`'s open question to that person, and `missed`'s detail."""
        x = self.nodes.get(n)
        if not x or self.is_reserved(n):
            return set()
        out = set()
        closed = not x["open"]
        unattended = self.unattended(n)
        waiting = self.waiting(n)
        if any(q["node"] == n and q["settled_at"] is None for q in self.requests.values()):
            if closed or (unattended and not waiting):
                out.add("unanswered_request")
        for c in self.commitments.values():
            if c["node"] == n and c["fulfilled_at"] is None:
                if closed or (unattended and c["followup_at"] is not None and self.now >= c["followup_at"]):
                    out.add("open_commitment")
        if unattended and any(
            s["node"] == n and s["unconfirmed_at"] is not None and s["confirmed_at"] is None
            for s in self.saids.values()
        ):
            out.add("unconfirmed_said")
        if n in self.intent_open and not self.live_holds(n) and not waiting:
            out.add("open_intent")
        for mark, _q in self.marks.get(n, ()):
            out.add("question_recheck" if mark == "Recheck" else "proceed_on_assumption")
        if any(o["node"] == n and o["open"] for o in self.objections.values()):
            out.add("objection")
        if x["merged_into"] is None and any(d["node"] == n and self.stale(d) for d in self.decisions.values()):
            out.add("basis_changed")
        if x["open"]:
            if x["attempts"] >= int(self.params.get("too_big_attempts", 3)):
                out.add("too_big")
            if x["due"] is not None and x["due"] <= self.now:
                out.add("overdue")
            kids = self.children(n)
            if kids and all(not self.nodes[k]["open"] for k in kids):
                out.add("ready_to_close")
            if x["role"] == "Series" and self.series_missed.get(n):
                out.add("missed")
        return out

    # -- principals -------------------------------------------------------------------------------

    def person_of(self, p: int) -> int | None:
        key = self.principal_key.get(p)
        if key is None:
            return None
        for m, x in self.nodes.items():
            if x["open"] and x["parent"] == PEOPLE and f"person:{key}" in x["keys"]:
                return m
        return None

    def aliases(self, p: int) -> set[int]:
        m = self.person_of(p)
        if m is None:
            return {p}
        keys = {k[len("person:") :] for k in self.nodes[m]["keys"] if (k or "").startswith("person:")}
        out = {i for i, k in self.principal_key.items() if k in keys}
        return out | {p}

    def same_person(self, p, q) -> bool:
        return p == q or (p is not None and q is not None and q in self.aliases(p))

    def serves(self, s: int, p: int, at: int | None = None) -> bool:
        """Whether session `s` serves `p`'s person (at `at`, or at any time)."""
        for q, since in self.sessions.get(s, {}).get("principals", {}).items():
            if (at is None or since <= at) and self.same_person(q, p):
                return True
        return False

    # -- next (§10.4) -----------------------------------------------------------------------------

    def paused(self, n: int) -> bool:
        return any(m in self.pauses and (self.pauses[m]["until"] or 0) > self.now for m in [n] + self.ancestors(n))

    def refused_by(self, n: int, cls) -> bool:
        """The class has refused the node (a quorum of its sessions' live refusals) or is backed
        off from it (a refusal's backoff has not passed) (§10.3)."""
        live = [r for r in self.refusals.get((n, cls), []) if (r["expires_at"] or 0) > self.now]
        if not live:
            return False
        quorum = live[0]["quorum"]
        return len({r["session"] for r in live}) >= quorum or any((r["backoff_until"] or 0) > self.now for r in live)

    def candidates(self, session: int) -> list[dict]:
        """What `next` would offer `session` now (§10.4), as far as the history shows: nodes with
        the offer tag or flags, available occurrences, signals, closed nodes with the flags that
        make them candidates, status items, and routing items; less the exclusions of §10.4."""
        s = self.sessions.get(session, {})
        caps, horizon, cls = s.get("caps", set()), s.get("horizon"), s.get("class")
        mine = {h["node"] for h in self.held_by(session)}
        out = []
        for n, x in self.nodes.items():
            if self.is_reserved(n) or x["merged_into"] is not None or n in mine:
                continue
            flags = self.flags(n)
            why = None
            status = False
            if x["open"]:
                free = len(self.live_holds(n)) < max(0, x["capacity"])
                if self.waiting(n):
                    if "open_commitment" in flags and self.unattended(n):
                        why, status = "status item: open_commitment", True
                elif free:
                    if flags:
                        why = "flags: " + ", ".join(sorted(flags))
                    elif x["role"] == "Occurrence":
                        why = "available occurrence" if x["available"] else None
                    elif x["role"] == "Signal" or x["origin"] == "Signal":
                        why = "signal"
                    elif OFFER in x["tags"] and not self.is_container(n):
                        why = "offer tag"
            else:
                closed_flags = flags & CLOSED_CANDIDATE_FLAGS
                if closed_flags:
                    if x["role"] == "Occurrence":
                        series = self.nodes.get(x["role_f"].get("series"))
                        if series and series["open"]:
                            closed_flags = set()
                    if closed_flags:
                        why = "closed, flags: " + ", ".join(sorted(closed_flags))
            if not why:
                continue
            if any(self.nodes.get(d, {}).get("open") for d in x["depends_on"]):
                continue
            if self.refused_by(n, cls) or self.paused(n):
                continue
            if not x["needs"] <= caps:
                continue
            # Status items too: they are subject to every exclusion but the wait (§10.4).
            if horizon is not None and x["est"] is not None and x["est"] > horizon:
                continue
            out.append({"node": n, "path": self.path(n), "why": why, "status": status})
        for e, u in self.from_user.items():
            if u["routed"] is None:
                out.append({"entry": e, "why": "routing item: an unrouted user entry"})
        return out
