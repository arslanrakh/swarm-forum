"""The slip counter: after a run, the slip each rule of §20.9 names, and the other measures.

It reads three sources:

- the server's history (`forum-server usability-export`, `control export`'s JSON lines), replayed by
  `history.History` into what the agents left behind: requests, `said` entries, intents, holds,
  user entries, questions, notes, and the flags the history shows;
- the proxy's log, the server's full response to every call, by agent: refusals, warnings, the
  next calls the response suggested, `leave_better`, and the arguments of each call (`unsent`);
- the agents' transcripts: turns, the fixed points at which the harness ended a session, and usage.

Each rule's slip is counted per run as RULES says, judged at the moment the rule's "When" names
(a close at its close, a worker at its end), whatever happened later. What the harness's fixed
points caused rather than the agent is reported apart (`apart`), never as a slip. A rule whose slip
the history and the log cannot show (10 for now) is reported as not measured, never as zero. The
definitions are mechanical and approximate where the protocol leaves judgement to the agent; each
slip's detail says what was counted, and README.md lists the definitions.
"""

from __future__ import annotations

import re

from . import packages as PK
from .history import CHECKPOINT_EVERY, LEASE_MARGIN, PEOPLE, REOPENING_FLAGS, TEAMS, History, sid, variant

# The rules of §20.9, with the slip each one names (the "Prevents" column) as this counter measures
# it.
RULES = {
    "E": "the same mistake again right after its refusal, or a session that ended on a refusal",
    "1": "a line from the agent's own human never logged as from_user",
    "2": "a said entry never confirmed, unless listed in unsent",
    "3": "a hold that expired with its checkpoint due; an intent left open by the last agent that held its node",
    "4": "a said, to_user, or close written without the newest request, said, heard, question, or answer on its node or conversation node",
    "5": "a user entry never routed, or routed by another session",
    "6": "a request left decided but not answered; a report to the wrong person or of the wrong kind",
    "7": "a request left to a stranger: at a close, without a decision or an answer; at the end, owed a decision",
    "8": "a series, person, or team closed; a finished node left held to expire; a node a take reopened, left open with no flag",
    "9": "a node split off that nothing offers (never held, no offer tag, no flag, no children, open)",
    "10": "a flag on a held node left for the next holder",
    "11": "a note or an agent's answer delivered and never read; a colleague's question left unanswered",
    "12": "a question for a served human defaulted or lapsed unasked; a digest entry left unshown while they were served",
    "13": "a worker that ended with leave_better or a joined node untouched, or while next would offer it work",
}
NOT_MEASURED = {"10"}

WARNING_CODE = re.compile(r'"code"\s*:\s*"([a-z_]+)"|\bcode:\s*([a-z_]+)')
ERROR_LINE = re.compile(r"^error: ([a-z_]+)", re.M)
HINT_LINE = re.compile(r"^hint: (.*)$", re.M)
EID = re.compile(r"^e(\d+)$")


def norm(text: str) -> str:
    return " ".join((text or "").lower().split())


def call_error(text: str) -> tuple[str | None, str]:
    m = ERROR_LINE.search(text or "")
    if not m:
        return None, ""
    h = HINT_LINE.search(text)
    return m.group(1), h.group(1) if h else ""


def next_now(structured) -> dict | None:
    if not isinstance(structured, dict):
        return None
    nc = structured.get("next_calls")
    if not isinstance(nc, dict):
        return None
    for item in nc.get("items") or []:
        if item.get("when") == "now":
            return item
    return None


def args_action(c: dict) -> str:
    return str((c.get("args") or {}).get("action", "?"))


def looks_like_key(v) -> bool:
    return isinstance(v, str) and (re.fullmatch(r"e\d+", v) is not None or (":" in v and not v.startswith(("/", "#"))))


def eid_of(x) -> int | None:
    if isinstance(x, int):
        return x
    m = EID.match(str(x or "").strip())
    return int(m.group(1)) if m else None


