//! The guided agents of D.15.4 (1.3): with probability `p_follow` an agent makes the first `now`
//! next call of its last response, when there is one, instead of its own next action. This turns
//! a next call's skeleton (D.8.26) into the command an agent would send, filling the texts the
//! skeleton leaves to it. `Hello` (a new session) is not followed: the agent is already in one.

use std::collections::BTreeSet;

use forum_core::query::guide::{Guidance, Moment, RouteSk, Skeleton};
use forum_core::{
    Cmd, HoldCmd, NodeCmd, NodeRef, ReleaseReason, SaidKind, State, TalkCmd, ViewCmd, WaitArg,
};

use crate::cmds::{self, text};

/// The first `now` item of a response, as a command, if it has one the agent can make.
#[must_use]
pub fn first_now(g: &Guidance, s: &State) -> Option<Cmd> {
    let item = g.items.first()?;
    if item.when != Moment::Now {
        return None;
    }
    to_cmd(&item.call, s)
}

/// A skeleton as the command an agent sends, with its texts filled.
#[must_use]
pub fn to_cmd(sk: &Skeleton, s: &State) -> Option<Cmd> {
    Some(match sk {
        Skeleton::Hello => return None,
        Skeleton::LogFromUser { principal } => Cmd::Talk(TalkCmd::FromUser {
            text: text("message"),
            client: text("chat"),
            principal: s.ix.principal_key.get(principal).cloned(),
            answers: None,
            to: None,
            clarifies: None,
            reason: None,
        }),
        Skeleton::ViewNode { node } => Cmd::View(ViewCmd::Node {
            targets: vec![NodeRef::Id(*node)],
            view: None,
            page: None,
        }),
        Skeleton::Checkpoint {
            token,
            intent_done,
            intent,
        } => Cmd::Hold(HoldCmd::Checkpoint {
            token: *token,
            done: text("progress"),
            next_step: text("continue"),
            intent: intent.then(|| text("an outside action")),
            intent_done: intent_done.then(|| text("it happened")),
            files: Vec::new(),
        }),
        Skeleton::CheckpointSwap { token } => Cmd::Hold(HoldCmd::Checkpoint {
            token: *token,
            done: text("progress"),
            next_step: text("continue"),
            intent: None,
            intent_done: Some(text("checked: it happened")),
            files: Vec::new(),
        }),
        Skeleton::Release { token, wait } => match wait {
            Some(q) => cmds::release(*token, ReleaseReason::Blocked, Some(WaitArg::Question(*q))),
            None => cmds::release(*token, ReleaseReason::Handoff, None),
        },
        Skeleton::Take { target } => Cmd::Hold(HoldCmd::Take {
            target: NodeRef::Id(*target),
            lease: None,
        }),
        Skeleton::Next => cmds::next(),
        Skeleton::CreateFor { entry } => {
            let mut p = forum_core::Path::default();
            p.0.push(forum_core::Name::new(&format!("for-e{}", entry.0))?);
            cmds::create(p, "a request", |a| {
                a.keys = vec![cmds::user_entry_key(*entry)];
            })
        }
        Skeleton::Split { parent } => {
            let mut p = path_of(s, *parent)?;
            p.0.push(forum_core::Name::new(&format!(
                "split-{}",
                s.counters.next_node.0
            ))?);
            cmds::create(p, "a part", |a| {
                a.tags = [cmds::label(forum_core::OFFER)].into();
            })
        }
        Skeleton::AddOffer { node, version } => cmds::update(*node, *version, |p| {
            p.tags_add = [cmds::label(forum_core::OFFER)].into();
        }),
        Skeleton::Close {
            node,
            token,
            settles,
            outcome,
        } => Cmd::Node(NodeCmd::Close {
            at: NodeRef::Id(*node),
            outcome: outcome.clone().unwrap_or(forum_core::Outcome::Done),
            summary: text("done"),
            verified: None,
            // 1.7 (§5): an item without `settles` leaves it to the close's default.
            settles: (!settles.is_empty()).then(|| settles.iter().copied().collect()),
            learnings: None,
            token: Some(*token),
            unsent: BTreeSet::new(),
        }),
        Skeleton::SayTo { at, to } => Cmd::Talk(TalkCmd::Say {
            at: NodeRef::Id(*at),
            text: text("a note"),
            to: Some(forum_core::AddresseeArg::Session(*to)),
            expects: None,
            reply_to: None,
            until: None,
        }),
        Skeleton::ToUser {
            principal,
            kind,
            re,
        } => Cmd::Talk(TalkCmd::ToUser {
            text: text("report"),
            kind: kind.unwrap_or(forum_core::ToUserKind::Result),
            verbatim: true,
            principal: s.ix.principal_key.get(principal).cloned(),
            re: re.iter().copied().collect(),
            at: None,
        }),
        Skeleton::Said {
            at,
            kind,
            where_,
            where_fill: _,
        } => Cmd::Talk(TalkCmd::Said {
            at: NodeRef::Id(*at),
            where_: where_.as_ref().map(|k| forum_core::Key {
                key: k.clone(),
                nv: forum_core::NormVer(1),
            }),
            text: text("reply"),
            kind: if *kind == SaidKind::Question {
                SaidKind::Reply
            } else {
                *kind
            },
            corrects: None,
            seen: None,
        }),
        Skeleton::Heard {
            at,
            answers,
            where_,
        } => Cmd::Talk(TalkCmd::Heard {
            at: NodeRef::Id(*at),
            where_: Some(forum_core::Key {
                key: where_.clone(),
                nv: forum_core::NormVer(1),
            }),
            from: text("reporter"),
            text: text("their answer"),
            answers: Some(*answers),
        }),
        Skeleton::Decide {
            at,
            settles,
            supersedes,
        } => Cmd::Talk(TalkCmd::Decide {
            at: NodeRef::Id(*at),
            text: text("decided"),
            settles: settles.iter().copied().collect(),
            supersedes: *supersedes,
            basis: Vec::new(),
            evidence: None,
        }),
        Skeleton::Route { entry, to } => match to {
            RouteSk::Fill | RouteSk::Nowhere => cmds::route_none(*entry),
            RouteSk::Node(n) => Cmd::Talk(TalkCmd::Route {
                entry: *entry,
                to: Some(forum_core::RouteTo::Node(NodeRef::Id(*n))),
                answers: None,
                clarifies: None,
                reason: None,
            }),
            RouteSk::Answers(q) => Cmd::Talk(TalkCmd::Route {
                entry: *entry,
                to: None,
                answers: Some(*q),
                clarifies: None,
                reason: None,
            }),
            // 1.11 (§8.2): the default's clarification of the person's open request.
            RouteSk::Clarifies(r) => Cmd::Talk(TalkCmd::Route {
                entry: *entry,
                to: None,
                answers: None,
                clarifies: Some(*r),
                reason: None,
            }),
        },
        Skeleton::AnswerUser {
            question,
            principal,
        } => Cmd::Talk(TalkCmd::FromUser {
            text: text("answer"),
            client: text("chat"),
            principal: principal.and_then(|p| s.ix.principal_key.get(&p).cloned()),
            answers: Some(*question),
            to: None,
            clarifies: None,
            reason: None,
        }),
        Skeleton::AnswerAgent { question } => {
            let q = s.aquestions.get(question)?;
            Cmd::Talk(TalkCmd::Answer {
                question: *question,
                node: q.node,
                asker: Some(q.asker),
                text: text("answer"),
            })
        }
        Skeleton::Resolve { objection } => Cmd::Talk(TalkCmd::Resolve {
            objection: *objection,
            note: text("resolved"),
            upheld: false,
            defer: false,
        }),
        Skeleton::Merge { from, into } => Cmd::Node(NodeCmd::Merge {
            from: NodeRef::Id(*from),
            into: NodeRef::Id(*into),
        }),
        Skeleton::LinkDuplicate { from, to } => Cmd::Node(NodeCmd::Link {
            at: NodeRef::Id(*from),
            rel: forum_core::LinkRel::Duplicate,
            to: NodeRef::Id(*to),
        }),
        // 1.10 (§16): a stale offer's question for its owners.
        Skeleton::AskOwners { at } => Cmd::Talk(TalkCmd::Ask {
            at: NodeRef::Id(*at),
            text: text("is this offer still wanted?"),
            to: forum_core::AskTo::User(forum_core::query::guide::stale_ask()),
            again: false,
        }),
        Skeleton::Wait { timeout } => Cmd::Talk(TalkCmd::Wait {
            timeout: *timeout,
            for_: None,
            since: None,
        }),
        // 1.12 (rule 9): the reopen of a claim nothing proves.
        Skeleton::Reopen { node } => Cmd::Node(NodeCmd::Reopen {
            at: NodeRef::Id(*node),
            reason: text("nothing proves it"),
        }),
    })
}

