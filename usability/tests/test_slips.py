"""The slip counter, rule by rule, on synthetic runs: each rule with a slip and with the behaviour
§20.9 asks for (`python3 -m unittest discover -s usability/tests`)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from harness import history as HI  # noqa: E402
from harness import packages as PK  # noqa: E402
from harness import scenario as SC  # noqa: E402
from harness import slips as SL  # noqa: E402
from synth import MIN, SEC, T0, Synth  # noqa: E402

PEOPLE = HI.PEOPLE
BAD_PARAMS = "the parameters break a rule of the constitution, or this server does not do this yet"


def what(result: dict, rule: str) -> list[str]:
    return [x.get("what", "") for x in result["details"][rule]]


class RuleE(unittest.TestCase):
    def run_with(
        self, second_ok: bool, tool="node", action="close", error="not_found", hint="path: no such node", built=None
    ):
        s = Synth()
        if built is not None:
            s.meta["built_packages"] = sorted(built)
        s.session(2)
        s.agent("W", [2])
        s.call("W", 2, tool, {"action": action}, T0 + SEC, error=error, hint=hint)
        if second_ok:
            s.call("W", 2, tool, {"action": action}, T0 + 2 * SEC)
        else:
            s.call("W", 2, tool, {"action": action}, T0 + 2 * SEC, error=error, hint=hint)
        s.call("W", 2, "session", {"action": "status"}, T0 + 3 * SEC)
        return s.count()

    def test_the_same_mistake_again_is_a_slip(self):
        self.assertEqual(what(self.run_with(False), "E"), ["the same mistake again"])

    def test_a_remedied_refusal_is_not(self):
        self.assertEqual(self.run_with(True)["rules"]["E"], 0)

    def test_a_command_the_core_lacks_is_not_a_slip_but_a_real_bad_params_is(self):
        r = self.run_with(False, "talk", "said", "invalid", BAD_PARAMS, built=PK.BUILT_BEFORE_D5)
        self.assertEqual(r["rules"]["E"], 0)
        self.assertEqual(r["refusals_not_built"], {"talk said": 2})
        # Once its package has landed, the same refusal is a real BadParams.
        r = self.run_with(False, "talk", "said", "invalid", BAD_PARAMS)
        self.assertEqual(r["rules"]["E"], 1)
        self.assertEqual(r["refusals_not_built"], {})
        r = self.run_with(False, "hold", "take", "invalid", BAD_PARAMS)
        self.assertEqual(r["rules"]["E"], 1)
        self.assertEqual(r["refusals"], {"invalid": 2})


class Rule1(unittest.TestCase):
    def test_a_line_never_logged(self):
        for logged, want in ((False, 1), (True, 0)):
            s = Synth()
            s.session(3)
            s.agent("H", [3], human="h")
            s.meta["deliveries"].append({"agent": "H", "from": "human", "text": "Please look at the export."})
            if logged:
                s.tx(3, T0 + SEC)
                s.append(1, 3, T0 + SEC, {"FromUser": {"principal": 0}}, "Please look at the export.")
            self.assertEqual(s.count()["rules"]["1"], want)


class Rule2(unittest.TestCase):
    def base(self, killed=False):
        s = Synth()
        s.session(2)
        s.node(10, "conv", keys=["chat:c/t"])
        s.agent("W", [2], killed_at=T0 + 3 * SEC if killed else None)
        s.tx(2, T0 + SEC)
        e = s.append(10, 2, T0 + SEC, {"Said": {"conv": "chat:c/t", "kind": "Reply", "corrects": None}}, "on it")
        s.call("W", 2, "talk", {"action": "said"}, T0 + SEC - 100)
        return s, e

    def test_a_said_never_confirmed_is_a_slip(self):
        s, _ = self.base()
        s.call("W", 2, "hold", {"action": "next"}, T0 + 2 * SEC)
        self.assertEqual(s.count()["rules"]["2"], 1)

    def test_a_confirmed_said_is_not(self):
        s, e = self.base()
        s.tx(2, T0 + 2 * SEC)
        s.ev(T0 + 2 * SEC, "SaidConfirmed", {"said": e, "via": "Explicit", "link": "https://chat/x"})
        self.assertEqual(s.count()["rules"]["2"], 0)

    def test_a_said_listed_in_unsent_is_not(self):
        s, e = self.base()
        s.call("W", 2, "hold", {"action": "release", "unsent": [f"e{e}"], "session": "s-2"}, T0 + 2 * SEC)
        r = s.count()
        self.assertEqual(r["rules"]["2"], 0)
        self.assertEqual(len(r["apart"]["said_listed_unsent"]), 1)

    def test_a_said_by_the_call_at_an_end_point_is_reported_apart(self):
        s, _ = self.base(killed=True)
        r = s.count()
        self.assertEqual(r["rules"]["2"], 0)
        self.assertEqual(len(r["apart"]["said_at_end_point"]), 1)


class Rule3(unittest.TestCase):
    def expired(self, checkpoint: bool, killed_at=None):
        s = Synth()
        s.session(2)
        s.node(11, "long")
        s.agent("W", [2], killed_at=killed_at)
        s.tx(2, T0 + SEC)
        s.take(11, 1, 2, T0 + SEC, lease=2 * MIN)
        s.call("W", 2, "hold", {"action": "take"}, T0 + SEC)
        if checkpoint:
            s.tx(2, T0 + 30 * SEC)
            s.checkpoint(11, 1, 2, T0 + 30 * SEC, lease=3 * MIN)
            s.call("W", 2, "hold", {"action": "checkpoint"}, T0 + 30 * SEC)
        s.call("W", 2, "view", {"action": "node"}, T0 + 40 * SEC)
        s.expire(11, 1, T0 + 10 * MIN)
        return s.count()

    def test_an_expiry_after_a_checkpoint_while_the_session_ran_is_a_slip(self):
        r = self.expired(checkpoint=True)
        self.assertEqual(what(r, "3"), ["expired after its last checkpoint"])

    def test_an_expiry_without_progress_is_a_slip(self):
        self.assertEqual(what(self.expired(checkpoint=False), "3"), ["expired without progress"])

    def test_an_end_point_before_the_checkpoint_was_due_is_apart(self):
        # Lease 2 min from T0+1 s: due at T0+1 s+1 min. Killed at T0+40 s, before it was due.
        r = self.expired(checkpoint=False, killed_at=T0 + 40 * SEC)
        self.assertEqual(r["rules"]["3"], 0)
        self.assertEqual(len(r["apart"]["expired_after_end_point"]), 1)

    def test_an_end_point_after_the_checkpoint_was_due_is_a_slip(self):
        r = self.expired(checkpoint=False, killed_at=T0 + 3 * MIN)
        self.assertEqual(r["rules"]["3"], 1)

    def intent(self, done: bool):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2)
        s.node(10, "backfill")
        s.tx(1, T0 + SEC)
        s.take(10, 1, 1, T0 + SEC)
        e = s.checkpoint(10, 1, 1, T0 + 2 * SEC)
        s.ev(T0 + 2 * SEC, "IntentOpened", {"node": 10, "intent": {"eid": e, "session": 1}})
        s.expire(10, 1, T0 + 5 * MIN)
        s.agent("W", [2])
        s.tx(2, T0 + 6 * MIN)
        s.take(10, 2, 2, T0 + 6 * MIN)
        s.call("W", 2, "hold", {"action": "take"}, T0 + 6 * MIN)
        s.tx(2, T0 + 7 * MIN)
        if done:
            s.ev(T0 + 7 * MIN, "IntentClosed", {"node": 10, "end": {"Done": {"entry": 99}}})
        s.release(10, 2, 2, T0 + 7 * MIN)
        s.call("W", 2, "hold", {"action": "release"}, T0 + 7 * MIN)
        return s.count()

    def test_another_sessions_intent_left_by_the_next_holder_is_its_slip(self):
        r = self.intent(done=False)
        self.assertEqual(
            [(x["agent"], x["what"]) for x in r["details"]["3"]],
            [("W", "another session's open intent left unchecked")],
        )

    def test_an_intent_closed_with_intent_done_is_not(self):
        self.assertEqual(self.intent(done=True)["rules"]["3"], 0)


class Rule4(unittest.TestCase):
    def run_with(self, shown_seq_after: bool, on_conversation_node: bool = False):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2)
        s.node(9, "thread", keys=["chat:c/t"])
        s.node(10, "work", parent=9)
        s.agent("W", [2])
        s.tx(2, T0 + SEC)
        s.take(10, 1, 2, T0 + SEC)
        s.ev(T0 + SEC, "Shown", {"session": 2, "node": 10, "seq": 0})
        s.tx(1, T0 + 2 * SEC)
        where = 9 if on_conversation_node else 10
        e = s.append(where, 1, T0 + 2 * SEC, {"Heard": {"conv": "chat:c/t", "answers": None}}, "actually, wait")
        if shown_seq_after:
            s.tx(2, T0 + 3 * SEC)
            s.ev(T0 + 3 * SEC, "Shown", {"session": 2, "node": where, "seq": e})
        s.tx(2, T0 + 4 * SEC)
        s.close(10, 2, T0 + 4 * SEC, fence=1)
        s.call("W", 2, "node", {"action": "close"}, T0 + 4 * SEC)
        return s.count()

    def test_a_close_without_the_newest_message_is_a_slip(self):
        self.assertEqual(self.run_with(False)["rules"]["4"], 1)

    def test_a_close_after_reading_it_is_not(self):
        self.assertEqual(self.run_with(True)["rules"]["4"], 0)

    def test_the_conversation_node_counts_too(self):
        self.assertEqual(self.run_with(False, on_conversation_node=True)["rules"]["4"], 1)
        self.assertEqual(self.run_with(True, on_conversation_node=True)["rules"]["4"], 0)


class Rule5(unittest.TestCase):
    def run_with(self, router):
        s = Synth()
        s.session(1, cls="seed")
        s.session(3)
        s.agent("H", [3], human="h")
        s.tx(3, T0 + SEC)
        e = s.append(1, 3, T0 + SEC, {"FromUser": {"principal": 0}}, "fix the export")
        if router is not None:
            s.tx(router, T0 + 2 * SEC)
            s.ev(
                T0 + 2 * SEC,
                "EntryRouted",
                {"entry": e, "routing": {"Request": {"node": 8, "request": 9}}, "reason": None},
            )
        return s.count()

    def test_routed_by_its_own_session_is_not_a_slip(self):
        self.assertEqual(self.run_with(3)["rules"]["5"], 0)

    def test_routed_by_a_stranger_or_never_is_a_slip(self):
        self.assertEqual(what(self.run_with(1), "5"), ["routed by another session"])
        self.assertEqual(what(self.run_with(None), "5"), ["never routed"])


class Rules6and7(unittest.TestCase):
    def closed(self, decided: bool, answered: bool, stranger_settles: bool):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2)
        s.node(12, "req-node")
        s.tx(1, T0 + SEC)
        r = s.request(12, 1, T0 + SEC)
        s.agent("W", [2])
        s.tx(2, T0 + 2 * SEC)
        s.take(12, 1, 2, T0 + 2 * SEC)
        s.call("W", 2, "hold", {"action": "take"}, T0 + 2 * SEC)
        s.tx(2, T0 + 3 * SEC)
        if decided:
            s.decide(12, 2, T0 + 3 * SEC, settles=[r])
        if answered:
            s.ev(T0 + 3 * SEC, "RequestAnswered", {"request": r, "by": 77})
        if decided and answered:
            s.ev(T0 + 3 * SEC, "RequestSettled", {"request": r, "decision": 1, "answer": 77})
        s.tx(2, T0 + 4 * SEC)
        s.close(12, 2, T0 + 4 * SEC, fence=1)
        s.call("W", 2, "node", {"action": "close"}, T0 + 4 * SEC)
        if stranger_settles:
            s.tx(1, T0 + 20 * MIN)
            d = s.decide(12, 1, T0 + 20 * MIN, settles=[r])
            s.ev(T0 + 20 * MIN, "RequestAnswered", {"request": r, "by": 78})
            s.ev(T0 + 20 * MIN, "RequestSettled", {"request": r, "decision": d, "answer": 78})
        return s.count()

    def test_a_close_that_leaves_the_request_to_a_stranger_is_judged_at_the_close(self):
        r = self.closed(decided=False, answered=False, stranger_settles=True)
        self.assertEqual(what(r, "7"), ["closed with neither a decision nor an answer"])

    def test_a_close_that_settled_first_is_not(self):
        r = self.closed(decided=True, answered=True, stranger_settles=False)
        self.assertEqual((r["rules"]["6"], r["rules"]["7"]), (0, 0))

    def test_a_close_owing_a_reply_is_rule_6(self):
        r = self.closed(decided=True, answered=False, stranger_settles=False)
        self.assertEqual(what(r, "6"), ["closed decided, without a reply"])
        self.assertEqual(r["rules"]["7"], 0)

    def test_a_request_owed_at_the_end_on_a_node_an_agent_held(self):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2)
        s.node(12, "req-node")
        s.tx(1, T0 + SEC)
        r = s.request(12, 1, T0 + SEC)
        s.agent("W", [2])
        s.tx(2, T0 + 2 * SEC)
        s.take(12, 1, 2, T0 + 2 * SEC)
        s.decide(12, 2, T0 + 3 * SEC, settles=[r])
        s.release(12, 1, 2, T0 + 4 * SEC)
        s.call("W", 2, "hold", {"action": "release"}, T0 + 4 * SEC)
        self.assertEqual(what(s.count(), "6"), ["decided, never answered"])

    def reports(self, kind: str, principal: int, alias: bool = False, through_entry: bool = False):
        s = Synth()
        s.session(1, cls="seed")
        s.principal(1, "mail:dana@example.org")
        s.principal(2, "chat:dana")
        if alias:
            s.node(20, "dana", parent=PEOPLE, capacity=0, keys=["person:mail:dana@example.org", "person:chat:dana"])
        s.session(3, principal=1)
        s.agent("H", [3], human="h")
        s.node(12, "req-node")
        s.tx(3, T0 + SEC)
        u = s.append(1, 3, T0 + SEC, {"FromUser": {"principal": 1}}, "fix it")
        r = s.request(12, 3, T0 + SEC, requester={"User": 1}, from_user=u, conv=f"user:e{u}")
        s.tx(3, T0 + 2 * SEC)
        s.append(
            1,
            3,
            T0 + 2 * SEC,
            {"ToUser": {"kind": kind, "verbatim": True, "re": [u if through_entry else r], "principal": principal}},
        )
        s.call("H", 3, "talk", {"action": "to_user"}, T0 + 2 * SEC)
        return s.count()

    def test_a_result_to_someone_else_is_a_slip_but_not_to_an_alias(self):
        self.assertIn("result to someone else", what(self.reports("Result", 2), "6"))
        self.assertNotIn("result to someone else", what(self.reports("Result", 2, alias=True), "6"))
        self.assertNotIn("result to someone else", what(self.reports("Result", 1), "6"))
        self.assertIn("result to someone else", what(self.reports("Result", 2, through_entry=True), "6"))

    def test_a_report_of_the_wrong_kind(self):
        self.assertIn("report of kind ack, never a result", what(self.reports("Ack", 1), "6"))


class Rule8(unittest.TestCase):
    def test_a_series_closed_by_an_agent(self):
        s = Synth()
        s.session(2)
        s.agent("W", [2])
        s.node(10, "weekly", role={"Series": {"repeat": {}, "last_missed": False}})
        s.tx(2, T0 + SEC)
        s.close(10, 2, T0 + SEC)
        s.call("W", 2, "node", {"action": "close"}, T0 + SEC)
        self.assertEqual(what(s.count(), "8"), ["series closed"])

    def reopened(self, settle: bool, close_again: bool):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2)
        s.node(12, "answered-late")
        s.tx(1, T0 + SEC)
        r = s.request(12, 1, T0 + SEC)
        s.close(12, 1, T0 + 2 * SEC)
        s.agent("W", [2])
        s.tx(2, T0 + MIN)
        s.ev(T0 + MIN, "NodeReopened", {"node": 12})
        s.take(12, 1, 2, T0 + MIN)
        s.call("W", 2, "hold", {"action": "take"}, T0 + MIN)
        s.tx(2, T0 + 2 * MIN)
        if settle:
            d = s.decide(12, 2, T0 + 2 * MIN, settles=[r])
            s.ev(T0 + 2 * MIN, "RequestAnswered", {"request": r, "by": 90})
            s.ev(T0 + 2 * MIN, "RequestSettled", {"request": r, "decision": d, "answer": 90})
        if close_again:
            s.close(12, 2, T0 + 3 * MIN, fence=1)
            s.call("W", 2, "node", {"action": "close"}, T0 + 3 * MIN)
        else:
            s.release(12, 1, 2, T0 + 3 * MIN)
            s.call("W", 2, "hold", {"action": "release"}, T0 + 3 * MIN)
        return s.count()

    def test_a_reopened_node_left_open_once_its_flag_is_gone(self):
        self.assertIn("reopened by a take, left open with no flag left", what(self.reopened(True, False), "8"))

    def test_a_reopened_node_closed_again_or_still_flagged_is_not(self):
        self.assertEqual(self.reopened(True, True)["rules"]["8"], 0)
        self.assertNotIn("reopened by a take, left open with no flag left", what(self.reopened(False, False), "8"))

    def test_a_finished_node_left_held_to_expire(self):
        s = Synth()
        s.session(2)
        s.node(12, "decided")
        s.agent("W", [2])
        s.tx(2, T0 + SEC)
        s.take(12, 1, 2, T0 + SEC)
        s.decide(12, 2, T0 + 2 * SEC)
        s.call("W", 2, "talk", {"action": "decide"}, T0 + 2 * SEC)
        s.expire(12, 1, T0 + 3 * MIN)
        self.assertEqual(
            what(s.count(), "8"), ["finished node left held, which expired (a decision recorded during the hold)"]
        )


class Rule9(unittest.TestCase):
    def run_with(self, **kw):
        s = Synth()
        s.session(2)
        s.agent("W", [2])
        s.tx(2, T0 + SEC)
        s.node(30, "split-off", by=2, **kw)
        s.call("W", 2, "node", {"action": "create", "take": False}, T0 + SEC)
        return s

    def test_a_split_off_node_nothing_offers(self):
        self.assertEqual(self.run_with().count()["rules"]["9"], 1)

    def test_an_offered_node_a_branch_or_a_flagged_node_is_not(self):
        self.assertEqual(self.run_with(tags=["offer"]).count()["rules"]["9"], 0)
        s = self.run_with()
        s.node(31, "child", parent=30, by=2, tags=["offer"])
        self.assertEqual(s.count()["rules"]["9"], 0)
        s = self.run_with()
        s.request(30, 2, T0 + 2 * SEC)
        self.assertEqual(s.count()["rules"]["9"], 0)


class Rule11(unittest.TestCase):
    def principal_note(self, read: bool):
        s = Synth()
        s.session(1, cls="seed")
        s.principal(1, "mail:alice@example.org")
        s.session(3, principal=1)
        s.agent("H", [3], human="h")
        s.call("H", 3, "session", {"action": "hello"}, T0 + SEC)
        s.tx(1, T0 + 2 * SEC)
        n = s.append(
            8,
            1,
            T0 + 2 * SEC,
            {
                "Note": {
                    "to": {"Principal": 1},
                    "until": T0 + 45 * MIN,
                    "recipients": [],
                    "pending_holder": False,
                    "expects": None,
                    "reply_to": None,
                }
            },
            "Alice: the export is fixed",
        )
        s.call("H", 3, "view", {"action": "node"}, T0 + 3 * SEC)
        if read:
            s.tx(3, T0 + 4 * SEC)
            s.ev(T0 + 4 * SEC, "InboxRead", {"session": 3, "notes": [n]})
        return s.count()

    def test_a_principal_note_never_read_is_a_slip(self):
        self.assertEqual(what(self.principal_note(False), "11"), ["note never read"])
        self.assertEqual(self.principal_note(True)["rules"]["11"], 0)

    def test_a_holders_note_reaches_a_later_holder_in_the_branch(self):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2)
        s.node(9, "programme")
        s.node(10, "lemma", parent=9)
        s.tx(1, T0 + SEC)
        s.append(
            9,
            1,
            T0 + SEC,
            {
                "Note": {
                    "to": {"Holders": 9},
                    "until": T0 + 45 * MIN,
                    "recipients": [],
                    "pending_holder": False,
                    "expects": None,
                    "reply_to": None,
                }
            },
            "Lemma 17 is refuted",
        )
        s.agent("W", [2])
        s.tx(2, T0 + MIN)
        s.take(10, 1, 2, T0 + MIN)
        s.call("W", 2, "hold", {"action": "take"}, T0 + MIN)
        self.assertEqual(what(s.count(), "11"), ["note never read"])

    def test_an_agent_question_left_unanswered(self):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2)
        s.agent("W", [2])
        s.call("W", 2, "session", {"action": "hello"}, T0 + SEC)
        s.tx(1, T0 + 2 * SEC)
        q = s.append(
            8,
            1,
            T0 + 2 * SEC,
            {
                "Question": {
                    "Agent": {"to": {"Session": 2}, "until": T0 + 45 * MIN, "recipients": [2], "pending_holder": False}
                }
            },
            "Which branch is the release on?",
        )
        s.tx(2, T0 + 3 * SEC)
        s.ev(T0 + 3 * SEC, "InboxRead", {"session": 2, "notes": [q]})
        s.call("W", 2, "talk", {"action": "wait"}, T0 + 3 * SEC)
        self.assertEqual(what(s.count(), "11"), ["a colleague's question left unanswered"])
        s.tx(2, T0 + 4 * SEC)
        s.append(8, 2, T0 + 4 * SEC, {"Answer": {"question": q, "recipient": 1, "until": T0 + 45 * MIN}}, "main")
        s.ev(T0 + 4 * SEC, "AqAnswered", {"q": q, "by": s.eid})
        self.assertEqual(s.count()["rules"]["11"], 0)


class Rule12(unittest.TestCase):
    def question(self, asked: bool, live: bool = True, ends="UqDefaulted"):
        s = Synth()
        s.session(1, cls="seed")
        s.session(3)  # names no principal: it serves `user` by the from_user entry it records
        s.agent("H", [3], human="h")
        s.tx(3, T0 + SEC)
        s.append(1, 3, T0 + SEC, {"FromUser": {"principal": 0}}, "look at the wait node")
        s.call("H", 3, "talk", {"action": "from_user"}, T0 + SEC)
        s.tx(1, T0 + 2 * SEC)
        q = s.append(
            2,
            1,
            T0 + 2 * SEC,
            {
                "Question": {
                    "User": {"source": "Agent", "about": None, "to": [0], "due": T0 + 15 * MIN, "has_default": True}
                }
            },
            "Keep the oldest row?",
        )
        if live:
            s.call("H", 3, "view", {"action": "node"}, T0 + 3 * SEC)
        if asked:
            s.tx(3, T0 + 4 * SEC)
            s.append(1, 3, T0 + 4 * SEC, {"ToUser": {"kind": "Question", "verbatim": True, "re": [], "principal": 0}})
            s.call("H", 3, "talk", {"action": "to_user"}, T0 + 4 * SEC)
        s.tx(None, T0 + 15 * MIN, "settle")
        s.ev(T0 + 15 * MIN, ends, {"q": q})
        return s.count()

    def test_a_question_that_defaults_unasked_while_the_human_is_served(self):
        self.assertEqual(what(self.question(asked=False), "12"), ["defaulted"])

    def test_a_question_asked_or_arriving_after_the_session_is_not(self):
        self.assertEqual(self.question(asked=True)["rules"]["12"], 0)
        self.assertEqual(self.question(asked=False, live=False)["rules"]["12"], 0)
        self.assertEqual(self.question(asked=False, ends="UqAnswered")["rules"]["12"], 0)

    def test_a_digest_entry_left_unshown(self):
        for shown, want in ((False, 1), (True, 0)):
            s = Synth()
            s.session(1, cls="seed")
            s.session(3)
            s.agent("H", [3], human="h")
            s.tx(3, T0 + SEC)
            s.append(1, 3, T0 + SEC, {"FromUser": {"principal": 0}}, "hi")
            s.tx(None, T0 + 2 * SEC, "settle")
            d = s.append(
                3, None, T0 + 2 * SEC, {"System": {"Digest": {"item": {"DefaultApplied": {"question": 5}}, "to": 0}}}
            )
            s.call("H", 3, "view", {"action": "node"}, T0 + 3 * SEC)
            if shown:
                s.tx(3, T0 + 4 * SEC)
                s.ev(T0 + 4 * SEC, "DigestShown", {"entry": d, "by": 50})
                s.call("H", 3, "talk", {"action": "to_user"}, T0 + 4 * SEC)
            self.assertEqual(s.count()["rules"]["12"], want, shown)


class Rule13(unittest.TestCase):
    def worker(self, caps=(), horizon=None):
        s = Synth()
        s.session(1, cls="seed")
        s.session(2, caps=caps, horizon=horizon)
        s.agent("W", [2])
        s.call("W", 2, "session", {"action": "hello"}, T0 + SEC, structured={"offers": [{"path": "/10"}]})
        return s

    def test_a_release_that_waits_for_its_question_leaves_nothing_offered(self):
        s = self.worker()
        s.node(10, "wait-node")
        s.tx(2, T0 + 2 * SEC)
        s.take(10, 1, 2, T0 + 2 * SEC)
        s.tx(2, T0 + 3 * SEC)
        q = s.append(
            10,
            2,
            T0 + 3 * SEC,
            {
                "Question": {
                    "People": {
                        "conv": "chat:c/t",
                        "blocking": True,
                        "said": 1,
                        "supersedes": None,
                        "recheck_at": T0 + 6 * MIN,
                    }
                }
            },
        )
        s.ev(T0 + 4 * SEC, "WaitSet", {"node": 10, "wait": {"Question": {"q": q, "rechecks_at_set": 0}}})
        s.release(10, 1, 2, T0 + 4 * SEC, reason="Blocked")
        s.call("W", 2, "hold", {"action": "release"}, T0 + 4 * SEC, structured={"leave_better": []})
        self.assertEqual(s.count()["rules"]["13"], 0)

    def test_offered_work_left_is_a_slip_unless_next_would_exclude_it(self):
        def run(**kw):
            s = self.worker(caps=kw.pop("caps", ()), horizon=kw.pop("horizon", None))
            s.node(20, "offered", tags=["offer"], **kw)
            s.call("W", 2, "view", {"action": "list"}, T0 + 5 * SEC)
            return s.count()

        r = run()
        self.assertEqual([x["source"] for x in r["details"]["13"]], ["the counter's model of next"])
        self.assertEqual(run(needs=["gpu"])["rules"]["13"], 0)
        self.assertEqual(run(needs=["gpu"], caps=["gpu"])["rules"]["13"], 1)
        self.assertEqual(run(est=90, horizon=60)["rules"]["13"], 0)

    def test_the_servers_own_answer_wins(self):
        s = self.worker()
        s.node(20, "offered", tags=["offer"])
        s.call(
            "W", 2, "hold", {"action": "next"}, T0 + 5 * SEC, structured={"empty": True, "why": "x", "leave_better": []}
        )
        self.assertEqual(s.count()["rules"]["13"], 0)
        s = self.worker()
        s.node(20, "x", tags=["offer"])
        s.call("W", 2, "session", {"action": "hello"}, T0 + 5 * SEC, structured={"offers": [{"path": "/x"}]})
        self.assertEqual([x["source"] for x in s.count()["details"]["13"]], ["hello's offers"])
        # Empty offers come as {items: [], why} (1.4, R25).
        s = self.worker()
        s.node(20, "x", tags=["offer"])
        s.call("W", 2, "session", {"action": "hello"}, T0 + 5 * SEC, structured={"offers": {"items": [], "why": {}}})
        self.assertEqual(s.count()["rules"]["13"], 0)

    def test_the_leave_better_of_an_empty_next(self):
        s = self.worker()
        s.node(21, "tidy")
        s.node(22, "done-child", parent=21)
        s.close(22, 1, T0 + 2 * SEC)
        s.call(
            "W",
            2,
            "hold",
            {"action": "next"},
            T0 + 5 * SEC,
            structured={"empty": True, "why": "x", "leave_better": ["/tidy"]},
        )
        self.assertEqual(s.count()["details"]["13"][0]["leave_better"], ["/tidy"])


class Misreadings(unittest.TestCase):
    def test_user_and_the_humans_keys_are_not_invented(self):
        s = Synth()
        s.session(3)
        s.humans = {"dana": {"principal": None}}
        s.agent("H", [3], human="dana")
        s.call("H", 3, "talk", {"action": "to_user", "principal": "user", "kind": "result"}, T0 + SEC)
        s.call("H", 3, "session", {"action": "hello", "principal": "mail:dana@example.org"}, T0 + 2 * SEC)
        s.call("H", 3, "talk", {"action": "to_user", "principal": "mail:dana@example.org"}, T0 + 3 * SEC)
        r = s.count()
        self.assertEqual(r["misreadings"]["R1_principal"], 1)
        s.humans = {"dana": {"principal": "mail:dana@example.org"}}
        self.assertEqual(s.count()["misreadings"]["R1_principal"], 0)

    def test_an_invented_principal_in_any_argument_of_any_call_refused_or_not_is_r1(self):
        """R3: a worker passed `from_user.principal: user:<name>`, refused for its scheme, then
        `ide:<name>`, refused because `hello {from_user}` was not built yet. Both are invented
        principals (R1), and the first refusal is not a format learned (R20). The run is counted
        with what the server had then."""
        s = Synth()
        s.meta["built_packages"] = sorted(PK.BUILT_BEFORE_D5)
        s.session(3)
        s.agent("W", [3])
        scheme = (
            'from_user.principal: "user:sam" is not a principal key: the scheme is malformed or reserved: '
            "expected [a-z][a-z0-9+.-]{0,15}, and not repeat: (or user: for a principal)"
        )
        hello = {"action": "hello", "intent": "work"}
        s.call(
            "W",
            3,
            "session",
            dict(hello, from_user={"client": "ide", "principal": "user:sam", "text": "go"}),
            T0 + SEC,
            error="invalid",
            hint=scheme,
        )
        s.call(
            "W",
            3,
            "session",
            dict(hello, from_user={"client": "ide", "principal": "ide:sam", "text": "go"}),
            T0 + 2 * SEC,
            error="invalid",
            hint=BAD_PARAMS,
        )
        s.call(
            "W",
            3,
            "talk",
            {"action": "ask", "at": "/x", "text": "?", "to": "principal:mail:sam@example.org"},
            T0 + 3 * SEC,
        )
        s.call("W", 3, "talk", {"action": "to_user", "principal": "IDE:sam", "kind": "result"}, T0 + 4 * SEC)
        s.call(
            "W", 3, "talk", {"action": "say", "at": "/x", "text": "hi", "to": ["user", "principal:user"]}, T0 + 5 * SEC
        )
        r = s.count()
        got = [(d["call"], d["what"], d.get("refused")) for d in r["misreading_details"]["R1_principal"]]
        self.assertEqual(
            got,
            [
                (1, "invented principal user:sam", "invalid"),
                (2, "invented principal ide:sam", "invalid"),
                (3, "invented principal mail:sam@example.org", None),
            ],
        )
        self.assertEqual(r["misreadings"]["R20"], 0)
        self.assertEqual(r["refusals"], {"invalid": 1})
        self.assertEqual(r["refusals_not_built"], {"session hello": 1})
        # A format refusal that is not about an invented principal stays R20.
        s.call(
            "W",
            3,
            "hold",
            {"action": "take", "target": "/x", "lease": "soon"},
            T0 + 6 * SEC,
            error="invalid",
            hint='lease: "soon" is not a duration',
        )
        self.assertEqual(s.count()["misreadings"]["R20"], 1)
        self.assertEqual(
            SL.principals_given({"from_user": {"principal": "a:b"}, "to": "principal:c:d"}), ["a:b", "c:d"]
        )


class Packages(unittest.TestCase):
    def test_not_built(self):
        before_d5 = PK.BUILT_BEFORE_D5
        self.assertTrue(PK.not_built("talk", "decide", {}, "invalid", BAD_PARAMS, built=before_d5))
        self.assertFalse(PK.not_built("talk", "decide", {}, "invalid", BAD_PARAMS, built=before_d5 | {"WP-D5b"}))
        self.assertFalse(PK.not_built("talk", "decide", {}, "invalid", BAD_PARAMS))
        self.assertFalse(PK.not_built("hold", "take", {}, "invalid", BAD_PARAMS, built=before_d5))
        before_d4 = frozenset({"WP-D1", "WP-D2", "WP-D3"})
        self.assertTrue(PK.not_built("node", "create", {"keys": ["chat:c/t"]}, "invalid", BAD_PARAMS, built=before_d4))
        self.assertFalse(PK.not_built("node", "create", {"keys": ["chat:c/t"]}, "invalid", BAD_PARAMS))
        self.assertFalse(PK.not_built("node", "create", {}, "invalid", BAD_PARAMS))
        self.assertEqual(PK.package_of("talk", "ask", {"to": "principal:mail:a@b"}), "WP-D5a")
        self.assertEqual(PK.package_of("talk", "ask", {"to": "holders:/x"}), "WP-D5b")
        self.assertFalse(PK.not_built("talk", "said", {}, "not_found", "no such node"))

    def test_ready_scenarios_need_nothing_unbuilt(self):
        self.assertEqual(PK.missing(["WP-D4", "WP-D5b", "WP-D7"], built=PK.BUILT_BEFORE_D5), ["WP-D5b", "WP-D7"])
        self.assertEqual(PK.missing(["WP-D4", "WP-D5b", "WP-D7"]), [])
        self.assertEqual(PK.missing(["WP-D4", "an orientation switch"], built=PK.BUILT), ["an orientation switch"])
        self.assertEqual(PK.missing(None), [])
        self.assertEqual(SC.missing(SC.load("s0-smoke")), [])
        self.assertEqual(SC.missing(SC.load("s5-takeover")), [])
        self.assertEqual(SC.missing(SC.load("s8-newcomer")), ["an orientation switch (see about)"])
        # Every package a command belongs to has landed.
        self.assertEqual({pkg for *_, pkg in PK.COMMANDS} - PK.BUILT, set())


class Replay(unittest.TestCase):
    def test_until_cuts_the_history_and_paths_follow_the_tree(self):
        s = Synth()
        s.session(1)
        s.node(8, "a", at=T0)
        s.node(9, "b", parent=8, at=T0 + MIN)
        h = HI.History(s.lines, until=T0 + SEC)
        self.assertNotIn(9, h.nodes)
        h = HI.History(s.lines)
        self.assertEqual(h.path(9), "/a/b")
        self.assertEqual(h.path(PEOPLE), "/@people")
        self.assertEqual(h.node_by_ref("/a/b"), 9)
        self.assertEqual(h.node_by_ref("#9"), 9)

    def test_basis_changed(self):
        s = Synth()
        s.session(1)
        s.node(8, "kb")
        s.node(9, "rollup")
        s.tx(1, T0 + SEC)
        d1 = s.decide(8, 1, T0 + SEC)
        s.decide(9, 1, T0 + 2 * SEC, basis=[{"Entry": d1}])
        s.close(9, 1, T0 + 3 * SEC)
        h = HI.History(s.lines)
        self.assertNotIn("basis_changed", h.flags(9))
        s.tx(1, T0 + 4 * SEC)
        s.append(8, 1, T0 + 4 * SEC, {"Retract": {"target": {"Entry": d1}}})
        s.ev(T0 + 4 * SEC, "DecisionRetracted", {"d": d1, "by": s.eid})
        h = HI.History(s.lines)
        self.assertIn("basis_changed", h.flags(9))
        self.assertEqual([c["node"] for c in h.candidates(1)], [9])


if __name__ == "__main__":
    unittest.main()