class Run:
    """What the counter knows of the run besides the history: the agents, their sessions and calls,
    and the fixed points at which the harness ended them."""

    def __init__(self, scn: dict, meta: dict, proxy_log: list[dict], transcripts: dict):
        self.agent_sessions = {a: {sid(s) for s in ss} - {None} for a, ss in meta.get("agent_sessions", {}).items()}
        self.calls_by_agent: dict[str, list[dict]] = {}
        for rec in proxy_log:
            self.calls_by_agent.setdefault(rec["agent"], []).append(rec)
            s = sid(rec.get("session"))
            if s is not None:
                self.agent_sessions.setdefault(rec["agent"], set()).add(s)
        self.sess_agent = {s: a for a, ss in self.agent_sessions.items() for s in ss}
        self.mine = set(self.sess_agent)
        self.spec = {a["id"]: a for a in scn.get("agents", [])}
        self.transcripts = transcripts
        self.built = set(meta.get("built_packages") or PK.BUILT)
        # The calls of each session: when each started and ended (ms).
        self.call_times: dict[int, list[tuple[int, int]]] = {}
        for rec in proxy_log:
            s = sid(rec.get("session"))
            if s is not None:
                t0 = int(rec.get("t", 0) * 1000)
                self.call_times.setdefault(s, []).append((t0, int(rec.get("t_end", rec.get("t", 0)) * 1000)))
        for v in self.call_times.values():
            v.sort()
        # The fixed points: when the harness ended an agent mid-task (ms).
        self.killed_at: dict[str, int] = {}
        for aid, tr in transcripts.items():
            for t in tr.get("turns", []):
                if t.get("ended_by") == "end_point" and t.get("ended"):
                    self.killed_at[aid] = int(t["ended"] * 1000)

    def agent(self, s) -> str | None:
        return self.sess_agent.get(s)

    def last_call(self, s) -> int | None:
        """When the session's last call ended (ms)."""
        t = self.call_times.get(s)
        return max(e for _s, e in t) if t else None

    def called_between(self, s, a: int, b: int) -> bool:
        """The session made a call that ended after `a` and by `b`: its status line showed what
        had reached it by `a`."""
        return any(a < e <= b for _s, e in self.call_times.get(s, []))

    def called_after(self, s, a: int) -> bool:
        """The session started a call after `a`."""
        return any(st > a for st, _e in self.call_times.get(s, []))

    def killed(self, s) -> int | None:
        return self.killed_at.get(self.agent(s) or "")

    def is_refused(self, c: dict) -> bool:
        return call_error(c.get("text") or "")[0] is not None

    def not_built(self, c: dict) -> bool:
        code, hint = call_error(c.get("text") or "")
        return PK.not_built(c.get("tool"), args_action(c), c.get("args"), code, hint, self.built)


def count(scn: dict, meta: dict, history: list[dict], proxy_log: list[dict], transcripts: dict) -> dict:
    H = History(history)
    R = Run(scn, meta, proxy_log, transcripts)
    slips: dict[str, list] = {k: [] for k in RULES}
    apart: dict[str, list] = {
        "expired_after_end_point": [],
        "intent_left_at_end_point": [],
        "said_at_end_point": [],
        "said_listed_unsent": [],
        "finished_hold_at_end_point": [],
    }
    # The run's end: its last event or its last call, whichever is later (reads record nothing).
    end_t = max([H.now] + [e for v in R.call_times.values() for _s, e in v])

    rule_e(R, slips)
    rule_1(H, R, meta, slips)
    rule_2(H, R, slips, apart)
    rule_3(H, R, slips, apart)
    rule_4(H, R, slips)
    rule_5(H, R, slips)
    rules_6_7(H, R, slips)
    rule_8(H, R, history, slips, apart)
    rule_9(H, R, slips)
    rule_11(H, R, end_t, slips)
    rule_12(H, R, end_t, slips)
    rule_13(R, history, slips)

    measures = other_measures(scn, R)
    per_rule = {k: (None if k in NOT_MEASURED else len(v)) for k, v in slips.items()}
    total = sum(v for v in per_rule.values() if v is not None)
    return {
        "rules": per_rule,
        "total": total,
        "details": slips,
        "apart": apart,
        **measures,
        "late_entries": H.late,
        "holds_live_at_end": [f"{h['node']}.{h['fence']}" for h in H.holds.values() if h["end"] is None],
        "holds_expired_after_end_point": [x["hold"] for x in apart["expired_after_end_point"]],
        "history_events": len(H.events),
        "end_time": end_t,
        "built_packages": sorted(R.built),
    }


# -- E ---------------------------------------------------------------------------------------------


def rule_e(R: Run, slips: dict) -> None:
    """A refused call repeated as it was, or a session that ended on a refusal."""
    for aid, calls in R.calls_by_agent.items():
        for i, c in enumerate(calls):
            code, hint = call_error(c["text"])
            if not code or R.not_built(c):
                continue
            if i + 1 < len(calls):
                # The same mistake again: the next call, of the same tool and action, is refused
                # with the same code for the same argument (the remedy was not applied).
                n = calls[i + 1]
                same = n["tool"] == c["tool"] and args_action(n) == args_action(c)
                ncode, nhint = call_error(n["text"])
                if same and ncode == code and not R.not_built(n) and nhint.split(":", 1)[0] == hint.split(":", 1)[0]:
                    slips["E"].append(
                        {
                            "agent": aid,
                            "call": i + 1,
                            "code": code,
                            "what": "the same mistake again",
                            "hint": hint[:100],
                        }
                    )
            elif not R.transcripts.get(aid, {}).get("ended"):
                slips["E"].append({"agent": aid, "call": i + 1, "code": code, "what": "ended on a refusal"})


# -- 1 ---------------------------------------------------------------------------------------------


