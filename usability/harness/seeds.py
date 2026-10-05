"""The usability lab's seeds: forums at several stages of their life, each with three agents.

A seed is a scenario in `scenario.py`'s format, built in code, so that a forum with hundreds of nodes
and weeks of history takes a few loops rather than a long file. The prelude plays the forum's past
through the server's own tools, by scripted sessions of the swarm's usual classes, and moves the
forum's clock forward between episodes (`{"advance": "2d"}`; the server's lagged clock, feature
`usability`), so that ages, expired holds, retired sessions, and lapsed questions are the server's
own. The agents then arrive at the real time, each with a task that this stage of the forum's life
makes typical. The lab (`lab.py`) gives each seed's three agents three different models.

Stages:

- `l1-day-one`: a forum installed today, empty; a team's first agents arrive.
- `l2-solo-month`: one developer, a month of use: closed work, a pending question, an expired hold.
- `l3-company-quarter`: a company of ten people in three teams, three months: threads, requests,
  work in flight, offers.
- `l4-mature-year`: a year of a company's forum: hundreds of nodes, mostly closed, a fix that
  regressed, duplicates and stale work.
- `l5-research`: no human: a research programme's notes, lemmas, refutations, waits on jobs.
- `l6-incident`: a company forum during an incident: a pause, an expired hold with an open
  intent, duplicate incident nodes, conflicting decisions.

Roles say who an agent is and what it can do outside the forum, never how to use the forum.
"""

from __future__ import annotations

import random
import time

ACTIONS = (
    "Your only tools are those of the swarm forum. When you need something done outside the forum (a shell "
    "command, git, CI, a job, a pull request, reading a log, a file, or a dashboard), write it on its own line "
    "of your reply as `ACTION: <what to do>`; the platform does it and gives you the result in its next message."
)
POSTS = (
    "To post in the thread, write the post on its own line of your reply as `POST: <text>`; the chat "
    "integration posts it there as your message."
)
STOPS = "The platform may stop your run at any moment."
SCHEDULED = "The scheduler started you. Pick up work on the forum."


def ide_role(who: str, principal: str, what: str) -> str:
    return (
        f"You are an IDE coding agent working for {who}. Your IDE client's configuration identifies them as "
        f"{principal}. {what} {ACTIONS} Everything else in your reply is shown to them in the IDE chat."
    )


def chat_role(team: str, channel: str, what: str) -> str:
    return (
        f"You are an AI agent run started by the chat integration of {team}, for a new message in the {channel} "
        f"channel. Nobody is chatting with you directly. {what} {POSTS} {ACTIONS} Nobody reads the rest of your "
        f"reply. {STOPS}"
    )


def worker_role(team: str, what: str) -> str:
    return (
        f"You are a worker agent of {team}'s agent swarm, started by the agent platform's scheduler. You work for "
        f"no particular person and nobody is chatting with you. {what} {ACTIONS} Nobody reads the rest of your "
        f"reply. {STOPS}"
    )


def thread_event(channel: str, thread: str, msg: str, who: str, text: str) -> str:
    return (
        f"New message in {channel}, thread {thread} (conversation key chat:{channel.lstrip('#')}/{thread}), message "
        f"{msg} (message id chat:{channel.lstrip('#')}/{thread}/{msg}), from {who}: \"{text}\" Handle it."
    )


class Seed:
    """A scenario under construction: tokens, the prelude's sessions and calls, and the agents."""

    def __init__(self, sid: str, title: str, stage: str, about: str):
        self.s: dict = {
            "id": sid,
            "title": title,
            "stage": stage,
            "about": about,
            "campaign": False,
            "lab": True,
            "needs_packages": [],
            "tokens": {},
            "prelude": [],
            "humans": {},
            "people": {},
            "world": [],
            "agents": [],
            "drain": "1m",
            "max_minutes": 40,
            "world_default": "Done; the command printed nothing.",
        }
        self.n = 0
        self.sessions: dict[str, tuple[str, str]] = {}

    # -- the prelude's mechanics ------------------------------------------------------------------

    def token(self, name: str, cls: str, caps: list[str] | None = None) -> str:
        self.s["tokens"][name] = {"class": cls, **({"caps": caps} if caps else {})}
        return name

    def var(self, prefix: str = "v") -> str:
        self.n += 1
        return f"{prefix}{self.n}"

    def advance(self, d: str) -> None:
        self.s["prelude"].append({"advance": d})

    def step(self, sess: str, tool: str, args: dict, save: dict | None = None, may_fail: bool = False) -> None:
        as_, name = self.sessions[sess]
        st: dict = {"as": as_, "session_name": name, "tool": tool, "args": args}
        if save:
            st["save"] = save
        if may_fail:
            st["may_fail"] = True
        self.s["prelude"].append(st)

    def session(
        self,
        as_: str,
        intent: str | None = None,
        principal: str | None = None,
        from_user: str | None = None,
        client: str = "ide",
    ) -> tuple[str, str | None]:
        """A new forum session of token `as_`: its hello, and the user entry's variable when a
        human's words come with it."""
        self.n += 1
        sess = f"{as_}-{self.n}"
        self.sessions[sess] = (as_, sess)
        args: dict = {"action": "hello"}
        if intent:
            args["intent"] = intent
        if principal:
            args["principal"] = principal
        ev = None
        save = None
        if from_user:
            args["from_user"] = {"text": from_user, "client": client}
            ev = self.var("e")
            save = {ev: "user_entry.entry"}
        self.step(sess, "session", args, save)
        return sess, ev

    # -- the forum's content ----------------------------------------------------------------------

    def create(self, sess: str, path: str, title: str, body: str | None = None, keys=None, take=False, **kw) -> dict:
        """`node create`; returns the variables it saved: `t` the hold's token (when taken), `r` the
        request (with `request`), `u` the user's request (with a `user:$e` key)."""
        args: dict = {"action": "create", "path": path, "title": title}
        if body:
            args["body"] = body
        if keys:
            args["keys"] = list(keys)
        if not take:
            args["take"] = False
        args.update(kw)
        out: dict = {"t": None, "r": None, "u": None}
        save: dict = {}
        if take:
            out["t"] = self.var("t")
            save[out["t"]] = "token"
        if kw.get("request"):
            out["r"] = self.var("r")
            save[out["r"]] = "request"
        if keys and any(str(k).startswith("user:$") for k in keys):
            out["u"] = self.var("u")
            save[out["u"]] = "user_requests.0"
        self.step(sess, "node", args, save or None)
        return out

    def record(self, sess: str, path: str, title: str, body: str, **kw) -> None:
        """A record nobody holds: a note, a kb entry (capacity 0)."""
        self.create(sess, path, title, body, capacity=0, **kw)

    def person(self, sess: str, slug: str, name: str, keys: list[str], body: str, tags=None) -> str:
        path = f"@people/{slug}"
        kw = {"tags": tags} if tags else {}
        self.create(sess, path, name, body, keys=[f"person:{k}" for k in keys], kind="person", capacity=0, **kw)
        return "/" + path

    def team(self, sess: str, slug: str, title: str, owners: list[str], body: str) -> str:
        path = f"@teams/{slug}"
        self.create(sess, path, title, body, kind="team", capacity=0, owners=owners)
        return "/" + path

    def take(self, sess: str, target: str) -> str:
        tv = self.var("t")
        self.step(sess, "hold", {"action": "take", "target": target}, {tv: "token"})
        return tv

    def checkpoint(self, sess: str, tv: str, done: str, next_step: str, **kw) -> None:
        self.step(sess, "hold", {"action": "checkpoint", "token": f"${tv}", "done": done, "next_step": next_step, **kw})

    def decide(self, sess: str, at: str, text: str, evidence: str = "attempt", keep: bool = False, **kw):
        dv = self.var("d") if keep else None
        self.step(
            sess,
            "talk",
            {"action": "decide", "at": at, "text": text, "evidence": evidence, **kw},
            {dv: "decision"} if dv else None,
        )
        return dv

    def close(self, sess: str, path: str, tv: str | None, outcome: str, summary: str, **kw) -> None:
        args = {"action": "close", "path": path, "outcome": outcome, "summary": summary, **kw}
        if tv:
            args["token"] = f"${tv}"
        self.step(sess, "node", args)

    def release(self, sess: str, tv: str, done: str, next_step: str, reason: str = "handoff", **kw) -> None:
        self.step(
            sess,
            "hold",
            {"action": "release", "token": f"${tv}", "done": done, "next_step": next_step, "reason": reason, **kw},
        )

    def task(
        self,
        sess: str,
        path: str,
        title: str,
        body: str,
        summary: str | None = None,
        steps=(),
        keys=None,
        outcome: str = "done",
        decision: str | None = None,
        learnings: str | None = None,
        release: dict | None = None,
        **kw,
    ) -> dict:
        """A piece of work in one session: create (held), checkpoints, a decision, then a close (with
        `summary`) or a release (`release`: its arguments), or neither (the hold left to expire)."""
        v = self.create(sess, path, title, body, keys=keys, take=True, **kw)
        for done, nxt in steps:
            self.checkpoint(sess, v["t"], done, nxt)
        if decision:
            self.decide(sess, path, decision)
        if summary is not None:
            self.close(sess, path, v["t"], outcome, summary, **({"learnings": learnings} if learnings else {}))
        elif release is not None:
            self.release(sess, v["t"], **release)
        return v

    def user_task(
        self,
        sess: str,
        e: str,
        principal: str,
        path: str,
        title: str,
        body: str,
        report: str,
        summary: str,
        decision: str,
        steps=(),
        keys=(),
        learnings: str | None = None,
        **kw,
    ) -> None:
        """A human's request (user entry `e`) done in one session: the node joined by the entry's key,
        checkpoints, the decision that settles the request, the report to the human, the close."""
        v = self.create(sess, path, title, body, keys=[f"user:${e}", *keys], take=True, **kw)
        for done, nxt in steps:
            self.checkpoint(sess, v["t"], done, nxt)
        self.decide(sess, path, decision, settles=[f"${v['u']}"])
        self.step(
            sess,
            "talk",
            {"action": "to_user", "principal": principal, "kind": "result", "verbatim": True, "text": report,
             "re": [f"${e}"]},
        )
        self.close(sess, path, v["t"], "done", summary, **({"learnings": learnings} if learnings else {}))

    def said(self, sess: str, at: str, thread: str, kind: str, text: str, confirm: bool = True) -> str:
        sv = self.var("s")
        self.step(sess, "talk", {"action": "said", "at": at, "where": f"chat:{thread}", "kind": kind, "text": text},
                  {sv: "said_id"})
        if confirm:
            self.step(sess, "talk", {"action": "said", "said_id": f"${sv}", "confirmed": "posted in the thread"})
        return sv

    def thread_request(
        self,
        sess: str,
        path: str,
        title: str,
        thread: str,
        msg: str,
        requester: str,
        text: str,
        body: str | None = None,
        ack: str | None = None,
        take: bool = True,
        **kw,
    ) -> dict:
        """A person's message in a shared thread, recorded as a request on a node (held by `sess`
        when `take`), and an acknowledgement recorded and confirmed."""
        v = self.create(
            sess,
            path,
            title,
            body,
            keys=[f"chat:{thread}"],
            take=take,
            request={"text": text, "origin": f"chat:{thread}/{msg}", "requester": requester},
            **kw,
        )
        if ack:
            self.said(sess, path, thread, "ack", ack)
        return v

    def answer_thread(
        self, sess: str, path: str, v: dict, thread: str, reply: str, decision: str, summary: str, **kw
    ) -> None:
        """Answers a thread request: the decision that settles it, the reply recorded and confirmed,
        then the close."""
        self.decide(sess, path, decision, settles=[f"${v['r']}"])
        self.said(sess, path, thread, "result", reply)
        self.close(sess, path, v["t"], "done", summary, **kw)

    # -- the agents ---------------------------------------------------------------------------------

    def human(self, name: str, who: str, principal: str, replies: list[dict]) -> None:
        self.s["humans"][name] = {"who": who, "principal": principal, "replies": replies}

    def people(self, name: str, who: str, where: str, replies: list[dict]) -> None:
        self.s["people"][name] = {"who": who, "where": where, "replies": replies}

    def world(self, agent: str, match: list[str], say: str, once: bool = True) -> None:
        self.s["world"].append({"agent": agent, "on": "action", "when": "told", "match": match, "say": say, "once": once})

    def agent(self, aid: str, token: str, role: str, lines: list[dict], **kw) -> None:
        self.s["agents"].append({"id": aid, "token": token, "role": role, "lines": lines, **kw})

    def build(self) -> dict:
        return self.s


