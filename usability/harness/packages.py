"""Which package of design D.13.2 builds each command, and which packages the server has.

Until a package lands, the core answers its commands with `invalid(BadParams)`, whose hint ("the
parameters break a rule of the constitution, or this server does not do this yet") real BadParams
refusals share. A refusal is the server's gap, not the agent's slip, only when its command belongs to
a package the server does not have yet: `not_built` checks both. Update `BUILT` when a package
lands; every run records the set it was counted with (`built_packages` in `run.json`), so that a
recount of an older run keeps its own. `BUILT` also decides which scenarios are ready: those whose
`needs_packages` it holds in full (`missing`).
"""

from __future__ import annotations

# The core packages merged into the binary the harness runs (D.13.2 waves): WP-D1 to WP-D7, with
# WP-D5 in its two parts (WP-D5a, the user log, and WP-D5b, people and agents), and WP-D1.4, the
# core's catch-up to 1.4 and to the core part of 1.5. WP-D8 is planned after the first release.
BUILT = frozenset({"WP-D1", "WP-D2", "WP-D3", "WP-D4", "WP-D1.4", "WP-D5a", "WP-D5b", "WP-D6", "WP-D7"})

# What the server had before WP-D5a, WP-D5b, WP-D6, and WP-D7 landed (the runs of round R3). Every
# command `COMMANDS` names is built now, so the tests of the not-built count use this set.
BUILT_BEFORE_D5 = frozenset({"WP-D1", "WP-D2", "WP-D3", "WP-D4", "WP-D1.4"})

BAD_PARAMS_HINT = "does not do this yet"


def _has(arg: str):
    return lambda args: args.get(arg) is not None


def _to_human(args: dict) -> bool:
    to = str(args.get("to") or "")
    return to == "user" or to.startswith("principal:")


def _always(args: dict) -> bool:
    return True


# (tool, action, condition on the arguments, package), first match wins. The conditions are the
# stubs' own: a create that would join (a key, D.13.1), a `repeat` (WP-D6), hello's `from_user`.
COMMANDS = [
    ("node", "key", _always, "WP-D4"),
    ("node", "link", _always, "WP-D4"),
    ("node", "reopen", _always, "WP-D4"),
    ("node", "move", _always, "WP-D4"),
    ("node", "merge", _always, "WP-D4"),
    ("node", "create", _has("repeat"), "WP-D6"),
    ("node", "update", _has("repeat"), "WP-D6"),
    ("node", "create", lambda a: bool(a.get("keys")), "WP-D4"),
    ("session", "hello", _has("from_user"), "WP-D5a"),
    ("session", "status", _always, "WP-D5b"),
    ("talk", "from_user", _always, "WP-D5a"),
    ("talk", "to_user", _always, "WP-D5a"),
    ("talk", "route", _always, "WP-D5a"),
    ("talk", "ask", _to_human, "WP-D5a"),
    ("talk", "ask", _always, "WP-D5b"),
    *[
        ("talk", a, _always, "WP-D5b")
        for a in (
            "request",
            "said",
            "heard",
            "answer",
            "say",
            "wait",
            "who",
            "decide",
            "approve",
            "object",
            "resolve",
            "retract",
        )
    ],
]


def missing(needs, built=BUILT) -> list[str]:
    """What a scenario needs that the server does not have yet: the entries of its `needs_packages`
    that `built` lacks. A need that is not a package (a server switch) is never built."""
    return [n for n in needs or [] if n not in set(built)]


def package_of(tool: str, action: str, args: dict | None) -> str | None:
    args = args or {}
    for t, a, cond, pkg in COMMANDS:
        if t == tool and a == action and cond(args):
            return pkg
    return None


def not_built(tool: str, action: str, args: dict | None, code: str | None, hint: str, built=BUILT) -> bool:
    """A refusal by a command of a package the server does not have yet: `invalid` with the
    placeholder's hint, for a command that such a package builds."""
    if code != "invalid" or BAD_PARAMS_HINT not in (hint or ""):
        return False
    pkg = package_of(tool, action, args)
    return pkg is not None and pkg not in set(built)
