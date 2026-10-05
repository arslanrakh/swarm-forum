"""Scenarios as data: loading and checking `scenarios/<id>.json`.

A scenario file holds:

- `id`, `title`, `covers` (the §30 flows and §32 steps it exercises), and `needs_packages`: the
  core packages (and any server switch) its prelude and its agents' flows need. It is ready when
  the server has them all (`packages.BUILT`, `missing`); one that is not is skipped unless the run
  forces it;
- `campaign`: false for a scenario that is not one of the WP-U row's (the smoke run): `campaign`
  does not run it, and the report's pass criteria do not ask for it;
- `extra_arms`: `{name: why}`, arms the WP-U row asks of this scenario beyond the two arms, which
  the harness cannot run yet (S8 without `orientation`); the report lists them as missing cells;
- `params`: `{profile, overrides}`, the server parameters (see `params.py`);
- `tokens`: `{name: {class, caps?}}`, the classes of the scenario's sessions; every agent gets a
  token of its own, of the class its `token` names, so that its calls can be told apart;
- `prelude`: scripted calls the harness makes before any agent arrives, each
  `{as, tool, args, save?, may_fail?}`, or `{sleep: <duration>, why}` to let deadlines fall due;
  `as` names a token (one forum session per name, or per `as` + `session_name` when given);
  `save` maps a variable to a dotted path in the structured content, and `$name` in later `args`
  is replaced by its value, alone or inside a longer string (`user:$e140`); a refused step stops
  the run as `prelude_failed`;
- `humans`: `{name: {who, principal, replies: [rule]}}`, the humans the agents serve; `principal`
  is the key their client names, or null when it names none;
- `people`: `{name: {who, where, replies: [rule]}}`, the people outside the swarm;
- `world`: `[rule]`, what outside systems answer an agent's outside actions; an action that no
  rule answers gets `world_default` (`runner.WORLD_DEFAULT` unless the scenario sets its own, or
  null for none), since every role promises an action's result in the next message;
- `agents`: `[{id, token, role, human?, conversation?, start, lines, end?, max_turns?}]`:
  - `role` is the agent's client configuration (Claude: appended system prompt; Codex: developer
    instructions): who it is and what it can do outside the forum, never how to use the forum.
    Agents have only the forum's tools, so a role tells the agent to write an outside action on
    its own line as `ACTION: ...`, and a post in the conversation it serves as `POST: ...`; the
    rest of its reply is what its own human reads;
  - `start`: `{at: <duration>}` after the prelude, `{after: <agent id>, delay}`, or
    `{on: <event>, delay?}`, an event that a reply rule raises;
  - `lines`: the messages the agent receives in order, `{from: human|event|person|world, text,
    after_call?}`. Lines are delivered between turns, so a human never interrupts a turn; a line
    with `after_call` keeps the Wizard-of-Oz round's trigger, and its delivery records whether the
    call had been made by then;
  - `end`: the fixed point at which the session is ended mid-task (see `agents.EndPoint`), the
    same in both arms;
- `drain`: how long the server runs on after the last agent, so that the deadlines the run left
  behind (lease ends above all) fire before the history is read;
- `seed_limits`: what a live run cannot recreate of the Wizard-of-Oz seed;
- `intended_flow` and `criteria`: the reference flow and the success criteria, for the reader;
  the slip counter does not depend on them.

A reply rule is `{on?: reply|post|action, when: asked|told|always, match?: [words], say, once?,
then?}`. It reads one part of the agent's reply: by default its own human's rules read the reply,
a person's rules read the `POST:` lines, and the world's rules (`agent` names the agents they
apply to, `*` all) read the `ACTION:` lines. `asked` fires when that part has a question mark
and one of the words (or any question, without `match`), `told` when it has one of the words,
`always` whenever it is not empty. Per source, the first rule that fires gives the reply, once
unless `once` is false. `then: {event, delay?}` raises an event instead of answering the same
agent, so that a person's answer starts a new session, as in §32 step 5.
"""

from __future__ import annotations

import json
from pathlib import Path

from . import packages as PK
from . import params as P

LINE_SOURCES = {"human", "event", "person", "world"}


class ScenarioError(Exception):
    pass


def scenarios_dir() -> Path:
    return Path(__file__).resolve().parents[1] / "scenarios"


def list_ids() -> list[str]:
    return sorted(p.stem for p in scenarios_dir().glob("s*.json"))


def campaign_ids() -> list[str]:
    """The scenarios of the WP-U row: every scenario but those marked `campaign: false`."""
    return [
        sid for sid in list_ids() if json.loads((scenarios_dir() / f"{sid}.json").read_text()).get("campaign", True)
    ]


def missing(s: dict) -> list[str]:
    """What the scenario needs that the server does not have yet; empty when it is ready."""
    return PK.missing(s.get("needs_packages"))


def load(sid: str) -> dict:
    path = scenarios_dir() / f"{sid}.json"
    if not path.exists():
        matches = [p for p in scenarios_dir().glob(f"{sid}*.json")]
        if len(matches) != 1:
            raise ScenarioError(f"no scenario {sid!r}: {list_ids()}")
        path = matches[0]
    s = json.loads(path.read_text())
    check(s)
    return s


def check(s: dict) -> None:
    for k in ("id", "title", "tokens", "agents"):
        if k not in s:
            raise ScenarioError(f"{s.get('id', '?')}: missing {k}")
    p = P.load(scenarios_dir(), s.get("params"))
    bad = P.problems(p)
    if bad:
        raise ScenarioError(f"{s['id']}: parameters break §22: {bad}")
    late = P.late_deadlines(p)
    if late:
        raise ScenarioError(f"{s['id']}: deadlines beyond an hour: {late}")
    ids = set()
    for a in s["agents"]:
        if a["id"] in ids:
            raise ScenarioError(f"{s['id']}: agent {a['id']} twice")
        ids.add(a["id"])
        if a["token"] not in s["tokens"]:
            raise ScenarioError(f"{s['id']}: agent {a['id']}: unknown token {a['token']}")
        for line in a.get("lines", []):
            if line.get("from", "human") not in LINE_SOURCES:
                raise ScenarioError(f"{s['id']}: agent {a['id']}: line source {line.get('from')}")
        if a.get("human") and a["human"] not in s.get("humans", {}):
            raise ScenarioError(f"{s['id']}: agent {a['id']}: unknown human {a['human']}")
        if a.get("conversation") and a["conversation"] not in s.get("people", {}):
            raise ScenarioError(f"{s['id']}: agent {a['id']}: unknown person {a['conversation']}")
    for a in s["agents"]:
        st = a.get("start", {"at": "0s"})
        if "after" in st and st["after"] not in ids:
            raise ScenarioError(f"{s['id']}: agent {a['id']} starts after unknown {st['after']}")
    for step in s.get("prelude", []):
        if "sleep" in step:
            P.ms(step["sleep"])
            continue
        if step.get("as") not in s["tokens"]:
            raise ScenarioError(f"{s['id']}: prelude step as unknown token {step.get('as')}")