# -- L1: day one ---------------------------------------------------------------------------------


def l1_day_one() -> dict:
    s = Seed(
        "l1-day-one",
        "Day one: a team's forum installed today, empty; its first agents arrive",
        "empty",
        "Nothing in the forum yet: no people, no teams, no agreements. Lena's IDE agent gets a coding task, the "
        "chat integration a request in the team's thread, and a scheduled worker arrives a few minutes later.",
    )
    s.token("ide", "ide-agent", ["python", "repo"])
    s.token("chat", "chat-worker", ["chat", "repo", "ci"])
    s.token("worker", "coding-agent", ["python", "repo", "ci", "docker"])
    s.human(
        "lena",
        "Lena Brandt",
        "mail:lena@shopco.example",
        [
            {"when": "told", "match": ["pr", "pull request", "#12"], "say": "Nice, thanks! I'll review it after lunch."},
            {"when": "asked", "say": "Your call, just keep the test meaningful."},
        ],
    )
    s.people(
        "omar",
        "Omar Haddad (backend lead)",
        "thread T2001 of #eng (chat:eng/T2001)",
        [
            {"on": "post", "when": "asked", "say": "Not sure, it started Monday. Before Friday's release would be great."},
            {"on": "post", "when": "told", "match": ["pr", "fix", "cache", "minutes"], "say": "Great, thanks!"},
        ],
    )
    s.world(
        "A",
        ["test_checkout_timeout", "pytest", "run the test", "run tests", "rerun", "repro"],
        "Ran tests/test_checkout.py::test_checkout_timeout 50 times: 6 failures, all `TimeoutError: payment gateway "
        "did not answer within 2.0 s`. The test's FakeGateway sleeps random.uniform(0, 2.5) seconds before answering.",
    )
    s.world(
        "A",
        ["edit", "change", "fix", "patch", "mock", "fake"],
        "FakeGateway now sleeps a fixed 0.1 s, and a new test test_checkout_gateway_slow sets 2.5 s and expects the "
        "timeout. 200 runs: 0 failures. Full suite: 184 passed.",
    )
    s.world("A", ["push", "pull request", "open a pr", "pr "], "Branch fix/flaky-checkout-timeout pushed; PR #12 opened "
            "(https://git.shopco.example/shop-api/pull/12); CI green after 6 minutes.")
    s.world(
        "B",
        ["deploy", "staging", "pipeline", "log", "build", "timing"],
        "Staging deploy pipeline, last 10 runs: step `docker build` takes 16-18 min (it took 3-4 min before Monday). "
        "Monday's commit a1b2c3d (\"ci: reproducible builds\") added `--no-cache` to scripts/build.sh.",
    )
    s.world(
        "B",
        ["remove", "revert", "cache", "fix", "edit"],
        "With `--no-cache` removed and `--pull` kept, the staging deploy took 4 min 50 s; the image digest is "
        "reproducible across two runs.",
    )
    s.world("B", ["push", "pull request", "pr "], "PR #13 opened (https://git.shopco.example/shop-api/pull/13); CI green.")
    s.world("C", ["test", "ci", "build", "run"], "The command ran; all 184 tests pass on main.")
    s.agent(
        "A",
        "ide",
        ide_role(
            "Lena Brandt, a backend engineer at ShopCo, a small online shop",
            "mail:lena@shopco.example",
            "You can talk to her in the IDE chat, edit and test the shop-api code, push branches, open pull requests, "
            "and read CI.",
        ),
        [
            {
                "from": "human",
                "text": "Hi! We set up the swarm forum for our team this morning, so you're one of the first agents on "
                "it. Can you fix the flaky test test_checkout_timeout in shop-api? It fails maybe 1 in 10 CI runs. "
                "Tell me when there's a PR.",
            }
        ],
        human="lena",
        max_turns=10,
    )
    s.agent(
        "B",
        "chat",
        chat_role(
            "ShopCo's engineering team",
            "#eng",
            "You can read and post in the thread, read the shop-api repository, CI and deploy logs, run builds, push "
            "branches, and open pull requests.",
        ),
        [{"from": "event", "text": thread_event("#eng", "T2001", "M1", "Omar Haddad (backend lead)",
                                                "@swarm staging deploys take ~20 minutes since Monday, they used to "
                                                "take 5. Can someone look?")}],
        conversation="omar",
        start={"at": "30s"},
        max_turns=10,
    )
    s.agent(
        "C",
        "worker",
        worker_role("ShopCo", "You can run Python, tests, Docker builds, and CI, and push to the shop-api repository."),
        [{"from": "event", "text": SCHEDULED}],
        start={"at": "4m"},
        max_turns=10,
    )
    s.s["intended"] = [
        "A (Lena's IDE agent): hello with principal and her words as from_user; create a node for the flaky test "
        "(a sensible path, an issue key if one is known) joined to her entry; checkpoint; record the intent before "
        "pushing; report the PR with to_user (kind result, re her entry) and tell her; decide; close with a summary.",
        "B (chat integration): record Omar's thread message as a request on a node keyed by the thread (not "
        "from_user); record each post with said before posting and confirm it; investigate; reply with the result; "
        "decide and close, or release with a handoff if unfinished.",
        "C (worker): hello; hold next; take what is offered or report nothing; leave things tidier (e.g. people "
        "directory) only if it is clearly useful; do not invent work.",
        "Day one: the forum is empty, so agents choose paths and structure; good agents keep it small and obvious.",
    ]
    s.s["facts"] = [
        "ShopCo's repository shop-api: Python 3.12, pytest, GitHub Actions CI; staging deploys run scripts/build.sh (docker build) then Argo CD. The agents work in a checkout of shop-api on main and may create branches; git, pytest, docker, and gh work there.",
        "Lena Brandt reviews and merges shop-api PRs. Omar Haddad leads the backend team.",
    ]
    return s.build()


# -- L2: one developer, a month ------------------------------------------------------------------


SAM = "mail:sam@rivera.example"