def rule_1(H: History, R: Run, meta: dict, slips: dict) -> None:
    """The agent's own human's lines, never logged."""
    for d in meta.get("deliveries", []):
        if d["from"] != "human":
            continue
        logged = [norm(u["text"]) for u in H.from_user.values() if R.agent(u["by"]) == d["agent"]]
        want = norm(d["text"])
        if not any(want in got or (got and got in want) for got in logged):
            slips["1"].append({"agent": d["agent"], "text": d["text"][:80]})


# -- 2 ---------------------------------------------------------------------------------------------


def unsent_lists(R: Run) -> dict[int, set]:
    """The `said` entries each session listed in `unsent` at a release or close that went through."""
    out: dict[int, set] = {}
    for calls in R.calls_by_agent.values():
        for c in calls:
            if (c["tool"], args_action(c)) not in (("hold", "release"), ("node", "close")) or R.is_refused(c):
                continue
            s = sid(c.get("session")) or sid((c.get("args") or {}).get("session"))
            for x in (c.get("args") or {}).get("unsent") or []:
                e = eid_of(x)
                if e is not None and s is not None:
                    out.setdefault(s, set()).add(e)
    return out


def rule_2(H: History, R: Run, slips: dict, apart: dict) -> None:
    """`said` entries never confirmed: rule 2's slip is a sent message left pending, so one the
    session listed in `unsent` is not one, and one recorded by the very call at which the harness
    ended the session is the fixed point's doing."""
    unsent = unsent_lists(R)
    for s in H.saids.values():
        if s["by"] not in R.mine or s["confirmed_at"] is not None:
            continue
        aid = R.agent(s["by"])
        rec = {"agent": aid, "said": s["eid"], "unconfirmed": s["unconfirmed_at"] is not None}
        if s["eid"] in unsent.get(s["by"], set()):
            apart["said_listed_unsent"].append(rec)
            continue
        if R.killed(s["by"]) is not None and not R.called_after(s["by"], s["at"]):
            # Recorded by the session's last call, after which the harness ended it.
            apart["said_at_end_point"].append(rec)
            continue
        slips["2"].append(rec)


# -- 3 ---------------------------------------------------------------------------------------------


def checkpoint_due(h: dict) -> int:
    """§20.9: the earlier of 5 minutes after the issue, last checkpoint, or renewal and 1 minute
    before the lease ends, never before that issue, checkpoint, or renewal."""
    last = h["last_ext"]
    return max(last, min(last + CHECKPOINT_EVERY, (h["lease_until"] or last) - LEASE_MARGIN))


def rule_3(H: History, R: Run, slips: dict, apart: dict) -> None:
    """Holds that expired with their checkpoint due (from the issue, the last checkpoint, or the
    last renewal), unless the harness ended the session before it was due; and intents left open,
    charged to the last agent that held their node after they opened."""
    for h in H.holds.values():
        if h["session"] not in R.mine or h["end"] != "Expired":
            continue
        aid = R.agent(h["session"])
        due = checkpoint_due(h)
        rec = {
            "agent": aid,
            "hold": f"{h['node']}.{h['fence']}",
            "checkpoints": h["checkpoints"],
            "due": due,
            "what": "expired without progress" if h["checkpoints"] == 0 else "expired after its last checkpoint",
        }
        k = R.killed(h["session"])
        if k is not None and k < due:
            apart["expired_after_end_point"].append(rec)
        else:
            slips["3"].append(rec)
    for it in H.intent_open.values():
        holders = sorted(
            (
                h
                for h in H.holds.values()
                if h["node"] == it["node"]
                and h["session"] in R.mine
                and (h["ended"] is None or h["ended"] >= it["opened"])
            ),
            key=lambda h: h["issued"],
        )
        if not holders:
            continue  # no agent held it after: an offered intent nobody took is rule 13's
        last = holders[-1]
        rec = {"agent": R.agent(last["session"]), "node": it["node"], "intent": it["eid"], "what": "open intent left"}
        k = R.killed(last["session"])
        if last["session"] == it["session"] and k is not None and k >= it["opened"]:
            apart["intent_left_at_end_point"].append(rec)
        else:
            if last["session"] != it["session"]:
                rec["what"] = "another session's open intent left unchecked"
            slips["3"].append(rec)


# -- 4 ---------------------------------------------------------------------------------------------

WATCHED = {"Request", "Said", "Heard", "Question", "Answer"}


def shown_before(H: History, session: int, node: int, at: int) -> int:
    best = 0
    for t, s, n, q in H.shown:
        if s == session and n == node and t <= at:
            best = max(best, q)
    return best