/// The path of a node, from its places.
fn path_of(s: &State, n: forum_core::NodeId) -> Option<forum_core::Path> {
    let mut names = Vec::new();
    let mut at = n;
    for _ in 0..64 {
        let node = s.nodes.get(&at)?;
        match &node.place {
            Some(p) => {
                names.push(p.name.clone());
                at = p.parent;
            }
            None => {
                names.reverse();
                return Some(forum_core::Path(names));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use forum_core::query::guide::{Because, NextCall, Rule};
    use forum_core::{Fence, NodeId, Token, Total};

    fn guidance(when: Moment, call: Skeleton) -> Guidance {
        Guidance {
            items: vec![NextCall {
                rule: Rule::Checkpoint,
                when,
                once: false,
                call,
                because: Because::RenewLimit,
            }],
            total: Total::Exact(1),
            end: None,
        }
    }

    #[test]
    fn only_a_first_now_item_is_followed() {
        let s = State::default();
        let token = Token {
            node: NodeId(9),
            fence: Fence(1),
        };
        let now = guidance(
            Moment::Now,
            Skeleton::Checkpoint {
                token,
                intent_done: false,
                intent: false,
            },
        );
        assert!(matches!(
            first_now(&now, &s),
            Some(Cmd::Hold(HoldCmd::Checkpoint { .. }))
        ));
        let later = guidance(Moment::Later, Skeleton::Next);
        assert!(first_now(&later, &s).is_none());
        let hello = guidance(Moment::Now, Skeleton::Hello);
        assert!(first_now(&hello, &s).is_none());
        let empty = Guidance {
            items: Vec::new(),
            total: Total::Exact(0),
            end: None,
        };
        assert!(first_now(&empty, &s).is_none());
    }
}