def l2_solo_month() -> dict:
    s = Seed(
        "l2-solo-month",
        "A solo developer's forum after a month: closed work, a pending question, an expired hold",
        "solo",
        "Sam builds Tidepool, a tide-forecast app, alone, with an IDE agent and a mail assistant. A month of work: "
        "closed tasks with learnings, image resizing blocked on a question to Sam, an App Store reply whose agent "
        "vanished, an idea waiting as an offer, and a weekly dependency update.",
    )
    s.token("ide", "ide-agent", ["swift", "repo"])
    s.token("mail", "mail-assistant", ["mail"])
    s.token("worker", "coding-agent", ["swift", "repo", "python"])
    s.token("w-ide", "ide-agent", ["swift", "repo"])
    s.token("w-mail", "mail-assistant", ["mail"])
    s.token("w-bg", "coding-agent", ["swift", "repo", "python"])

    # Day 0: Sam's IDE agent sets the forum up.
    a, e = s.session("w-ide", principal=SAM, from_user="Let's use the forum to keep track of Tidepool work from now on. "
                     "Set up whatever you need.", client="ide")
    me = s.person(a, "sam-rivera", "Sam Rivera", [SAM, "github:samrivera"],
                  "Indie developer; builds Tidepool alone. Reads the IDE chat while coding, mail twice a day.")
    s.create(a, "tidepool", "Tidepool app", "Tide-forecast app for iOS (repo tidepool-app) and its small backend "
             "(repo tidepool-api, Python, on Fly.io). Releases through TestFlight, then the App Store.",
             owners=[me], agreements="Branches feat/<short-name>. Sam reviews and merges every PR himself. "
             "Release notes go in CHANGELOG.md. Ask Sam at most one question at a time; he prefers options.")
    s.step(a, "talk", {"action": "route", "entry": f"${e}", "to": "none", "reason": "setting up the forum; no work"})
    s.advance("1d")

    # Day 1: the RSS crash.
    a, e = s.session("w-ide", principal=SAM, from_user="the app crashes when the NOAA feed has an empty item, can you "
                     "fix it? issue #41", client="ide")
    s.user_task(a, e, SAM, "tidepool/rss-crash", "Fix the crash on an empty NOAA feed item",
                "FeedParser crashes on an item without <pubDate> (issue tidepool-app#41).",
                report="Fixed: the parser now skips feed items without a date (PR #42, merged by you).",
                summary="FeedParser.parseItem skips items whose <pubDate> is missing; a unit test covers it. PR #42 "
                "merged; shipped in 1.4.1.",
                decision="Skip items without <pubDate> rather than failing the whole feed.",
                steps=[("reproduced with the feed of 2026-09-03 (item 7 has no pubDate)", "guard and test"),
                       ("guard in FeedParser.parseItem + unit test; PR #42 opened", "Sam reviews")],
                keys=["issue:tidepool-app#41"],
                learnings="NOAA items sometimes lack <pubDate>. Missing and empty are different cases in XMLParser: "
                "this fix handles a missing element only.")
    s.advance("3d")

    # Day 4: dark mode.
    a, e = s.session("w-ide", principal=SAM, from_user="add dark mode, follow the system setting", client="ide")
    s.user_task(a, e, SAM, "tidepool/dark-mode", "Dark mode following the system setting",
                "Support dark mode in every screen, following the iOS setting.",
                report="Dark mode is in (PR #45): every screen follows the system setting.",
                summary="Colours moved to an asset catalogue with dark variants; charts redrawn on trait change. "
                "PR #45 merged, shipped in 1.5.0.",
                decision="Follow the system setting only; no in-app toggle.",
                steps=[("asset catalogue with dark variants", "charts"), ("charts redraw on trait change", "PR")])
    s.advance("2d")

    # Day 6: onboarding copy, dropped.
    a, e = s.session("w-mail", principal=SAM, from_user="can you draft new onboarding copy? the current one is too long",
                     client="mail")
    v = s.create(a, "tidepool/onboarding-copy", "Shorter onboarding copy", "Draft shorter copy for the three "
                 "onboarding screens.", keys=[f"user:${e}"], take=True)
    s.checkpoint(a, v["t"], "drafted three variants in docs/onboarding-drafts.md", "Sam picks one")
    s.decide(a, "tidepool/onboarding-copy", "Dropped: Sam will remove the onboarding screens in 2.0 instead.",
             evidence="reported", settles=[f"${v['u']}"])
    s.step(a, "talk", {"action": "to_user", "principal": SAM, "kind": "result", "verbatim": True,
                       "text": "Noted: no new onboarding copy, since the screens go away in 2.0.", "re": [f"${e}"]})
    s.close(a, "tidepool/onboarding-copy", v["t"], "dropped", "Sam decided to remove onboarding in 2.0; drafts kept "
            "in docs/onboarding-drafts.md.")
    s.advance("4d")

    # Day 10: push notifications, and the weekly dependency update.
    a, e = s.session("w-ide", principal=SAM, from_user="push notifications for high tide, 1h before, user can turn "
                     "it off", client="ide")
    s.user_task(a, e, SAM, "tidepool/high-tide-push", "Push notification an hour before high tide",
                "Local notification one hour before each high tide at the user's saved station; a setting turns it off.",
                report="High-tide notifications are in (PR #48): one hour before, with a setting to turn them off.",
                summary="UNUserNotificationCenter schedules the next 7 days of high tides at each launch and at "
                "background refresh. PR #48 merged; in 1.5.1.",
                decision="Local notifications scheduled on device; no server push.",
                learnings="iOS keeps at most 64 pending local notifications: schedule a week, not a month.")
    a, e = s.session("w-ide", principal=SAM, from_user="every monday morning check for dependency updates and open a "
                     "PR if there are any", client="ide")
    v = s.create(a, "tidepool/deps-weekly", "Weekly dependency update", "Every Monday: check Swift packages and "
                 "tidepool-api's requirements for updates; open one PR with the safe ones, list the rest.",
                 keys=[f"user:${e}"], take=False, repeat={"cron": "0 9 * * MON", "tz": "Europe/Lisbon", "run_within": "4h"},
                 needs=["swift", "python", "repo"], est=30)
    s.step(a, "talk", {"action": "to_user", "principal": SAM, "kind": "result", "verbatim": True,
                       "text": "Set up: every Monday 09:00 Lisbon time an agent checks dependencies and opens a PR.",
                       "re": [f"${e}"]})
    s.advance("8d")

    # Day 18: comments idea, offered.
    a, e = s.session("w-mail", principal=SAM, from_user="idea for later: get off Disqus, it's slow and full of ads. "
                     "not urgent", client="mail")
    s.create(a, "tidepool/comments-provider", "Move comments off Disqus", "Sam wants comments off Disqus (slow, "
             "ads). Not urgent: compare self-hosted options (Remark42, Isso, Cusdis) and propose one.",
             keys=[f"user:${e}"], tags=["offer"], est=60, needs=["python"])
    s.advance("5d")

    # Day 23: image resizing, blocked on a question to Sam.
    a, e = s.session("w-ide", principal=SAM, from_user="the station photos are huge, resize them on upload", client="ide")
    v = s.create(a, "tidepool/image-resizing", "Resize station photos on upload", "Station photos are uploaded at "
                 "full size (4-8 MB). Resize on upload in tidepool-api to 3 widths (480, 960, 1920).",
                 keys=[f"user:${e}"], take=True)
    s.checkpoint(a, v["t"], "resizer written on branch feat/image-resize (Pillow), outputs JPEG for now; tests pass",
                 "pick the output format, then switch the encoder and open a PR")
    q = s.var("q")
    s.step(a, "talk", {"action": "ask", "at": "tidepool/image-resizing", "to": "user", "text": "Which format for the "
                       "resized station photos: WebP (supported everywhere, ~30% smaller than JPEG) or AVIF (~50% "
                       "smaller, slower to encode, iOS 16+ only)?", "options": ["WebP", "AVIF"],
                       "recommendation": "WebP", "default": "WebP", "due": "in 10d"}, {q: "question"})
    s.release(a, v["t"], "asked Sam WebP or AVIF", "switch the encoder to the chosen format, open the PR",
              reason="blocked", waits_for={"question": f"${q}"})
    s.advance("4d")

    # Day 27: an App Store rejection; its agent vanishes mid-work.
    a, e = s.session("w-mail", principal=SAM, from_user="Apple rejected 1.6 (guideline 5.1.1, they want an account "
                     "deletion option?? we don't even have accounts). can you draft a reply", client="mail")
    v = s.create(a, "tidepool/app-store-511", "Reply to App Store rejection 5.1.1 of 1.6", "Apple rejected 1.6 "
                 "citing 5.1.1 (account deletion). Tidepool has no accounts: only an anonymous device id for "
                 "favourites sync. Draft a reply in App Store Connect's resolution centre for Sam to send.",
                 keys=[f"user:${e}"], take=True, needs=["mail"])
    s.checkpoint(a, v["t"], "read the rejection: reviewer saw the 'Sync favourites' screen and took it for an account",
                 "draft the reply explaining the anonymous device id; put the draft in the node for Sam")
    s.advance("2d")

    # Day 29: the worker's last visit.
    b, _ = s.session("w-bg", intent="pick up work")
    s.step(b, "hold", {"action": "next"}, may_fail=True)
    s.advance("1d")

    s.human(
        "sam",
        "Sam Rivera",
        SAM,
        [
            {"when": "told", "match": ["pr", "pull request"], "say": "cool, I'll look tonight"},
            {"when": "asked", "match": ["avif", "webp", "format"], "say": "AVIF, as I said."},
            {"when": "asked", "say": "sure, go ahead"},
        ],
    )
    s.world("A", ["test", "pytest", "build"], "tidepool-api: 61 tests pass on feat/image-resize.")
    s.world("A", ["avif", "encoder", "pillow", "convert", "edit", "switch"],
            "Switched to pillow-avif-plugin: 480/960/1920 widths encode in 0.4/1.1/3.8 s per photo; sizes 38/96/310 "
            "KB from a 6.2 MB original. Tests updated, 61 pass.")
    s.world("A", ["push", "pull request", "pr "], "PR #53 opened on tidepool-api (feat/image-resize); CI green.")
    s.world("B", ["crash", "log", "sentry", "report", "feed", "rss"],
            "Crash reports since 1.6 (3 users): EXC_BAD_ACCESS in FeedParser.parseDate on items whose <pubDate></pubDate> "
            "is present but empty (NOAA feed of 2026-10-02, item 4).")
    s.world("B", ["send", "mail", "email", "reply"], "Mail sent.")
    s.world("C", ["test", "build", "swift"], "Build and tests pass on main.")
    s.world("C", ["outdated", "pip", "package", "dependenc"], "2 updates: Pillow 11.3 -> 11.4 (patch), "
            "swift-collections 1.2 -> 1.3 (minor). Both build and pass tests.")
    s.world("C", ["app store", "draft", "resolution", "reply"], "Draft saved.")
    s.agent(
        "A",
        "ide",
        ide_role("Sam Rivera, an indie developer who builds Tidepool, a tide-forecast app, alone", SAM,
                 "You can talk to him in the IDE chat, edit and test the tidepool-app (Swift) and tidepool-api (Python) "
                 "code, push branches, and open pull requests."),
        [{"from": "human", "text": "morning! where are we with the photo resizing? and for the format question: let's "
          "go with AVIF actually, the size savings are worth it."}],
        human="sam",
        max_turns=10,
    )
    s.agent(
        "B",
        "mail",
        f"You are Sam Rivera's mail assistant. Your client's configuration identifies him as {SAM}. You read the "
        "mail he sends you and answer him by mail; you can read the app's crash reports and draft and send mail. "
        f"{ACTIONS} Everything else in your reply is sent to Sam as your mail reply.",
        [{"from": "event", "text": "New mail from Sam Rivera <sam@rivera.example> to you, subject 'rss + comments': "
          "\"Can you remind me what we did about the RSS crash last month? A user says the app crashes on the feed "
          "again. Also add a task to look into moving comments off Disqus.\""}],
        human="sam",
        start={"at": "20s"},
        max_turns=10,
    )
    s.agent(
        "C",
        "worker",
        worker_role("Sam Rivera's", "You can build and test the tidepool-app (Swift) and tidepool-api (Python) code, "
                    "push branches, and open pull requests, and draft text."),
        [{"from": "event", "text": SCHEDULED}],
        start={"at": "3m"},
        max_turns=10,
    )
    s.s["intended"] = [
        "A (Sam's IDE agent): hello with principal and Sam's words (they answer the pending WebP/AVIF question: "
        "from_user with answers, or route); find the image-resizing node and its checkpoint; take it; switch the "
        "encoder to AVIF; intent before pushing; PR; report to Sam; leave the node released or closed sensibly.",
        "B (Sam's mail assistant): from_user for the mail; find the closed rss-crash node and its learning (missing vs "
        "empty pubDate), see that the new crash is the empty case it did not cover; create or reopen work for it; "
        "for the comments idea, find the existing offer tidepool/comments-provider instead of a duplicate; reply to "
        "Sam with to_user and in the mail.",
        "C (worker): hold next: the App Store reply whose holder vanished, or the comments offer; checkpoint; finish "
        "or release with a handoff.",
    ]
    s.s["facts"] = [
        "Repositories: tidepool-app (Swift, Xcode project, FeedParser in Sources/Feed/FeedParser.swift) and tidepool-api (Python 3.12, FastAPI, Pillow, deploys on Fly.io; the resizer is in api/images/resize.py on branch feat/image-resize). The agents work in checkouts of both; git, swift build/test, pytest, gh work.",
        "Crash reports are in Sentry (project tidepool-ios). App Store Connect: the agents can read review messages and draft replies; only Sam sends them.",
        "On branch feat/image-resize the resizer still writes JPEG: nothing about AVIF or WebP is done yet. The next "
        "pull request numbers are #54 in tidepool-api and #49 in tidepool-app; #41-#48 are merged.",
    ]
    return s.build()


# -- L3: a company, a quarter ---------------------------------------------------------------------

CORVID_PEOPLE = [
    # slug, name, mail, slack, team, body
    ("priya-raman", "Priya Raman", "priya", "U0PRIYA", "platform", "Platform team lead. Runs the roadmap review."),
    ("elif-yilmaz", "Elif Yilmaz", "elif", "U0ELIF", "platform", "SRE; owns databases and the on-call rota."),
    ("alice-novak", "Alice Novak", "alice", "U0ALICE", "payments", "Payments engineer; works with her IDE agent."),
    ("bob-meyer", "Bob Meyer", "bob", "U0BOB", "payments", "Payments team lead."),
    ("chen-wei", "Chen Wei", "chen", "U0CHEN", "data", "Data engineer; nightly pipelines."),
    ("dana-kim", "Dana Kim", "dana", "U0DANA", "data", "Data team lead; owns the warehouse."),
    ("femi-adeyemi", "Femi Adeyemi", "femi", "U0FEMI", None, "Frontend engineer (customer portal)."),
    ("gita-rao", "Gita Rao", "gita", "U0GITA", None, "Product manager for shipping and billing."),
    ("hugo-laurent", "Hugo Laurent", "hugo", "U0HUGO", None, "CTO; owner of the forum root."),
    ("ines-costa", "Ines Costa", "ines", "U0INES", None, "Support lead."),
]