def rule_4(H: History, R: Run, slips: dict) -> None:
    """A said, to_user, or close written without the newest message: on the node, those by another
    session after the writer's hold was issued and past its `seen`; on its conversation node, when
    that is another node, those past its `seen`, whenever they were recorded."""
    for e in H.entries.values():
        is_write = e["type"] in ("Said", "ToUser") or (e["type"] == "System" and "Closed" in e["body"])
        if not is_write or e["by"] not in R.mine:
            continue
        held = [
            h
            for h in H.holds.values()
            if h["node"] == e["node"]
            and h["session"] == e["by"]
            and h["issued"] <= e["at"]
            and (h["ended"] is None or h["ended"] >= e["at"])
        ]
        since = max((h["issued"] for h in held), default=None)
        places = [(e["node"], since)]
        conv = H.conversation_node(e["node"])
        if conv is not None and conv != e["node"]:
            places.append((conv, None))
        unseen = []
        for n, since in places:
            seen = shown_before(H, e["by"], n, e["at"])
            unseen += [
                x["eid"]
                for x in H.entries.values()
                if x["node"] == n
                and x["type"] in WATCHED
                and x["by"] != e["by"]
                and (x["seq"] or 0) > seen
                and x["at"] <= e["at"]
                and x["eid"] != e["eid"]
                and (since is None or x["at"] >= since)
            ]
        if unseen:
            slips["4"].append({"agent": R.agent(e["by"]), "entry": e["eid"], "unseen": unseen[:5]})


# -- 5 ---------------------------------------------------------------------------------------------


def rule_5(H: History, R: Run, slips: dict) -> None:
    """A user entry the agent recorded and did not route itself: never routed, or routed by
    another session (a stranger, without its context)."""
    for u in H.from_user.values():
        if u["by"] not in R.mine:
            continue
        if u["routed"] is None:
            slips["5"].append({"agent": R.agent(u["by"]), "entry": u["eid"], "what": "never routed"})
        elif u["routed"]["session"] != u["by"]:
            slips["5"].append(
                {
                    "agent": R.agent(u["by"]),
                    "entry": u["eid"],
                    "what": "routed by another session",
                    "by": u["routed"]["session"],
                }
            )


# -- 6 and 7 ---------------------------------------------------------------------------------------


def last_agent_holder(H: History, R: Run, node: int, since: int) -> dict | None:
    hs = [
        h
        for h in H.holds.values()
        if h["node"] == node and h["session"] in R.mine and (h["ended"] is None or h["ended"] >= since)
    ]
    return max(hs, key=lambda h: h["issued"]) if hs else None


def rules_6_7(H: History, R: Run, slips: dict) -> None:
    """Requests. Each agent close is judged at the close: a request on the node then unsettled
    lacked a decision, an answer, or both, and the closer left it to a stranger, whoever settled
    it later. A request no close left is judged at the end, when an agent held its node after it was
    recorded or an agent recorded it. Then the reports: a result to someone other than the requester
    (any alias), and a report of another kind naming a request that was never answered."""
    judged: set = set()
    for c in H.closes:
        if c["session"] not in R.mine or c["merged"]:
            continue
        for q in c["unsettled"]:
            r = q["request"]
            if r in judged:
                continue
            judged.add(r)
            rec = {"agent": R.agent(c["session"]), "request": r, "node": c["node"]}
            if q["decided"] and not q["answered"]:
                slips["6"].append({**rec, "what": "closed decided, without a reply"})
            elif q["answered"] and not q["decided"]:
                slips["7"].append({**rec, "what": "closed answered, without a decision"})
            elif not q["answered"] and not q["decided"]:
                slips["7"].append({**rec, "what": "closed with neither a decision nor an answer"})
    for r, q in H.requests.items():
        if q["settled_at"] is not None or r in judged:
            continue
        holder = last_agent_holder(H, R, q["node"], q["at"])
        who = holder["session"] if holder else (q["by"] if q["by"] in R.mine else None)
        if who is None:
            continue
        judged.add(r)
        rec = {"agent": R.agent(who), "request": r, "node": q["node"]}
        decided, answered = H.decided(r), q["answered_at"] is not None
        if decided and not answered:
            slips["6"].append({**rec, "what": "decided, never answered"})
        elif answered and not decided:
            slips["7"].append({**rec, "what": "answered, no decision"})
        elif not answered and not decided:
            slips["7"].append({**rec, "what": "never decided or answered"})
    wrong_kind: set = set()
    for t in H.to_user.values():
        if t["by"] not in R.mine:
            continue
        for x in sorted(t["re"]):
            r = H.request_of_entry(x)
            q = H.requests.get(r) if r is not None else None
            if not q or q["requester"] is None:
                continue
            rec = {"agent": R.agent(t["by"]), "entry": t["eid"], "request": r}
            if t["kind"] == "Result" and not H.same_person(q["requester"], t["principal"]):
                slips["6"].append({**rec, "what": "result to someone else"})
            elif t["kind"] != "Result" and q["answered_at"] is None and r not in wrong_kind:
                wrong_kind.add(r)
                slips["6"].append({**rec, "what": f"report of kind {t['kind'].lower()}, never a result"})


# -- 8 ---------------------------------------------------------------------------------------------


