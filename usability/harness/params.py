"""The server parameters of a run: §22's defaults, shortened, and checked against §22's rules.

A scenario names a profile (`scenarios/params-<name>.json`) and may override single parameters.
The shortened profile keeps every rule of §22 (`Params::valid`), so the server starts, and puts
every deadline a scenario can meet within an hour. The server checks the rules again at startup.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

SECOND = 1_000
MINUTE = 60 * SECOND
HOUR = 60 * MINUTE
DAY = 24 * HOUR

_UNITS = {"ms": 1, "s": SECOND, "m": MINUTE, "h": HOUR, "d": DAY}

# §22's defaults, in the units of the TOML configuration (D.11.9).
DEFAULTS = {
    "lease_first": "15m",
    "lease_default": "20m",
    "lease_max": "2h",
    "lease_ceiling": "8h",
    "renew_max": 3,
    "hold_per_session": 2,
    "presence_silence": "15m",
    "presence_resolution": "3m",
    "session_retain": "24h",
    "op_retain": "24h",
    "conversation_schemes": ["chat", "issue", "mail"],
    "commitment_followup": "90m",
    "question_recheck": "4h",
    "question_max_rechecks": 2,
    "user_question_due": "72h",
    "user_question_due_absent": "1h",
    "user_question_max_pending": 20,
    "user_gap_window": "28d",
    "deliver_window": "24h",
    "talk_wait_max": "50s",
    "hello_budget": 4_096,
    "brief_budget": 16_384,
    "digest_max": 2_048,
    "leave_better_max": 3,
    "refusal_ttl": "7d",
    "refusal_window": "7d",
    "refusal_backoff": "1h",
    "refusal_quorum": 3,
    "too_big_attempts": 3,
    "pause_ttl": "2h",
    "archive_after": "7d",
    "coverage_window": "28d",
    "capacity_window": "7d",
    "write_per_hour": 120,
    "queue_max_wait": "10s",
    "text_max": 65_536,
    "tree_max_depth": 16,
    "tree_max_children": 500,
    "settle_batch": 1_000,
    "instructions_max": 512,
}

DURATIONS = [k for k, v in DEFAULTS.items() if isinstance(v, str)]
COUNTS = [
    "hold_per_session",
    "question_max_rechecks",
    "too_big_attempts",
    "leave_better_max",
    "tree_max_children",
    "settle_batch",
    "write_per_hour",
    "digest_max",
    "instructions_max",
    "user_question_max_pending",
    "refusal_quorum",
]
# Durations that are windows over the history, not deadlines a run waits for.
WINDOWS = {"user_gap_window", "coverage_window", "capacity_window", "refusal_window"}
# Durations that bound one call, not a deadline.
CALL_BOUNDS = {"talk_wait_max", "queue_max_wait"}


def ms(value) -> int:
    """A duration in milliseconds: an integer, or a text such as `15m`, `2h`, `50s`, or `7d`."""
    if isinstance(value, int):
        return value
    m = re.fullmatch(r"\s*(\d+)\s*(ms|s|m|h|d)\s*", str(value))
    if not m:
        raise ValueError(f"not a duration: {value!r}")
    return int(m.group(1)) * _UNITS[m.group(2)]


def problems(p: dict) -> list[str]:
    """The rules of §22 that `p` breaks (empty when `Params::valid` would hold)."""
    out = []
    d = {k: ms(p[k]) for k in DURATIONS}
    out += [f"{k} is under 1 s" for k, v in d.items() if v < SECOND]
    if not d["lease_first"] <= d["lease_default"] <= d["lease_max"] <= d["lease_ceiling"]:
        out.append("lease.first <= lease.default <= lease.max <= lease.ceiling")
    out += [f"{k} is under 1" for k in COUNTS if int(p[k]) < 1]
    if d["user_question_due_absent"] > d["user_question_due"]:
        out.append("user_question.due_absent <= user_question.due")
    if d["refusal_backoff"] > d["refusal_ttl"]:
        out.append("refusal.backoff <= refusal.ttl")
    if 3 * d["presence_resolution"] > d["presence_silence"]:
        out.append("presence.resolution is at most a third of presence.silence")
    if not (d["session_retain"] > d["lease_ceiling"] and d["session_retain"] > d["presence_silence"]):
        out.append("session.retain exceeds lease.ceiling and presence.silence")
    if set(p["conversation_schemes"]) & {"user", "person", "repeat"}:
        out.append("conversation.schemes never contains user, person, or repeat")
    if int(p["tree_max_depth"]) < 3:
        out.append("tree.max_depth is at least 3")
    if int(p["text_max"]) < int(p["digest_max"]):
        out.append("text.max is at least digest.max")
    if int(p["hello_budget"]) < 512 or int(p["brief_budget"]) < 1_024:
        out.append("hello.budget >= 512 and brief.budget >= 1 KiB")
    return out


# Orders among §22's defaults beyond the rules of `problems`: (left, relation, right, kept, why). A
# kept order holds in the short profile, because it shapes what agents meet; an inverted one does
# not, for the reason given (`params-short.json` says the same).
ORDERS = [
    ("presence_silence", "==", "lease_first", True, "a session that vanished leaves present when its first lease ends"),
    ("refusal_ttl", ">", "session_retain", True, "refusal memory outlives a session"),
    ("refusal_window", ">", "session_retain", True, "the refusal window outlives a session"),
    ("archive_after", ">", "session_retain", True, "a closed node is archived after its sessions retire"),
    (
        "question_recheck",
        ">",
        "lease_max",
        False,
        "a recheck past 15 min puts the second recheck beyond the hour; with the defaults too a recheck can "
        "fire during a hold renewed toward its ceiling",
    ),
    ("user_question_due", ">", "lease_ceiling", False, "a ceiling under 15 min would cut long live work"),
    ("deliver_window", "<", "user_question_due", False, "notes would be delivered for under 15 min"),
]
_REL = {"==": lambda a, b: a == b, ">": lambda a, b: a > b, "<": lambda a, b: a < b}


def orders(p: dict) -> list[tuple[str, bool, bool, bool]]:
    """Each order of ORDERS as `(text, holds with the defaults, holds in p, kept)`."""
    out = []
    for left, rel, right, kept, _why in ORDERS:
        text = f"{left} {rel} {right}"
        out.append(
            (text, _REL[rel](ms(DEFAULTS[left]), ms(DEFAULTS[right])), _REL[rel](ms(p[left]), ms(p[right])), kept)
        )
    return out


def late_deadlines(p: dict, horizon: int = HOUR) -> list[str]:
    """The deadline durations of `p` longer than `horizon` (windows and call bounds excepted)."""
    return [k for k in DURATIONS if k not in WINDOWS and k not in CALL_BOUNDS and ms(p[k]) > horizon]


def load(scenarios_dir: Path, spec: dict | None) -> dict:
    """The parameters of a scenario: the named profile over §22's defaults, then the overrides."""
    spec = spec or {}
    p = dict(DEFAULTS)
    profile = spec.get("profile")
    if profile:
        data = json.loads((scenarios_dir / f"params-{profile}.json").read_text())
        p.update(data["params"])
    p.update(spec.get("overrides", {}))
    unknown = set(p) - set(DEFAULTS)
    if unknown:
        raise ValueError(f"unknown parameters: {sorted(unknown)}")
    return p


def toml(p: dict) -> str:
    """The `[params]` table of the server's configuration (D.11.9)."""
    lines = ["[params]"]
    for k, v in p.items():
        if v == DEFAULTS[k]:
            continue
        lines.append(f"{k} = {json.dumps(v)}")
    return "\n".join(lines) + "\n"