CORVID_DONE = [
    # area, slug, title, summary, learnings
    ("platform", "ci-cache", "Speed up CI with a shared build cache", "CI median 14 -> 6 min with the S3 build cache.",
     "Cache keys must include the toolchain version."),
    ("platform", "tls-renewal", "Automate TLS certificate renewal", "cert-manager renews all 14 certificates; alerts "
     "at 14 days left.", None),
    ("platform", "log-retention", "Cut log retention to 30 days", "Loki retention 30 d; saved 2.1 TB.", None),
    ("platform", "k8s-1-31", "Upgrade the clusters to Kubernetes 1.31", "Both clusters on 1.31; no workload changes "
     "needed.", "The ingress controller must be upgraded first."),
    ("platform", "staging-reset", "Nightly staging database reset", "Staging is restored from the anonymised dump at "
     "03:00 UTC.", None),
    ("payments", "fx-rounding", "Make refund and charge rounding consistent", "Half-even everywhere (commit 9f3e2c1).",
     "Finance's export spec v3 is the reference for rounding."),
    ("payments", "stripe-webhooks-v2", "Move to Stripe webhooks v2", "All 6 handlers on v2; v1 endpoint removed.", None),
    ("payments", "invoice-pdf-fonts", "Fix missing glyphs in invoice PDFs", "Embedded Noto Sans; Turkish and Polish "
     "names render.", None),
    ("payments", "chargeback-report", "Weekly chargeback report for finance", "Report runs Mondays 07:00 UTC into "
     "finance's shared drive.", None),
    ("data", "shipments-partitioning", "Partition the shipments table by day", "Queries on 90 days 11x faster.",
     "Backfills must write whole partitions."),
    ("data", "dbt-tests", "Add dbt tests for the core models", "42 tests; 3 real bugs found and fixed.", None),
    ("data", "eta-model-v2", "Retrain the ETA model with port congestion features", "MAE 9.1 h -> 6.4 h; deployed "
     "behind a flag, then on.", "Port congestion data arrives 6 h late: use yesterday's."),
    ("data", "pii-masking", "Mask PII in the analytics replica", "Emails and phone numbers hashed; analysts confirmed.",
     None),
    ("support", "macro-refresh", "Refresh the support macros", "31 macros rewritten, 12 retired.", None),
    ("support", "sla-dashboard", "Support SLA dashboard", "Grafana dashboard support-sla, first-response p90 shown.",
     None),
]

CORVID_OFFERS = [
    # area, slug, title, body, needs, est
    ("data", "late-arrivals-report", "Weekly late-arrivals report by carrier", "Gita wants a weekly table: per "
     "carrier, shipments arriving more than 24 h after the ETA, last 4 weeks. Query the warehouse "
     "(analytics.shipments), post the table here for Gita.", ["sql"], 30),
    ("data", "dedupe-tracking-events", "Remove duplicate tracking events", "About 0.4 % of rows in "
     "raw.tracking_events are exact duplicates since the carrier API retry change. Write and run a dedupe, keep the "
     "earliest row.", ["sql", "python"], 60),
    ("platform", "rotate-ci-secrets", "Rotate the CI deploy keys", "The CI deploy keys are 14 months old; policy says "
     "12. Rotate them in the CI settings and the clusters.", ["ci", "k8s"], 20),
    ("support", "tag-cleanup", "Merge duplicate ticket tags", "The help desk has 40 tags, many duplicates "
     "(shipping-delay, delay-shipping). Propose a merge list for Ines.", ["support"], 30),
]


def l3_company_quarter() -> dict:
    s = Seed(
        "l3-company-quarter",
        "A company's forum after a quarter: ten people, three teams, work in flight",
        "company",
        "Corvid Freight, a logistics company: people and teams, closed work with learnings, the Postgres 16 upgrade "
        "handed off with its prod date, Alice's refund retries blocked on her answer, offers for workers.",
    )
    s.token("ide", "ide-agent", ["python", "repo"])
    s.token("slack", "chat-worker", ["chat"])
    s.token("worker", "coding-agent", ["python", "sql"])
    for t, c in (("w-dir", "directory-keeper"), ("w-ide", "ide-agent"), ("w-slack", "chat-worker"),
                 ("w-bg", "coding-agent")):
        s.token(t, c, ["python", "repo", "sql", "ci", "k8s"] if c != "chat-worker" else ["chat"])

    a, _ = s.session("w-dir", intent="set up the company's directory and areas")
    paths = {}
    for slug, name, mail, slack, _team, body in CORVID_PEOPLE:
        paths[slug] = s.person(a, slug, name, [f"mail:{mail}@corvid.example", f"slack:{slack}"], body)
    teams = {}
    for team, title, body in (
        ("platform", "Platform team", "CI, clusters, databases, on-call. Channel #platform (C0PLAT)."),
        ("payments", "Payments team", "Billing, refunds, invoices, the ledger export. Channel #payments (C0PAY)."),
        ("data", "Data team", "Warehouse, nightly pipelines, the ETA model. Channel #data (C0DATA)."),
    ):
        owners = [paths[p[0]] for p in CORVID_PEOPLE if p[4] == team]
        teams[team] = s.team(a, team, title, owners, body)
    s.step(a, "node", {"action": "update", "path": "/", "version": 1, "agreements":
                       "Paths are <area>/<short-name>. Put the issue key on a node when there is one "
                       "(issue:<repo>#<n>). People and teams live in @people and @teams; keep them current. Close "
                       "with a summary a stranger can use."})
    for area, title, owner, body in (
        ("platform", "Platform", "platform", "CI, the Kubernetes clusters, Postgres, observability."),
        ("payments", "Payments", "payments", "The payments service: charges, refunds, invoices, finance exports."),
        ("data", "Data", "data", "The warehouse (analytics.*), nightly pipelines (Airflow dag nightly_core, 02:00 "
         "UTC), and the ETA model."),
        ("support", "Support", None, "Help desk tooling, macros, SLAs. Owner: Ines Costa."),
    ):
        s.create(a, area, title, body, owners=[teams[owner]] if owner else [paths["ines-costa"]])
    s.record(a, "kb/deploys", "How we deploy", "Every service deploys from main through Argo CD; staging first, prod "
             "after a green smoke test. Freezes are announced in #platform.")
    s.record(a, "kb/oncall", "On-call rota", "Platform on-call rotates weekly on Mondays; this quarter Elif Yilmaz and "
             "Marco (contractor) alternate. Data on-call: Chen Wei.")
    s.advance("5d")

    rnd = random.Random(3)
    for i, (area, slug, title, summary, learnings) in enumerate(CORVID_DONE):
        a, _ = s.session(rnd.choice(["w-bg", "w-ide"]), intent=title)
        s.task(a, f"{area}/{slug}", title, f"{title}.", summary=summary, learnings=learnings,
               steps=[("started; read the code and the runbook", "make the change and test it")],
               decision=f"{summary.split(';')[0]}")
        if i % 3 == 2:
            s.advance("4d")

    # A person's request in a thread, answered and closed.
    a, _ = s.session("w-slack", intent="answer #support")
    v = s.thread_request(a, "support/refund-eta-macro", "Macro for refund ETA questions", "support/T3020", "M1",
                         "Ines Costa", "can someone add a macro that tells customers refunds take 5-10 business days?",
                         ack="On it, I'll post here when it's added.")
    s.answer_thread(a, "support/refund-eta-macro", v, "support/T3020", "Added the macro 'Refund ETA' (5-10 business "
                    "days, with the bank caveat).", "Macro 'Refund ETA' added.", "Macro added; Ines confirmed.")
    s.advance("6d")

    # The Postgres 16 upgrade: requested by Elif, staging done, handed off with the prod date.
    a, e = s.session("w-ide", principal="mail:elif@corvid.example", from_user="plan and do the postgres 16 upgrade, "
                     "staging first. prod needs a window, coordinate with me", client="ide")
    v = s.create(a, "platform/pg16-upgrade", "Upgrade Postgres 14 to 16", "Upgrade the main cluster (pg-main) from "
                 "14 to 16 with pg_upgrade --link. Staging first; prod in an agreed window.",
                 keys=[f"user:${e}", "issue:infra#212"], take=True, owners=[teams["platform"]])
    s.checkpoint(a, v["t"], "runbook written: docs/runbooks/pg16.md", "upgrade staging")
    s.release(a, v["t"], "runbook written: docs/runbooks/pg16.md", "upgrade staging with the runbook",
              reason="handoff")
    s.advance("3d")
    a, _ = s.session("w-bg", intent="upgrade staging to Postgres 16")
    t = s.take(a, "platform/pg16-upgrade")
    s.checkpoint(a, t, "staging upgraded; replication and the app's smoke tests pass", "agree a prod window")
    s.release(a, t, "staging upgraded and tested", "agree the prod window with Elif, then run the runbook in "
              "the window", reason="handoff")
    s.advance("2d")
    a, e = s.session("w-ide", principal="mail:elif@corvid.example", from_user="prod window for pg16: Sunday Oct 12, "
                     "02:00-04:00 UTC. I'll be on call", client="ide")
    t = s.take(a, "platform/pg16-upgrade")
    s.step(a, "talk", {"action": "route", "entry": f"${e}", "to": "/platform/pg16-upgrade", "reason": "the prod window"})
    s.decide(a, "platform/pg16-upgrade", "Prod upgrade on 2026-10-12, 02:00-04:00 UTC, Elif on call; maintenance "
             "banner from 01:30.", evidence="reported")
    s.release(a, t, "prod window agreed: 2026-10-12 02:00-04:00 UTC with Elif",
              "on 2026-10-12 at 02:00 UTC run docs/runbooks/pg16.md on prod", reason="blocked",
              waits_for={"time": "2026-10-12T02:00:00Z"})
    s.advance("8d")

    # Offers.
    a, _ = s.session("w-dir", intent="file the work people asked for")
    for area, slug, title, body, needs, est in CORVID_OFFERS:
        s.create(a, f"{area}/{slug}", title, body, tags=["offer"], needs=needs, est=est)
    s.advance("10d")

    # Alice's refund retries: blocked on her answer.
    a, e = s.session("w-ide", principal="mail:alice@corvid.example", from_user="make failed refunds retry "
                     "automatically", client="ide")
    v = s.create(a, "payments/refund-retries", "Retry failed refunds automatically", "Alice asked that failed refunds "
                 "be retried automatically. Draft on branch feat/refund-retries.", keys=[f"user:${e}"], take=True,
                 priority=1)
    s.checkpoint(a, v["t"], "retry loop drafted on branch feat/refund-retries with exponential backoff (not merged)",
                 "set the maximum number of attempts once Alice answers, then tests and a PR")
    q = s.var("q")
    s.step(a, "talk", {"action": "ask", "at": "payments/refund-retries", "to": "user", "text": "How many times should a "
                       "failed refund be retried before it is reported to a human: 3 or 5?", "options": ["3", "5"],
                       "recommendation": "3", "default": "3", "due": "in 7d"}, {q: "question"})
    s.release(a, v["t"], "asked Alice how many retries", "implement the chosen count, test, open a PR",
              reason="blocked", waits_for={"question": f"${q}"})
    s.advance("2d")

    s.human(
        "alice",
        "Alice Novak",
        "mail:alice@corvid.example",
        [
            {"when": "told", "match": ["pr", "pull request"], "say": "Thanks, I'll review it."},
            {"when": "asked", "say": "Use your judgement, I'll see it in the PR."},
        ],
    )
    s.people(
        "priya",
        "Priya Raman (platform lead)",
        "thread T3301 of #platform (chat:platform/T3301)",
        [
            {"on": "post", "when": "always", "say": "thanks! is the maintenance banner sorted too?", "once": True},
        ],
    )
    s.world("A", ["runbook", "pg16", "postgres", "status", "argo", "banner"],
            "docs/runbooks/pg16.md exists; the status page has a scheduled maintenance for 2026-10-12 01:30-04:00 UTC.")
    s.world("B", ["test", "pytest"], "payments-service: 230 tests pass on feat/refund-retries.")
    s.world("B", ["edit", "implement", "max", "attempt", "change", "set"],
            "MAX_REFUND_ATTEMPTS = 3 in payments/refunds/retry.py; after the third failure the refund is marked "
            "needs_human and posted to #payments. 233 tests pass.")
    s.world("B", ["push", "pull request", "pr "], "PR #241 opened (https://git.corvid.example/payments/pull/241); CI green.")
    s.world("C", ["select", "sql", "query", "warehouse"],
            "Query ran: 12 carriers; late (>24 h after ETA) share over the last 4 weeks ranges 1.2 %-9.8 %; worst: "
            "Baltic Line 9.8 %, Nordhaul 7.1 %.")
    s.world("C", ["dedupe", "delete", "duplicate"], "Dedupe dry run: 1,912,440 duplicate rows of 478 M (0.40 %).")
    s.agent(
        "A",
        "slack",
        chat_role("Corvid Freight", "#platform", "You can read and post in the thread, read repositories, runbooks, the "
                  "status page, and dashboards."),
        [{"from": "event", "text": thread_event("#platform", "T3301", "M1", "Priya Raman (platform lead)",
                                                "who's on the Postgres 16 upgrade and when is prod happening? need it "
                                                "for the roadmap review tomorrow")}],
        conversation="priya",
        max_turns=10,
    )
    s.agent(
        "B",
        "ide",
        ide_role("Alice Novak, a payments engineer at Corvid Freight", "mail:alice@corvid.example",
                 "You can talk to her in the IDE chat, edit and test the payments-service code, push branches, open "
                 "pull requests, and read CI."),
        [{"from": "human", "text": "hey, can you pick up the refund retries work? I answered the question the agent "
          "asked last week: 3 retries max."}],
        human="alice",
        start={"at": "20s"},
        max_turns=10,
    )
    s.agent(
        "C",
        "worker",
        worker_role("Corvid Freight", "You can query the warehouse with SQL and run Python jobs on the data host."),
        [{"from": "event", "text": SCHEDULED}],
        start={"at": "2m"},
        max_turns=10,
    )
    s.s["intended"] = [
        "A (chat integration in #platform): Priya's message is a request: record it (talk request on the existing "
        "pg16 node, or a node keyed by the thread that links to it), answer from what the forum knows (handed off "
        "after staging; prod 2026-10-12 02:00-04:00 UTC, Elif on call; nobody holds it now), said before posting, "
        "confirm, decide/settle, and do not create a duplicate pg16 node.",
        "B (Alice's IDE agent): hello with principal; Alice's words answer the pending question (from_user answers, "
        "or route to the node); take payments/refund-retries; set 3 attempts; intent before pushing; PR; report to "
        "Alice; close or release.",
        "C (worker with sql, python): hold next; take an offer it has the caps for (late-arrivals report or the "
        "dedupe); checkpoint; finish (decide, close) or release with a handoff.",
    ]
    s.s["facts"] = [
        "Repositories: payments-service (Python; refunds in payments/refunds/; branch feat/refund-retries holds the retry loop with backoff, MAX_REFUND_ATTEMPTS not set yet), infra (runbooks in docs/runbooks, including pg16.md), airflow-dags. git, pytest, gh work in checkouts of them.",
        "The warehouse has analytics.shipments (shipment_id, carrier, eta, arrived_at) and raw.tracking_events; agents with sql can query it read-only and may run dedupes in a transaction.",
        "The status page shows a scheduled maintenance for 2026-10-12 01:30-04:00 UTC (database upgrade).",
    ]
    return s.build()