def close_flags(H: History, n: int) -> set:
    """The flags a close of `n` would leave on it now (§10.1, §14)."""
    out = set()
    if any(q["node"] == n and q["settled_at"] is None for q in H.requests.values()):
        out.add("unanswered_request")
    if any(c["node"] == n and c["fulfilled_at"] is None for c in H.commitments.values()):
        out.add("open_commitment")
    if any(o["node"] == n and o["open"] for o in H.objections.values()):
        out.add("objection")
    if n in H.intent_open:
        out.add("open_intent")
    if any(s["node"] == n and s["unconfirmed_at"] is not None and s["confirmed_at"] is None for s in H.saids.values()):
        out.add("unconfirmed_said")
    if any(d["node"] == n and H.stale(d) for d in H.decisions.values()):
        out.add("basis_changed")
    return out & REOPENING_FLAGS


def finished(H: History, h: dict) -> str | None:
    """Why rule 8 would close the hold's node now, if it would (§20.9 rule 8, not its container
    branch): the close is clean, and the work shows as done."""
    n = h["node"]
    x = H.nodes.get(n)
    if not x or not x["open"] or H.is_container(n) or H.is_reserved(n):
        return None
    unsettled = [q for q in H.requests.values() if q["node"] == n and q["settled_at"] is None]
    clean = (
        all(q["answered_at"] is not None for q in unsettled)
        and len(unsettled) <= 5
        and not any(c["node"] == n and c["fulfilled_at"] is None for c in H.commitments.values())
        and not any(o["node"] == n and o["open"] for o in H.objections.values())
        and not any(o["session"] != h["session"] for o in H.live_holds(n))
        and not any(H.nodes[c]["open"] for c in H.children(n))
        and not (n in H.intent_open and H.intent_open[n]["session"] != h["session"])
    )
    if not clean:
        return None
    if unsettled:
        return "all its unsettled requests answered"
    if "ready_to_close" in H.flags(n):
        return "ready_to_close"
    if any(d["node"] == n and d["by"] == h["session"] and d["at"] >= h["issued"] for d in H.decisions.values()):
        return "a decision recorded during the hold"
    if any(
        i["node"] == n and i["end"] == "Done" and i["closed_by"] == h["session"] and (i["closed"] or 0) >= h["issued"]
        for i in H.intents
    ):
        return "its intent closed with intent_done during the hold"
    return None


def rule_8(H: History, R: Run, history: list[dict], slips: dict, apart: dict) -> None:
    """Containers closed; a finished node left held, which expires as an attempt; a node a take
    reopened and left open once the flags a close would leave are gone."""
    for c in H.closes:
        if c["session"] not in R.mine or c["merged"]:
            continue
        x = H.nodes.get(c["node"], {})
        if x.get("role") == "Series":
            slips["8"].append({"agent": R.agent(c["session"]), "node": c["node"], "what": "series closed"})
        if x.get("parent") in (PEOPLE, TEAMS):
            slips["8"].append({"agent": R.agent(c["session"]), "node": c["node"], "what": "person or team closed"})
    for h in H.holds.values():
        if h["session"] not in R.mine or h["end"] != "Expired":
            continue
        why = finished(History(history, until=h["ended"] - 1), h)
        if not why:
            continue
        rec = {
            "agent": R.agent(h["session"]),
            "node": h["node"],
            "hold": f"{h['node']}.{h['fence']}",
            "what": f"finished node left held, which expired ({why})",
        }
        k = R.killed(h["session"])
        (apart["finished_hold_at_end_point"] if k is not None and k < h["ended"] else slips["8"]).append(rec)
    last_reopen: dict = {}
    for ro in H.reopens:
        last_reopen[ro["node"]] = ro
    for n, ro in last_reopen.items():
        x = H.nodes.get(n)
        if not ro["by_take"] or ro["session"] not in R.mine or not x or not x["open"] or H.live_holds(n):
            continue
        if not close_flags(H, n):
            slips["8"].append(
                {"agent": R.agent(ro["session"]), "node": n, "what": "reopened by a take, left open with no flag left"}
            )


# -- 9 ---------------------------------------------------------------------------------------------


def rule_9(H: History, R: Run, slips: dict) -> None:
    """A node an agent created that nothing offers at the end: never held, open, capacity 1 or
    more, no offer tag, no flag, not waiting, not a container, an occurrence, or a signal, and no
    children (a branch's work is its children's)."""
    held = {h["node"] for h in H.holds.values()}
    for n, x in H.nodes.items():
        if x["origin"] != "Agent" or x["by"] not in R.mine or not x["open"] or n in held:
            continue
        if (
            x["capacity"] < 1
            or x["role"] != "Plain"
            or H.is_container(n)
            or "offer" in x["tags"]
            or H.flags(n)
            or H.waiting(n)
            or H.children(n)
        ):
            continue
        slips["9"].append({"agent": R.agent(x["by"]), "node": n, "path": H.path(n)})


# -- 11 --------------------------------------------------------------------------------------------