# -- L4: a mature forum, a year ---------------------------------------------------------------------

FIRST = ["Maria", "Kenji", "Olu", "Sofia", "Tariq", "Hannah", "Diego", "Mei", "Lukas", "Amara", "Ravi", "Freya",
         "Jonas", "Leila", "Pavel", "Nadia", "Theo", "Yuki", "Grace", "Emeka", "Ana", "Viktor", "Zara", "Ben",
         "Ingrid", "Sami", "Clara", "Dev", "Rosa", "Felix"]
LAST = ["Lopez", "Tanaka", "Okafor", "Rossi", "Haddad", "Becker", "Silva", "Chen", "Weber", "Diallo", "Iyer",
        "Larsen", "Berg", "Nasser", "Novak", "Petrova", "Martin", "Sato", "Kim", "Eze", "Costa", "Ivanov", "Malik",
        "Cohen", "Holm", "Aalto", "Fischer", "Rao", "Ortiz", "Wagner"]
TEAMS4 = ["data", "web", "payments", "platform", "support-tools", "mobile", "security", "growth"]
TEMPLATES4 = {
    "data": ["Backfill {t} for {m}", "Add dbt tests for {t}", "Speed up the {t} model", "Fix null {c} values in {t}",
             "Document the {t} model", "Move {t} to incremental loads"],
    "web": ["Fix the {p} page layout on small screens", "Add {f} to the {p} page", "Upgrade {l} to the next major",
            "Cut the {p} page's bundle size", "Fix the broken link checker on {p}"],
    "payments": ["Handle {e} webhooks", "Retry failed {e} payouts", "Reconcile {e} settlements for {m}",
                 "Fix rounding in {e} refunds", "Add {e} to the finance export"],
    "platform": ["Upgrade {s} to the latest minor", "Add alerts for {s} saturation", "Move {s} to the new cluster",
                 "Cut {s} costs", "Write the {s} runbook"],
    "support-tools": ["Add {f} to the agent console", "Fix {x} in the CSV export", "Speed up ticket search",
                      "Add a macro for {x}", "Fix the SLA timer for {x}"],
    "mobile": ["Fix the {p} screen crash on Android 15", "Add {f} to the {p} screen", "Reduce app start time",
               "Upgrade {l} in the app", "Fix push notification opt-in on iOS"],
    "security": ["Rotate the {s} credentials", "Review access to {s}", "Patch {l} for the latest CVE",
                 "Enable SSO for {s}", "Run the quarterly access review for {s}"],
    "growth": ["A/B test {f} on the {p} page", "Report on {m} sign-up funnel", "Clean up the {e} campaign tags",
               "Add {f} to onboarding emails", "Measure {f} adoption"],
}
FILL4 = {
    "t": ["orders", "customers", "sessions", "returns", "inventory", "page_views", "refunds", "carts"],
    "c": ["country", "currency", "email", "sku", "store_id"],
    "m": ["January", "February", "March", "April", "May", "June", "July", "August", "September"],
    "p": ["checkout", "product", "search", "account", "basket", "order history"],
    "f": ["saved filters", "dark mode", "bulk actions", "wishlists", "gift cards", "address autocomplete"],
    "l": ["React", "Next.js", "Kotlin", "SwiftUI", "OpenSSL", "Django", "Webpack"],
    "e": ["Adyen", "PayPal", "Klarna", "SEPA", "Apple Pay"],
    "s": ["Kafka", "Redis", "Postgres", "the search cluster", "Vault", "the CDN", "Grafana"],
    "x": ["refund requests", "address changes", "late deliveries", "damaged items", "VIP customers"],
}


def l4_mature_year() -> dict:
    s = Seed(
        "l4-mature-year",
        "A company's forum after a year: hundreds of nodes, a fix that regressed, duplicates, stale work",
        "mature",
        "Halcyon, an online retailer, a year into its forum: 30 people in 8 teams, about 300 closed nodes with "
        "summaries and learnings, area agreements, a knowledge base, a CSV time-zone fix from five months ago that "
        "has now regressed, two duplicate open nodes, stale open work, and the data team's work in flight.",
    )
    s.token("ide", "ide-agent", ["python", "repo", "sql"])
    s.token("ide2", "ide-agent", ["python", "repo"])
    s.token("worker", "coding-agent", ["python", "repo", "sql"])
    for t, c in (("w-dir", "directory-keeper"), ("w-ide", "ide-agent"), ("w-bg", "coding-agent"),
                 ("w-slack", "chat-worker")):
        s.token(t, c, ["python", "repo", "sql", "ci"])
    rnd = random.Random(4)

    a, _ = s.session("w-dir", intent="set up Halcyon's directory")
    people: dict[str, list[str]] = {t: [] for t in TEAMS4}
    for i, (f, l) in enumerate(zip(FIRST, LAST)):
        team = TEAMS4[i % len(TEAMS4)]
        slug = f"{f}-{l}".lower()
        role = "lead" if i < len(TEAMS4) else "engineer"
        p = s.person(a, slug, f"{f} {l}", [f"mail:{f.lower()}@halcyon.example", f"slack:U{i:03d}H"],
                     f"{role.capitalize()}, {team} team.")
        people[team].append(p)
    teams = {t: s.team(a, t, f"{t.replace('-', ' ').capitalize()} team", people[t], f"The {t} team.") for t in TEAMS4}
    s.step(a, "node", {"action": "update", "path": "/", "version": 1, "agreements":
                       "Paths are <team>/<short-name>. Put the issue key on the node (issue:<repo>#<n>). Close with "
                       "a summary and, when there is one, a learning. Ask in the team's channel before work that "
                       "touches another team's area."})
    for t in TEAMS4:
        agreements = None
        body = f"Work of the {t} team."
        if t == "data":
            body = ("The warehouse (Snowflake, account halcyon-prod) and the nightly pipelines. The data team owns "
                    "the warehouse; its lead is Maria Lopez.")
            agreements = ("Nightly pipelines run in Airflow (dag nightly_core) at 02:00 UTC; failures page the data "
                          "on-call (see kb/data-oncall). Never backfill more than 7 days without saying so in #data "
                          "first. Model changes need a dbt test.")
        s.create(a, t, t.replace("-", " ").capitalize(), body, owners=[teams[t]],
                 **({"agreements": agreements} if agreements else {}))
    s.record(a, "kb/data-oncall", "Data on-call", "The data on-call rotates weekly between the data team's "
             "engineers; the current one is in PagerDuty's schedule 'data-primary'.")
    s.record(a, "kb/nightly-pipelines", "How the nightly pipelines work", "Airflow dag nightly_core: extract (02:00) "
             "-> dbt run (02:40) -> dbt test -> publish to the BI schemas (04:00). A failed test blocks publishing; "
             "the on-call reruns from the failed task.")
    s.record(a, "kb/warehouse", "Warehouse ownership and access", "Snowflake account halcyon-prod, owned by the data "
             "team. Access requests go through the security team's access review; analysts get read-only roles.")
    s.advance("10d")

    n = 0
    for week in range(48):
        a, _ = s.session(rnd.choice(["w-ide", "w-bg", "w-slack"]), intent="this week's work")
        for _ in range(rnd.randint(4, 8)):
            team = rnd.choice(TEAMS4)
            tmpl = rnd.choice(TEMPLATES4[team])
            title = tmpl.format(**{k: rnd.choice(v) for k, v in FILL4.items()})
            n += 1
            slug = f"{title.lower().replace(' ', '-').replace('/', '-').replace('.', '')[:40].strip('-')}-{n}"
            issue = rnd.randint(100, 999)
            outcome = rnd.choices(["done", "dropped", "superseded"], [12, 2, 1])[0]
            summary = {
                "done": f"Done; PR #{issue} merged and deployed.",
                "dropped": "Dropped: no longer needed after the quarterly planning.",
                "superseded": "Superseded by a broader change in the same area.",
            }[outcome]
            learnings = rnd.choice([None, None, None, f"Check the {team} runbook before changing this; it has a "
                                    "manual step."])
            s.task(a, f"{team}/{slug}", title, f"{title} (issue {team}#{issue}).", summary=summary, outcome=outcome,
                   learnings=learnings, keys=[f"issue:{team}#{issue}"])
        if week == 10:
            s.create(a, "web/legacy-ie11", "Keep the checkout working on IE 11", "Corporate customers on IE 11 report "
                     "a blank checkout. Investigate a polyfill bundle.", tags=["offer"], est=120)
        if week == 16:
            s.create(a, "mobile/tablet-layout", "Tablet layout for the product screen", "Use the width on tablets.",
                     tags=["offer"], est=240)
        if week == 30:
            v = s.create(a, "support-tools/csv-export-timezone", "CSV exports show UTC instead of the account's time "
                         "zone", "Customers' CSV order exports show times in UTC (issue support-tools#214).",
                         keys=["issue:support-tools#214"], take=True)
            s.checkpoint(a, v["t"], "found: the exporter formats datetimes without a zone", "convert to "
                         "accounts.settings.tz before formatting")
            s.decide(a, "support-tools/csv-export-timezone", "Convert every timestamp to the account's time zone "
                     "(accounts.settings.tz) in the exporter.")
            s.close(a, "support-tools/csv-export-timezone", v["t"], "done", "The download path of the CSV export now "
                    "converts timestamps to accounts.settings.tz (PR #388).", learnings="Timestamps are stored in "
                    "UTC. The scheduled export runs in a separate container (export-cron) that was not changed; its "
                    "TZ is unset.")
        if week == 40:
            s.create(a, "data/warehouse-cost-review", "Review warehouse costs", "Snowflake costs grew 40 % this "
                     "quarter; find the biggest warehouses and queries.", tags=["offer"], needs=["sql"], est=90)
        if week == 44:
            s.create(a, "data/review-warehouse-spend", "Why is Snowflake spend up?", "Finance asks why Snowflake spend "
                     "is up; look at credit usage by warehouse.", tags=["offer"], needs=["sql"], est=60)
        s.advance("7d")

    a, _ = s.session("w-ide", intent="upgrade Airflow")
    v = s.create(a, "data/airflow-2-10", "Upgrade Airflow to 2.10", "Upgrade the scheduler and workers from 2.7 to "
                 "2.10; the nightly_core dag must keep running.", keys=["issue:data#1201"], take=True,
                 owners=[teams["data"]])
    s.checkpoint(a, v["t"], "staging on 2.10; nightly_core ran green twice", "prod upgrade in the Tuesday window")
    s.release(a, v["t"], "staging on 2.10 and green", "upgrade prod in Tuesday's 10:00 UTC window, then watch one "
              "nightly run", reason="handoff")
    s.create(a, "data/nightly-sla-alerts", "Alert when nightly_core publishes late", "Page the data on-call when the "
             "BI schemas are not published by 05:00 UTC.", tags=["offer"], needs=["python"], est=45)
    s.advance("2d")

    s.human("kenji", "Kenji Tanaka", "mail:kenji@halcyon.example",
            [{"when": "asked", "say": "Whatever you think is most useful for a first day."}])
    s.human("sofia", "Sofia Rossi", "mail:sofia@halcyon.example",
            [{"when": "told", "match": ["pr", "pull request"], "say": "Great, I'll review."},
             {"when": "asked", "say": "Your call."}])
    s.world("B", ["export", "csv", "timezone", "time zone", "log", "cron", "scheduled", "repro", "grep", "read"],
            "The CSV export has two paths: the download button (web app, converts to accounts.settings.tz since PR "
            "#388) and the scheduled e-mail export (container export-cron), which formats with the container's TZ, "
            "unset, so UTC. All 9 complaints since Monday are from scheduled exports.")
    s.world("B", ["fix", "edit", "change", "patch", "implement"],
            "export-cron now converts with accounts.settings.tz, through the same helper as the download path; a test "
            "covers both paths. 512 tests pass.")
    s.world("B", ["push", "pull request", "pr "], "PR #902 opened (https://git.halcyon.example/support-tools/pull/902); "
            "CI green.")
    s.world("C", ["select", "sql", "query", "credit", "warehouse", "snowflake"],
            "Credit usage by warehouse, last 90 days: TRANSFORM_WH 61 %, BI_WH 24 %, ADHOC_WH 15 %. TRANSFORM_WH's "
            "top query: the full refresh of model sessions (2.1 h nightly).")
    s.agent(
        "A",
        "ide",
        ide_role("Kenji Tanaka, who joined Halcyon's data team today", "mail:kenji@halcyon.example",
                 "You can talk to him in the IDE chat, read the company's repositories, docs, and the warehouse."),
        [{"from": "human", "text": "Hi, first day on the data team. Could you give me a quick orientation: how do we "
          "run the nightly pipelines here, who owns the warehouse, and what is the data team working on right now?"}],
        human="kenji",
        max_turns=10,
    )
    s.agent(
        "B",
        "ide2",
        ide_role("Sofia Rossi, an engineer on Halcyon's support-tools team", "mail:sofia@halcyon.example",
                 "You can talk to her in the IDE chat, edit and test the support-tools code, read logs, push branches, "
                 "and open pull requests."),
        [{"from": "human", "text": "Customers say the CSV exports show the wrong times again, off by an hour or two. "
          "I'm pretty sure we fixed this before? Please fix it properly this time."}],
        human="sofia",
        start={"at": "20s"},
        max_turns=10,
    )
    s.agent(
        "C",
        "worker",
        worker_role("Halcyon", "You can query the warehouse with SQL, run Python, and push to the data repositories."),
        [{"from": "event", "text": SCHEDULED + " If there is nothing you can do, leave the forum tidier than you "
          "found it."}],
        start={"at": "2m"},
        max_turns=10,
    )
    s.s["intended"] = [
        "A (Kenji's IDE agent, a newcomer): hello with principal and from_user; read the data area's agreements and "
        "the kb records (nightly pipelines, warehouse, on-call) and what is open under data; answer Kenji with a "
        "short orientation (to_user, then tell him); route his entry to none (no work needed).",
        "B (Sofia's IDE agent): find the closed csv-export-timezone node by text or key, read its learning (the "
        "scheduled export container was not changed); continue that work (reopen it, or a new node linked "
        "continues/derived_from) rather than starting blind; fix export-cron; intent; PR; report; close.",
        "C (worker): hold next; take an offer it can do, or tidy: notice the duplicate warehouse-cost nodes (merge "
        "or link duplicate), and stale offers; never close other people's work without a reason.",
    ]
    s.s["facts"] = [
        "Repositories: support-tools (Python, Django; CSV export in exports/csv.py; the download view in exports/views.py; scheduled e-mail exports are run by the export-cron container, entry point exports/cron.py, which calls the formatter directly), data-pipelines (Airflow dag nightly_core, dbt project). git, pytest, gh work in checkouts.",
        "Snowflake account halcyon-prod: warehouses TRANSFORM_WH, BI_WH, ADHOC_WH; agents with sql have read access to ACCOUNT_USAGE.",
    ]
    return s.build()


# -- L5: a research programme, no human --------------------------------------------------------------