def deliveries(H: History, R: Run, note: dict) -> dict[int, int]:
    """The agent sessions a note or agent question reached, and when: its fixed recipients; a
    `holder:` note's taker (NoteBound); a `principal:` note, the sessions serving that person (any
    alias); a `holders:` note, the sessions holding in the branch. The sender never reads its own
    `holder:` or `holders:` note (1.4, R12)."""
    out: dict[int, int] = {}
    for s in note["recipients"]:
        out[s] = note["at"]
    for s, t in note["bound"].items():
        out.setdefault(s, t)
    kind, tv = note["to"] or (None, {})
    v = tv.get("_")
    if kind == "Principal":
        for s in R.mine:
            since = [t for p, t in H.sessions.get(s, {}).get("principals", {}).items() if H.same_person(p, v)]
            if since:
                out.setdefault(s, max(note["at"], min(since)))
    elif kind == "Holders" and v is not None:
        scope = {v} | H.descendants(v)
        for h in H.holds.values():
            if h["session"] in R.mine and h["session"] != note["by"] and h["node"] in scope:
                if h["ended"] is None or h["ended"] > note["at"]:
                    out.setdefault(h["session"], max(note["at"], h["issued"]))
    return {s: t for s, t in out.items() if s in R.mine}


def rule_11(H: History, R: Run, end_t: int, slips: dict) -> None:
    """A note, an agent question, or an answer delivered to an agent session that made a call after
    the delivery (its status line showed the inbox) and before the note's `until`, and never read it;
    and an agent question that reached an agent session and that nobody answered by the end."""
    for note in H.notes.values():
        until = note["until"] if note["until"] is not None else end_t
        reached = deliveries(H, R, note)
        for s, t0 in reached.items():
            read = H.inbox_read.get(s, {}).get(note["eid"])
            if read is not None and read <= until:
                continue
            if R.called_between(s, t0, min(until, end_t)):
                what = "agent question never read" if note["kind"] == "question" else "note never read"
                slips["11"].append({"agent": R.agent(s), "entry": note["eid"], "what": what})
        if note["kind"] == "question" and note["answered_at"] is None:
            answered = any(a["question"] == note["eid"] for a in H.answers.values())
            seen_by = sorted(s for s, t0 in reached.items() if R.called_between(s, t0, min(until, end_t)))
            if not answered and seen_by:
                slips["11"].append(
                    {
                        "agents": sorted({R.agent(s) for s in seen_by}),
                        "entry": note["eid"],
                        "what": "a colleague's question left unanswered",
                    }
                )
    for a in H.answers.values():
        s = a["recipient"]
        if s not in R.mine:
            continue
        until = a["until"] if a["until"] is not None else end_t
        read = H.inbox_read.get(s, {}).get(a["eid"])
        if (read is None or read > until) and R.called_between(s, a["at"], min(until, end_t)):
            slips["11"].append({"agent": R.agent(s), "entry": a["eid"], "what": "answer never read"})


# -- 12 --------------------------------------------------------------------------------------------


def serving_windows(H: History, R: Run, p: int, end_t: int) -> list[tuple]:
    """(session, from, to): when an agent session served `p`'s person and was live (from the
    principal's binding, by hello or by a from_user entry, to the session's last call)."""
    out = []
    for s in R.mine:
        x = H.sessions.get(s)
        if not x:
            continue
        since = [t for q, t in x["principals"].items() if H.same_person(q, p)]
        last = R.last_call(s)
        if since and last is not None:
            out.append((s, max(min(since), x["opened"]), min(last, end_t)))
    return out


def rule_12(H: History, R: Run, end_t: int, slips: dict) -> None:
    """A question for a human that defaulted or lapsed, which, while it was pending at a stage
    addressed to that human's person, an agent session serving them was live and recorded no
    to_user of kind question to them; and a digest entry that was pending while such a session was
    live and that no to_user showed before that session ended."""
    for q in H.uquestions.values():
        if q["ended"] not in ("defaulted", "lapsed"):
            continue
        stages = q["stages"]
        found = None
        for i, st in enumerate(stages):
            start = st["start"]
            stop = stages[i + 1]["start"] if i + 1 < len(stages) else (q["ended_at"] or end_t)
            k = st["stage"] or 0
            if k >= len(q["chain"]):
                continue
            who = q["chain"][k]
            for s, a, b in serving_windows(H, R, who, end_t):
                lo, hi = max(start, a), min(stop, b)
                if lo > hi:
                    continue
                asked = any(
                    t["by"] == s
                    and t["kind"] == "Question"
                    and H.same_person(t["principal"], who)
                    and start <= t["at"] <= stop
                    for t in H.to_user.values()
                )
                if not asked:
                    found = {"agent": R.agent(s), "question": q["eid"], "what": q["ended"], "stage": k}
                    break
            if found:
                break
        if found:
            slips["12"].append(found)
    for d in H.digests.values():
        shown = d["shown_at"]
        for s, a, b in serving_windows(H, R, d["to"], end_t):
            pending_while_live = a <= (shown if shown is not None else end_t) and d["at"] <= b
            if pending_while_live and (shown is None or shown > b):
                slips["12"].append({"agent": R.agent(s), "digest": d["eid"], "what": "left unshown"})
                break