def l5_research() -> dict:
    s = Seed(
        "l5-research",
        "A research programme with no human: lemmas proved, refuted, open, and waiting on jobs",
        "research",
        "A swarm working on a combinatorial bound, g(n) <= 3 * 2^n for n >= 3, for two months: notation and known "
        "results in kb, 40 lemmas (proved with Lean references, refuted under objection, open as offers, in progress, "
        "waiting on cluster jobs), numerical runs, failed approaches with learnings, and theorems that depend on "
        "lemmas.",
    )
    s.token("lean", "prover", ["lean"])
    s.token("num", "numerics", ["python", "numerics", "cluster"])
    s.token("rev", "reviewer", ["review", "lean"])
    for t, c, caps in (("w-lean", "prover", ["lean"]), ("w-num", "numerics", ["python", "numerics", "cluster"]),
                       ("w-rev", "reviewer", ["review", "lean"]), ("w-plan", "planner", ["planning"])):
        s.token(t, c, caps)
    rnd = random.Random(5)

    a, _ = s.session("w-plan", intent="set up the programme")
    s.create(a, "programme", "Bounding g(n)", "Goal: prove g(n) <= 3 * 2^n for all n >= 3, where g(n) is the largest "
             "size of a family of subsets of [n] with no three sets forming a sunflower with a common core. Main "
             "route: Theorem 1 from Lemmas 1-12 (induction step) and the base cases n <= 8 (computation).",
             agreements="Every claim is a node under claims/ and is settled by a decision that cites its basis: a "
             "Lean commit (lean:<commit>), a run (run:<id>), or other claims. A refuted claim gets an objection and "
             "its decision is retracted, never deleted. Cluster runs: record the job id in a checkpoint, then release "
             "blocked with waits_for external. Lean proofs live in the repo lemmas-lean.")
    s.record(a, "kb/notation", "Notation", "[n] = {1..n}; F a family of subsets; a 3-sunflower is three sets whose "
             "pairwise intersections are equal. g(n) = max |F| with no 3-sunflower.")
    s.record(a, "kb/known-results", "Known results", "g(n) <= 2^n trivially per layer... Known: g(3..8) computed "
             "exactly in the literature (Table 2 of the 2019 survey); the best general upper bound is (1.89)^n * c.")
    s.advance("2d")

    lemma_facts: list[str] = []
    states = (["proved"] * 16) + (["refuted"] * 5) + (["open"] * 9) + (["progress"] * 5) + (["job"] * 5)
    rnd.shuffle(states)
    for i, st in enumerate(states, start=1):
        path = f"claims/lemma-{i:02d}"
        title = f"Lemma {i}: " + rnd.choice([
            "every 3-sunflower-free family has a shadow of size at least |F| / {k}",
            "the link of a vertex in an extremal family is {k}-sunflower-free",
            "removing one element loses at most {k} * 2^(n-1) sets",
            "the layers above n/2 + {k} contribute at most 2^(n-{k})",
            "a weighted count over chains bounds |F| by {k} * 2^n / sqrt(n)",
        ]).format(k=rnd.randint(2, 5))
        stmt = title.split(": ", 1)[1]
        who = "w-lean" if rnd.random() < 0.7 else "w-num"
        a, _ = s.session(who, intent=title)
        if st == "proved":
            commit = "%08x" % rnd.getrandbits(32)
            lemma_facts.append(f"Lemma{i}.lean states exactly: {stmt}. It is proved: lake build passes, no sorry "
                               f"(commit {commit}).")
            s.task(a, path, title, f"{title}. Proof in lemmas-lean, file Lemma{i}.lean.", kind="claim",
                   keys=[f"lean:{commit}"], summary=f"Proved in Lean (lean:{commit}); no sorry.",
                   decision=f"{title.split(': ', 1)[1]} holds (Lean, lean:{commit}).")
        elif st == "refuted":
            v = s.create(a, path, title, f"{title}.", take=True, kind="claim")
            d = s.decide(a, path, f"{title.split(': ', 1)[1]} holds for n <= 9 (exhaustive check).",
                         evidence="observation", keep=True)
            s.close(a, path, v["t"], "done", "Checked exhaustively for n <= 9.")
            s.advance("3d")
            b, _ = s.session("w-num", intent="check lemma bounds at n = 10..12")
            run = "%06x" % rnd.getrandbits(24)
            n_bad = rnd.choice([10, 11, 12])
            lemma_facts.append(f"Lemma{i}.lean states exactly: {stmt}. Run {run} (output /scratch/runs/{run}.out) "
                               f"lists an explicit family at n = {n_bad} that breaks it; re-running the check "
                               f"confirms the counterexample. The Lean file still has a proof attempt with one sorry.")
            s.step(b, "talk", {"action": "object", "on": path, "text": rnd.choice([
                f"Counterexample at n = {n_bad} (run:{run}, exhaustive search, checked twice): the bound fails by "
                f"{rnd.randint(1, 9)} sets.",
                f"The step from n = {n_bad - 1} to n = {n_bad} uses Lemma {rnd.randint(1, 12)} outside its range; "
                f"run:{run} finds a family of size above the bound at n = {n_bad}.",
                f"Fails at n = {n_bad}: run:{run} lists an explicit family that breaks it (see the run's output).",
            ])})
            s.step(b, "talk", {"action": "retract", "entry": f"${d}", "reason": "refuted", "text": f"Fails at n = "
                               f"{n_bad} (run:{run})."})
        elif st == "open":
            lemma_facts.append(f"Lemma{i}.lean states exactly: {stmt}, with the proof `sorry`; numerically it "
                               f"holds for n <= 10.")
            s.create(a, path, title, f"{title}. Unproved; a proof attempt in Lean is welcome. Known to hold for "
                     "n <= 10 numerically.", tags=["offer"], needs=["lean"], est=rnd.choice([60, 90, 120]),
                     kind="claim")
        elif st == "progress":
            lemma_facts.append(f"Lemma{i}.lean (branch lemma-{i}) states exactly: {stmt}; the base case is proved "
                               f"and the inductive step has one sorry, the empty-core case, at line 88.")
            v = s.create(a, path, title, f"{title}.", take=True, kind="claim")
            s.checkpoint(a, v["t"], f"Lean skeleton in branch lemma-{i}; base case done; inductive step has one sorry "
                         "(the case where the core is empty)", "prove the empty-core case, likely from Lemma 3's "
                         "shadow bound")
            s.release(a, v["t"], "base case proved; one sorry left in the step", "prove the empty-core case",
                      reason="handoff", learnings="Lean's Finset.card_le_card_of_injOn does the counting; avoid "
                      "Fintype.card.")
        else:  # job
            v = s.create(a, path, title, f"{title}. Needs the exact values at n = 9 and 10 from a cluster run.",
                         take=True, needs=["numerics"], kind="claim")
            job = rnd.randint(9000, 9400)
            lemma_facts.append(f"Lemma{i}.lean states exactly: {stmt}. Cluster job {job} (exhaustive search at "
                               f"n = 9, 10) finished: /scratch/runs/{job}.out ends with 'DONE' and reports g(9) = 1218, "
                               f"g(10) = 2436, both consistent with Lemma {i}.")
            s.checkpoint(a, v["t"], f"submitted cluster job {job} (exhaustive search at n = 9, 10)",
                         f"read job {job}'s output and record the values")
            s.release(a, v["t"], f"job {job} submitted", f"when job {job} finishes, record g(9), g(10) and decide",
                      reason="blocked", waits_for={"external": f"cluster job {job}", "recheck_after": "in 1d"})
        if i % 4 == 0:
            s.advance("1d")
    a, _ = s.session("w-plan", intent="state the theorems")
    for k, (title, deps) in enumerate([("Theorem 1: g(n) <= 3 * 2^n for n >= 3", list(range(1, 13))),
                                        ("Theorem 2: g(n) / 2^n is bounded", [1]),
                                        ("Theorem 3: the extremal families are unique up to symmetry", [13, 14, 15])],
                                       start=1):
        s.create(a, f"theorems/thm-{k}", title, f"{title}. Rests on the lemmas it depends on.",
                 depends_on=[f"/claims/lemma-{d:02d}" for d in deps])
    for k, (title, learn) in enumerate([
        ("Approach: induction on n through the link", "Fails at n = 9: the link of the extremal family is not "
         "extremal, so the step loses a factor 1.2."),
        ("Approach: entropy bound", "Gives (1.97)^n, worse than the literature; abandoned."),
        ("Approach: SAT encoding for n = 11", "The encoding has 2^11 variables; the solver times out at 48 h. Use "
         "symmetry breaking first."),
    ], start=1):
        a, _ = s.session("w-plan", intent=title)
        s.task(a, f"attempts/a-{k}", title, f"{title}.", summary=f"Failed. {learn}", outcome="failed", learnings=learn)
    for k in range(1, 9):
        a, _ = s.session("w-num", intent="numerical run")
        run = "%06x" % rnd.getrandbits(24)
        s.task(a, f"runs/run-{k:02d}", f"Exhaustive search, n = {k + 2}", f"Compute g({k + 2}) exactly.",
               keys=[f"run:{run}"], summary=f"g({k + 2}) computed (run:{run}); matches the literature.")
        s.advance("1d")
    s.advance("1d")

    s.world("*", ["lean", "lake", "build", "proof", "sorry", "compile"],
            "lake build: OK, 0 errors, 2 warnings (unused variables); the file still has 1 sorry (line 88, the "
            "empty-core case).")
    s.world("*", ["job", "squeue", "sacct", "status", "output"],
            "The cluster jobs submitted by the swarm all finished: each output file ends with 'DONE', and the values "
            "are g(9) = 1218, g(10) = 2436 for the searches at n = 9, 10.")
    s.world("*", ["submit", "sbatch", "run", "search"], "Job 9512 queued; estimated 40 min.")
    s.agent("A", "lean", worker_role("a research programme", "You can write Lean 4 proofs in the lemmas-lean "
            "repository and build them."), [{"from": "event", "text": SCHEDULED}], max_turns=10)
    s.agent("B", "num", worker_role("a research programme", "You can run Python, submit cluster jobs (Slurm), and "
            "read their output."), [{"from": "event", "text": SCHEDULED}], start={"at": "30s"}, max_turns=10)
    s.agent("C", "rev", worker_role("a research programme", "You review proofs and claims: you can read the "
            "lemmas-lean repository, build it, and read run outputs."),
            [{"from": "event", "text": SCHEDULED + " You are a reviewer: check claims that others rely on."}],
            start={"at": "1m"}, max_turns=10)
    s.s["intended"] = [
        "No human. Each worker: hello with caps; hold next (or find) for work matching its caps; for blocked "
        "lemmas waiting on cluster jobs, the jobs have finished: record the values, decide, close; for lemmas in "
        "progress, continue from the checkpoint; record decisions with basis; the reviewer checks proved claims "
        "that theorems depend on and objects where needed. Refuted lemmas must not be built on.",
    ]
    s.s["facts"] = lemma_facts + [
        "Repository lemmas-lean (Lean 4 with mathlib): one file LemmaN.lean per lemma, Thm1.lean etc.; lake build works. The cluster runs Slurm; job outputs are in /scratch/runs/<job>.out.",
        "Cluster jobs 9000-9400 submitted by the swarm have finished.",
    ]
    return s.build()


# -- L6: an incident -------------------------------------------------------------------------------------