# -- 13 --------------------------------------------------------------------------------------------


def rule_13(R: Run, history: list[dict], slips: dict) -> None:
    """A worker (no principal) that ended on its own, holding nothing, while the first node of the
    last `leave_better` it was given (by a close, a release, or an empty next) was still open and
    free; while the node its create joined was free; or while `next` would have offered it an item.
    What `next` would offer is the server's own answer when the worker's last call was an empty
    `next` or a `hello` (its offers); otherwise the counter's model of §10.4 at the worker's end."""
    for aid, calls in R.calls_by_agent.items():
        spec = R.spec.get(aid, {})
        tr = R.transcripts.get(aid, {})
        if spec.get("human") or tr.get("ended") or not calls:
            continue
        ss = R.agent_sessions.get(aid, set())
        t_end = max(int(c.get("t_end", c["t"]) * 1000) for c in calls)
        Hw = History(history, until=t_end)
        if any(Hw.sessions.get(s, {}).get("principals") for s in ss):
            continue
        if any(Hw.held_by(s) for s in ss):
            continue  # it ended holding work: rules 3 and 8 count what became of it
        pending_lb: list = []
        joined = None
        known = None  # what the server last said next would offer, if the last call said it
        for c in calls:
            st = c.get("structured") if isinstance(c.get("structured"), dict) else {}
            ok = not R.is_refused(c)
            tool, act = c["tool"], args_action(c)
            known = None
            if not ok:
                continue
            if (tool, act) in (("node", "close"), ("hold", "release")):
                pending_lb = list(st.get("leave_better") or [])
            elif (tool, act) == ("hold", "next"):
                if st.get("empty"):
                    pending_lb = list(st.get("leave_better") or [])
                    known = []
                else:
                    pending_lb, joined = [], None
            elif (tool, act) == ("hold", "take"):
                pending_lb, joined = [], None
            elif (tool, act) == ("node", "create") and st.get("existing"):
                joined = st["existing"]
            elif (tool, act) == ("session", "hello"):
                # A list of offers, or `{items: [], why}` when there are none (1.4, R25); a hello cut
                # to its budget may leave them out, and then the model decides.
                offers = st.get("offers")
                if isinstance(offers, dict):
                    offers = offers.get("items")
                if isinstance(offers, list):
                    known = [o.get("path") or o.get("id") for o in offers if isinstance(o, dict)]
        if pending_lb:
            n = Hw.node_by_ref(pending_lb[0])
            x = Hw.nodes.get(n) if n is not None else None
            if x and x["open"] and len(Hw.live_holds(n)) < x["capacity"]:
                slips["13"].append({"agent": aid, "leave_better": pending_lb[:3]})
                continue
        if joined:
            n = Hw.node_by_ref(joined)
            x = Hw.nodes.get(n) if n is not None else None
            if x and x["open"] and len(Hw.live_holds(n)) < x["capacity"] and not Hw.waiting(n):
                slips["13"].append({"agent": aid, "joined": joined})
                continue
        if known is not None:
            if known:
                slips["13"].append({"agent": aid, "offered": known[:5], "source": "hello's offers"})
            continue
        s = max(ss) if ss else None
        offered = Hw.candidates(s) if s is not None else []
        if offered:
            slips["13"].append({"agent": aid, "offered": offered[:5], "source": "the counter's model of next"})


# -- other measures --------------------------------------------------------------------------------


def known_principals(scn: dict) -> set[str]:
    keys = {"user"}
    for h in (scn.get("humans") or {}).values():
        if h.get("principal"):
            keys.add(str(h["principal"]).lower())
    return keys


def principals_given(args) -> list[str]:
    """Every principal a call's arguments name, at any depth, in order: a `principal` argument
    (`hello {principal}`, `hello {from_user: {principal}}`, `talk from_user` and `to_user`) and an
    addressee `principal:<key>` in a `to` (`talk ask`, `talk say`)."""
    out: list[str] = []

    def walk(x) -> None:
        if isinstance(x, dict):
            for k, v in x.items():
                if k == "principal" and isinstance(v, str) and v:
                    out.append(v)
                elif k == "to":
                    for t in v if isinstance(v, list) else [v]:
                        if isinstance(t, str) and t.startswith("principal:"):
                            out.append(t[len("principal:") :])
                walk(v)
        elif isinstance(x, list):
            for v in x:
                walk(v)

    walk(args)
    return out