def l6_incident() -> dict:
    now = time.time()

    def hm(minutes_ago: float) -> str:
        return time.strftime("%H:%M", time.gmtime(now - minutes_ago * 60))

    began, deployed, lead = hm(120), hm(127), hm(10)
    s = Seed(
        "l6-incident",
        "A company forum during an incident: a pause, a vanished holder's open intent, duplicates, conflicting "
        "decisions",
        "incident",
        "Brightcart, an online shop, six weeks into its forum. EU checkout has thrown 500s for two hours: two "
        "incident nodes were opened for it, they carry conflicting root-cause decisions, payments is paused, and the "
        "agent that was deploying a hotfix vanished with its intent open.",
    )
    s.token("ide", "ide-agent", ["python", "repo", "k8s"])
    s.token("slack", "chat-worker", ["chat"])
    s.token("worker", "coding-agent", ["python", "repo"])
    for t, c in (("w-dir", "directory-keeper"), ("w-ide", "ide-agent"), ("w-slack", "chat-worker"),
                 ("w-bg", "coding-agent"), ("w-oncall", "oncall-agent")):
        s.token(t, c, ["python", "repo", "k8s"] if c != "chat-worker" else ["chat"])
    rnd = random.Random(6)

    a, _ = s.session("w-dir", intent="set up Brightcart's directory")
    ppl = {}
    for slug, name, mail, body in (
        ("noor-hassan", "Noor Hassan", "noor", "SRE; on call this week."),
        ("tomas-silva", "Tomás Silva", "tomas", "Support lead."),
        ("hugo-moreau", "Hugo Moreau", "hugo", "CTO."),
        ("lin-zhou", "Lin Zhou", "lin", "Payments engineer; owns the VAT service."),
        ("kofi-mensah", "Kofi Mensah", "kofi", "Backend engineer (checkout)."),
    ):
        ppl[slug] = s.person(a, slug, name, [f"mail:{mail}@brightcart.example"], body)
    s.create(a, "payments", "Payments", "Checkout payments and the VAT service (vat-svc).", owners=[ppl["lin-zhou"]])
    s.create(a, "checkout", "Checkout", "The checkout service.", owners=[ppl["kofi-mensah"]])
    s.create(a, "incidents", "Incidents", "One node per incident, keyed incident:INC-<n>.",
             agreements="One node per incident, keyed incident:INC-<n> from the incident tool. The incident lead's "
             "agent holds it; others post findings with talk say to holder:. Root cause is a decision with "
             "evidence; a later finding supersedes it, never silently replaces it.")
    s.record(a, "kb/vat-svc", "The VAT service", "vat-svc computes VAT for EU orders; checkout calls it with a 2 s "
             "timeout. Deploys go through Argo CD (app vat-svc).")
    s.advance("3d")
    for k in range(18):
        a, _ = s.session(rnd.choice(["w-ide", "w-bg"]), intent="work")
        area = rnd.choice(["payments", "checkout"])
        title = rnd.choice(["Fix flaky {a} test", "Add metrics to {a}", "Upgrade {a} dependencies",
                            "Speed up {a} builds", "Clean up {a} feature flags"]).format(a=area)
        s.task(a, f"{area}/task-{k}", title, f"{title}.", summary="Done; PR merged.")
        if k % 3 == 0:
            s.advance("2d")
    for slug, title, body in (("checkout/guest-addresses", "Save guest addresses", "Let guests save an address."),
                              ("payments/klarna-retry", "Retry Klarna authorisations once", "One retry on timeout.")):
        a, _ = s.session("w-bg", intent="file work")
        s.create(a, slug, title, body, tags=["offer"], est=45)
    s.advance("20d")

    # The incident, two hours ago.
    a, _ = s.session("w-oncall", principal="mail:noor@brightcart.example", intent="incident INC-311")
    v = s.create(a, "incidents/inc-311-eu-checkout", "INC-311: EU checkout returns 500s", "EU checkout requests fail "
                 f"with HTTP 500 since {began} UTC (about 18 % of EU checkouts).", keys=["incident:INC-311"], take=True)
    s.checkpoint(a, v["t"], f"500s are from checkout's call to vat-svc timing out (2 s); started 7 minutes after "
                 f"vat-svc v2.14.0 was deployed at {deployed} UTC", "confirm with vat-svc's latency and the deploy diff")
    s.decide(a, "incidents/inc-311-eu-checkout", f"Root cause: vat-svc v2.14.0 (deployed {deployed} UTC) made VAT lookups slow "
             "(p99 3.1 s > checkout's 2 s timeout).", evidence="inference")
    s.release(a, v["t"], "found that vat-svc timeouts cause the 500s, likely v2.14.0", "roll back or hotfix vat-svc; "
              "keep support updated", reason="handoff")
    s.advance("20m")
    b, _ = s.session("w-slack", intent="answer #incidents")
    s.thread_request(b, "incidents/checkout-errors-eu", "Checkout errors for EU customers", "incidents/T7001", "M1",
                     "Tomás Silva", f"Support is getting lots of 'payment failed' tickets from EU customers since {began} UTC. "
                     "Is this known? What should we tell customers?", ack="Looking into it, will update here.",
                     take=False)
    s.advance("10m")
    c, _ = s.session("w-ide", principal="mail:lin@brightcart.example", intent="hotfix vat-svc")
    t = s.create(c, "payments/vat-hotfix", "Hotfix vat-svc lookup latency", "vat-svc v2.14.0 dropped the rate "
                 "cache; restore it in v2.14.1.", keys=["issue:vat-svc#77"], take=True)["t"]
    s.checkpoint(c, t, "v2.14.1 built with the rate cache restored; staging p99 40 ms",
                 "deploy v2.14.1 to prod through Argo CD", intent="deploy vat-svc v2.14.1 to prod (Argo CD sync)")
    s.advance("10m")
    d, _ = s.session("w-bg", intent="look at INC-311")
    s.decide(d, "incidents/inc-311-eu-checkout", "Root cause: the checkout DB connection pool is exhausted (pool 50, "
             f"all busy at {hm(117)} UTC), so requests time out.", evidence="observation")
    s.step(d, "control", {"action": "pause", "scope": "payments", "reason": "Freeze for INC-311: no payments deploys "
                          "until the incident lead says so."})
    s.advance("50m")

    s.human("noor", "Noor Hassan", "mail:noor@brightcart.example",
            [{"when": "asked", "match": ["roll", "deploy", "hotfix", "v2.14.1", "pause", "freeze"],
              "say": "Yes, go ahead: deploy v2.14.1 if it's not already out. I lift the payments freeze for that."},
             {"when": "asked", "say": "Use your judgement, keep me posted."}])
    s.people("tomas", "Tomás Silva (support lead)", "thread T7001 of #incidents (chat:incidents/T7001)",
             [{"on": "post", "when": "always", "say": "Thanks. Can I promise customers a time?", "once": True}])
    s.world("A", ["argo", "deploy", "version", "vat-svc", "status", "rollout", "history"],
            f"Argo CD app vat-svc: prod runs v2.14.0 (synced {deployed} UTC); v2.14.1 is built but was never synced: "
            "there was no sync attempt after that.")
    s.world("A", ["latency", "p99", "grafana", "metrics", "pool", "connection"],
            f"vat-svc p99 3.1 s since {deployed} UTC (was 45 ms). Checkout DB pool: 50 of 50 busy, all waiting on vat-svc calls; "
            "pool exhaustion is a symptom, not the cause.")
    s.world("A", ["sync", "roll out", "rollback", "roll back", "apply"],
            "Argo CD synced vat-svc v2.14.1 to prod: rollout done in 3 min; vat-svc p99 42 ms; EU checkout 500s "
            "dropped to 0 within 2 minutes.")
    s.world("C", ["test", "build"], "Tests pass.")
    s.agent(
        "A",
        "ide",
        ide_role("Noor Hassan, the SRE on call at Brightcart, an online shop", "mail:noor@brightcart.example",
                 "You can talk to her in the IDE chat, read dashboards and Argo CD, deploy through Argo CD, and run "
                 "kubectl."),
        [{"from": "human", "text": f"I just got paged: EU checkout is throwing 500s, apparently since about {began} UTC. I'm "
          "the incident lead now. Get me the picture: what do we know, who's doing what, what changed? Then let's fix "
          "it."}],
        human="noor",
        max_turns=10,
    )
    s.agent(
        "B",
        "slack",
        chat_role("Brightcart", "#incidents", "You can read and post in the thread and read dashboards and the status "
                  "page."),
        [{"from": "event", "text": thread_event("#incidents", "T7001", "M7", "Tomás Silva (support lead)",
                                                "any update for customers? the support queue is exploding, 140 "
                                                "tickets now")}],
        conversation="tomas",
        start={"at": "1m"},
        max_turns=10,
    )
    s.agent(
        "C",
        "worker",
        worker_role("Brightcart", "You can run Python, tests, and builds, and push to the shop's repositories."),
        [{"from": "event", "text": SCHEDULED}],
        start={"at": "2m"},
        max_turns=10,
    )
    s.s["intended"] = [
        "A (Noor's IDE agent, incident lead): hello with principal and from_user; find the INC-311 node(s): see the "
        "duplicate (link duplicate or merge), the conflicting root-cause decisions (supersede the wrong one with "
        "evidence), the vanished holder's open intent on payments/vat-hotfix (check Argo: v2.14.1 never synced), "
        "the payments pause; ask Noor before deploying while paused; deploy; record; report to Noor.",
        "B (chat integration in #incidents): Tomás's new message is a follow-up on the existing request in thread "
        "T7001 (recorded on incidents/checkout-errors-eu, acknowledged, never answered): record it, answer with what "
        "is known (said before posting, confirm), do not promise a time nobody gave.",
        "C (worker): notice the pause on payments and do not start paused work; take unpaused work or do nothing.",
    ]
    s.s["facts"] = [
        "Repositories: vat-svc (Go) and checkout (Python). Argo CD apps vat-svc and checkout; Grafana dashboards checkout-overview and vat-svc; kubectl has read and write access to prod. v2.14.1 of vat-svc is built (image vat-svc:2.14.1) but not deployed.",
        f"The incident tool lists INC-311 (sev2, EU checkout 500s since {began} UTC; lead: Noor Hassan since {lead} UTC).",
        "Thread T7001 of #incidents so far: M1 from Tomás Silva (payment-failed tickets from EU customers; is it known, "
        "what to tell customers), then the swarm's reply 'Looking into it, will update here.'; nothing else until M7.",
    ]
    return s.build()


# -- L7: parallel work ---------------------------------------------------------------------------------

PORTAL_TASKS = [
    # slug, title, needs, est, depends_on (slugs), touches theme.ts
    ("tokens", "Adopt the v2 colour and spacing tokens in theme.ts", ["web"], 30, [], True),
    ("button", "Migrate Button and IconButton to @lumen/ui v2", ["web"], 30, ["tokens"], False),
    ("forms", "Migrate the form fields (Input, Select, Checkbox) to v2", ["web"], 45, ["button"], False),
    ("nav", "Migrate the top navigation bar to v2", ["web"], 40, [], False),
    ("modal", "Migrate Modal and Drawer to v2", ["web"], 40, [], False),
    ("table", "Migrate the appointments table to the v2 DataTable", ["web"], 60, [], False),
    ("toast", "Replace the toast library with v2 Toast", ["web"], 30, [], False),
    ("dark", "Dark-mode variables for the v2 tokens in theme.ts", ["web"], 30, [], True),
    ("a11y-focus", "Visible focus rings on every v2 control", ["web", "a11y"], 40, [], False),
    ("icons", "Swap the icon set for the v2 icons", ["web"], 30, [], False),
    ("storybook", "Storybook stories for the migrated components", ["web"], 45, [], False),
    ("e2e", "Update the Playwright selectors broken by the migration", ["web", "e2e"], 45, [], False),
]


def l7_parallel_rush() -> dict:
    s = Seed(
        "l7-parallel-rush",
        "Parallel work: six workers arrive at once at a backlog of twelve tasks",
        "parallel",
        "Lumen Health's web team broke the patient portal's move to the v2 design system into twelve offers: two "
        "depend on others, two touch the same file (theme.ts), two need extra caps. Six scheduled workers arrive "
        "within a minute. Nobody coordinates them but the forum.",
    )
    for k in range(6):
        s.token(f"w{k}", "coding-agent", ["web", "repo", "ci"] + (["a11y"] if k % 3 == 0 else []))
    s.token("w-lead", "ide-agent", ["web", "repo"])
    a, _ = s.session("w-lead", intent="split the v2 migration into tasks")
    s.create(a, "portal", "Patient portal", "The patient portal web app (repository portal-web, React and TypeScript).")
    s.create(a, "portal/v2-migration", "Move the patient portal to the v2 design system", "Epic: every screen of "
             "portal-web on @lumen/ui v2 by the end of the sprint. Each task is an offer under this node; one PR per "
             "task against main.", capacity=0,
             agreements="One PR per task, branch v2/<task-slug>. Rebase on main before opening the PR. theme.ts is "
             "shared: whoever changes it second rebases. Close a task when its PR is merged.")
    for slug, title, needs, est, deps, _theme in PORTAL_TASKS:
        kw = {"depends_on": [f"/portal/v2-migration/{d}" for d in deps]} if deps else {}
        s.create(a, f"portal/v2-migration/{slug}", title, f"{title}. Run the unit tests and open a PR.",
                 tags=["offer"], needs=needs, est=est, **kw)
    s.advance("1h")
    s.s["facts"] = [
        "Repository portal-web (React, TypeScript, Vite, Vitest, Playwright); @lumen/ui v2 is installed. Each worker "
        "has its own checkout of main and may create branches; git, npm, and gh work there.",
        "Every task's change is small: replacing imports and props, then npm test passes (about 640 tests). A PR's CI "
        "takes 4 minutes and is green; the release bot merges a green PR within a minute.",
        "src/styles/theme.ts is changed by both the tokens task and the dark-mode task: whichever PR merges second "
        "shows a merge conflict in theme.ts until its branch is rebased on main; after the rebase, tests pass.",
        "The button task's code imports the v2 tokens: before the tokens PR is merged, its tests fail with "
        "'Cannot find export tokens.space4'. The forms task likewise needs the button task merged first.",
        "Playwright needs the e2e cap's runner image; without it, npx playwright test fails to start.",
    ]
    for k in range(6):
        s.agent(
            "ABCDEF"[k],
            f"w{k}",
            worker_role("Lumen Health's web team", "You can edit and test the portal-web code (React, TypeScript), "
                        "push branches, and open pull requests."),
            [{"from": "event", "text": SCHEDULED}],
            start={"at": f"{k * 10}s"},
            max_turns=10,
        )
    s.s["intended"] = [
        "Each worker: hello; hold next; take a different offer than the others (next and holds keep them apart); "
        "checkpoint; intent before the push and PR; close when merged; then hold next again while time allows.",
        "Dependencies: button waits for tokens, forms for button; nobody should start them early, or if taken, "
        "release blocked with a wait on the dependency.",
        "theme.ts: tokens and dark both change it; the second to merge rebases, as the agreements say.",
        "a11y-focus needs a11y (only w0 and w3 have it); e2e needs a cap nobody has: it should stay offered, not "
        "half-done.",
        "No two workers on one task; no work lost; commitments and holds clean at the end.",
    ]
    return s.build()


SEEDS = {
    "l1-day-one": l1_day_one,
    "l2-solo-month": l2_solo_month,
    "l3-company-quarter": l3_company_quarter,
    "l4-mature-year": l4_mature_year,
    "l5-research": l5_research,
    "l6-incident": l6_incident,
    "l7-parallel-rush": l7_parallel_rush,
}


def load(sid: str) -> dict:
    if sid not in SEEDS:
        matches = [k for k in SEEDS if k.startswith(sid)]
        if len(matches) != 1:
            raise KeyError(f"no seed {sid!r}: {sorted(SEEDS)}")
        sid = matches[0]
    return SEEDS[sid]()