def other_measures(scn: dict, R: Run) -> dict:
    """Refusals, warnings, next calls followed, the misreadings the 1.4 texts address, and usage."""
    refusals: dict[str, int] = {}
    refusals_not_built: dict[str, int] = {}
    warnings: dict[str, int] = {}
    misreadings = {"R1_principal": [], "R1_thread": [], "R5": [], "R20": [], "R21": [], "session": []}
    suggested = followed = 0
    known = known_principals(scn)
    for aid, calls in R.calls_by_agent.items():
        spec = R.spec.get(aid, {})
        invented: set = set()
        for i, c in enumerate(calls):
            code, hint = call_error(c["text"])
            args = c.get("args") or {}
            # R1 (R7): a principal no human of the scenario has, given in any argument of any call,
            # a refused one included, and counted the first time; `user` and the humans' configured
            # keys are not invented (rule 6's call names `user`).
            given = principals_given(args)
            made_up = [key for key in given if key.lower() not in known]
            if code and R.not_built(c):
                name = f"{c['tool']} {args_action(c)}"
                refusals_not_built[name] = refusals_not_built.get(name, 0) + 1
            elif code:
                refusals[code] = refusals.get(code, 0) + 1
                low = hint.lower()
                if code == "not_found" and "session" in low and args.get("session"):
                    misreadings["session"].append({"agent": aid, "call": i + 1, "session": args["session"]})
                if "missing" in low and "requires" in low or "unknown argument" in low or "not an argument" in low:
                    misreadings["R21"].append({"agent": aid, "call": i + 1, "hint": hint[:120]})
                elif made_up and "principal" in low:
                    # The refusal of an invented principal (R3: `user:<name>`, refused for its
                    # scheme) is the invention, counted as R1 below, not a format learned (R20).
                    pass
                elif any(w in low for w in ("not a duration", "rfc 3339", "scheme", "malformed", "expected")):
                    misreadings["R20"].append({"agent": aid, "call": i + 1, "hint": hint[:120]})
            for k in ("at", "on", "target", "path"):
                if looks_like_key(args.get(k)):
                    misreadings["R5"].append({"agent": aid, "call": i + 1, "arg": k, "value": str(args.get(k))[:60]})
            if spec.get("conversation") and not spec.get("human"):
                if (c["tool"] == "talk" and args.get("action") == "from_user") or (
                    c["tool"] == "session" and args.get("from_user")
                ):
                    misreadings["R1_thread"].append(
                        {"agent": aid, "call": i + 1, "what": "a thread's message logged as from_user"}
                    )
            for key in made_up:
                if key.lower() not in invented:
                    invented.add(key.lower())
                    d = {"agent": aid, "call": i + 1, "what": f"invented principal {key}"}
                    if code:
                        d["refused"] = code
                    misreadings["R1_principal"].append(d)
            for m in WARNING_CODE.finditer(c["text"] or ""):
                w = m.group(1) or m.group(2)
                if w and not code:
                    warnings[w] = warnings.get(w, 0) + 1
            item = next_now(c.get("structured"))
            if item and i + 1 < len(calls):
                suggested += 1
                n = calls[i + 1]
                if n["tool"] == item.get("tool") and (n.get("args") or {}).get("action") == (
                    item.get("args") or {}
                ).get("action"):
                    followed += 1
    return {
        "refusals": refusals,
        "refusals_total": sum(refusals.values()),
        "refusals_not_built": refusals_not_built,
        "warnings": warnings,
        "unsettled_requests_warnings": warnings.get("unsettled_requests", 0),
        "open_commitments_warnings": warnings.get("open_commitments", 0),
        "next_calls": {"suggested_now": suggested, "followed": followed},
        "calls": sum(len(v) for v in R.calls_by_agent.values()),
        "calls_by_agent": {a: len(v) for a, v in R.calls_by_agent.items()},
        "usage": usage(R.transcripts),
        "misreadings": {k: len(v) for k, v in misreadings.items()},
        "misreading_details": misreadings,
    }


def usage(transcripts: dict) -> dict:
    """Tokens are per turn, summed; a client's cost is the session's so far, so its last value counts."""
    out = {
        "input_all_tokens": 0,
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_tokens": 0,
        "cache_creation_tokens": 0,
        "cost_usd": 0.0,
    }
    for tr in transcripts.values():
        session_cost = 0.0
        for t in tr.get("turns", []):
            u = dict(t.get("usage") or {})
            if u and "input_all_tokens" not in u:  # a transcript written before the field existed
                u["input_all_tokens"] = u.get("input_tokens", 0) + (
                    u.get("cache_read_tokens", 0) + u.get("cache_creation_tokens", 0)
                    if tr.get("vendor") == "claude"
                    else 0
                )
            for k in out:
                v = u.get(k)
                if k != "cost_usd" and isinstance(v, (int, float)):
                    out[k] += v
            c = u.get("session_cost_usd", u.get("cost_usd"))
            if isinstance(c, (int, float)):
                session_cost = max(session_cost, float(c))
        out["cost_usd"] += session_cost
    return out


__all__ = ["RULES", "NOT_MEASURED", "count", "variant"]
