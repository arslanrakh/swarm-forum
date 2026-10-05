//! The commands of `node link` and `node key`, and the join branch of create-or-join in `node
//! create` (design D.8.9, D.8.20). Owner: WP-D4.
//!
//! Each command is a pair: a validation that reads the state and returns the plan or the error,
//! and an execution of the plan that emits the events and cannot fail. Every validation follows
//! D.8.24's row of its command.

use alloc::vec::Vec;

use crate::apply::{node_of, target};
use crate::cmd::{Cmd, KeyChange, NodeCmd};
use crate::error::{Error, What, Why};
use crate::event::Ev;
use crate::helpers::Exec;
use crate::helpers::conversation::{conv_node, record_request, route_to};
use crate::helpers::principals::intern;
use crate::helpers::restructure::{KeyAdd, join_adds, key_add, open_signal, resolve_pair};
use crate::reply::Reply;
use crate::state::{Origin, Routing, State};
use crate::types::{
    Key, KeyStr, LinkRel, NodeId, PrincipalKey, QuestionId, RequestId, SignalKind, UserEntryId,
};

use super::Vcx;
use super::node_basic::NewRequest;

/// Whether `n` is one of the reserved nodes (§4, K8).
pub(super) fn reserved(s: &State, n: NodeId) -> bool {
    n.is_reserved()
        || s.nodes
            .get(&n)
            .is_some_and(|x| x.origin == Origin::Reserved)
}

// ------------------------------------------------------------------------------------------------
// link

/// The plan of `node link` (D.8.20).
#[derive(Debug)]
pub(crate) struct LinkPlan {
    at: NodeId,
    rel: LinkRel,
    to: NodeId,
    /// The identical link exists: nothing changes.
    existing: bool,
}

/// Validates `node link {path, rel, to}` (D.8.20, D.8.24's row): both nodes resolve
/// (`not_found(Node)`) → `invalid(SelfLink)`. `to` is stored as given: readers follow
/// `merged_into` (§16).
pub(crate) fn link(cx: &Vcx<'_>, cmd: &Cmd) -> Result<LinkPlan, Error> {
    let Cmd::Node(NodeCmd::Link { rel, to, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let at = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let to = node_of(s, to)?;
    if at == to {
        return Err(Error::invalid(Why::SelfLink));
    }
    let existing = s
        .nodes
        .get(&at)
        .is_some_and(|x| x.links.contains(&(*rel, to)));
    Ok(LinkPlan {
        at,
        rel: *rel,
        to,
        existing,
    })
}

/// Executes `node link`: `LinkAdded` (version + 1), then a `duplicate` link opens a duplicate
/// signal, a `conflicts_with` link a conflict signal, and a `superseded_by` link resolves the
/// pair's open signals of both kinds (D.8.19). An identical link changes nothing.
pub(crate) fn link_exec(ex: &mut Exec<'_>, plan: LinkPlan) -> Reply {
    if plan.existing {
        return Reply::Linked {
            existing: true,
            signal: None,
        };
    }
    let (at, to) = (plan.at, plan.to);
    ex.emit(Ev::LinkAdded {
        node: at,
        rel: plan.rel,
        to,
    });
    let signal = match plan.rel {
        LinkRel::Duplicate => open_signal(ex, SignalKind::Duplicate, at, to),
        LinkRel::ConflictsWith => open_signal(ex, SignalKind::Conflict, at, to),
        LinkRel::SupersededBy => {
            resolve_pair(ex, at, to, "resolved: linked as superseded_by");
            None
        }
        LinkRel::Continues | LinkRel::DerivedFrom => None,
    };
    Reply::Linked {
        existing: false,
        signal,
    }
}

// ------------------------------------------------------------------------------------------------
// key

/// The plan of `node key` (D.8.9).
#[derive(Debug)]
pub(crate) enum KeyPlan {
    /// `node key {add}`.
    Add {
        n: NodeId,
        key: Key,
        choice: KeyAdd,
        /// The principal of a `person:` key, when it is new (D.2.2).
        person: Option<PrincipalKey>,
        /// The entry of a `user:e<n>` key, when it is unrouted.
        route: Option<UserEntryId>,
    },
    /// `node key {remove}`.
    Remove { n: NodeId, key: KeyStr },
}

/// Validates `node key {path, add | remove}` (D.8.9, D.8.24's row): the node → a `repeat:` key
/// (`invalid(ReservedKey)`, A-90) → a reserved node (`invalid(ReservedNode)`, A-29, K8) → for an
/// addition, a `user:e<n>` key naming no user entry (`not_found(UserEntry)`); for a removal, a key
/// the node does not carry (`not_found(Key)`).
pub(crate) fn key(cx: &Vcx<'_>, cmd: &Cmd) -> Result<KeyPlan, Error> {
    let Cmd::Node(NodeCmd::Key { change, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let n = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let k = match change {
        KeyChange::Add(k) => &k.key,
        KeyChange::Remove(k) => k,
    };
    if k.is_reserved() {
        return Err(Error::invalid(Why::ReservedKey));
    }
    if reserved(s, n) {
        return Err(Error::invalid(Why::ReservedNode));
    }
    let Some(node) = s.nodes.get(&n) else {
        return Err(Error::not_found(What::Node));
    };
    match change {
        KeyChange::Add(key) => {
            let route = match key.key.user_entry() {
                Some(e) => {
                    let u = s
                        .user_entries
                        .get(&e)
                        .ok_or(Error::not_found(What::UserEntry))?;
                    (u.routing == Routing::Unrouted).then_some(e)
                }
                None => None,
            };
            let person = key
                .key
                .person()
                .filter(|p| !s.principal_ids.contains_key(p));
            let choice = key_add(
                s.ix.keys_open.get(&key.key).copied(),
                n,
                node.keys.contains_key(&key.key),
            );
            Ok(KeyPlan::Add {
                n,
                key: key.clone(),
                choice,
                person,
                route,
            })
        }
        KeyChange::Remove(k) => {
            if !node.keys.contains_key(k) {
                return Err(Error::not_found(What::Key));
            }
            Ok(KeyPlan::Remove { n, key: k.clone() })
        }
    }
}

/// Executes `node key` (D.8.9): a key another open node carries is not added, and opens a
/// `duplicate` signal (`KeyExisting`); a key the node carries changes nothing; otherwise
/// `PrincipalSeen` for a new `person:` principal, `KeyAdded`, and the routing of an unrouted
/// `user:e<n>` entry to the node. A removal is `KeyRemoved`.
pub(crate) fn key_exec(ex: &mut Exec<'_>, plan: KeyPlan) -> Reply {
    match plan {
        KeyPlan::Add {
            n,
            key,
            choice,
            person,
            route,
        } => match choice {
            KeyAdd::Existing(o) => Reply::KeyExisting {
                existing: o,
                signal: open_signal(ex, SignalKind::Duplicate, n, o),
            },
            KeyAdd::Already => Reply::KeyAdded {
                already: true,
                user_request: None,
            },
            KeyAdd::Add => {
                if let Some(p) = person {
                    intern(ex, &p);
                }
                ex.emit(Ev::KeyAdded { node: n, key });
                // 1.4 (R3): the request the routing of an added `user:e<n>` recorded.
                let user_request = route.and_then(|e| route_to(ex, e, n));
                Reply::KeyAdded {
                    already: false,
                    user_request,
                }
            }
        },
        KeyPlan::Remove { n, key } => {
            ex.emit(Ev::KeyRemoved { node: n, key });
            Reply::KeyRemoved
        }
    }
}

// ------------------------------------------------------------------------------------------------
// create-or-join: the join (D.8.9)

/// The plan of a `node create` that joins an open carrier (D.8.9): nothing but the keys and the
/// request was validated (§7: "ignores the other fields").
#[derive(Debug)]
pub(crate) struct JoinPlan {
    /// The open carrier with the smallest id.
    c: NodeId,
    /// The other open carriers, ascending.
    others: Vec<NodeId>,
    /// `person:` keys whose principal is new.
    persons: Vec<PrincipalKey>,
    /// The keys no open node carries, in the order given.
    add: Vec<Key>,
    /// The unrouted user entries among the added keys.
    route: Vec<UserEntryId>,
    /// The request, with its defaults.
    request: Option<NewRequest>,
}

/// The join of a create whose `keys` (deduplicated, checked) an open node carries, or `None` when
/// none does and the call creates a node (D.8.9 step 3). A carrier is never reserved: no key can
/// be added to a reserved node (A-29).
pub(super) fn join(s: &State, keys: &[Key], request: Option<&NewRequest>) -> Option<JoinPlan> {
    let carriers: alloc::collections::BTreeSet<NodeId> = keys
        .iter()
        .filter_map(|k| s.ix.keys_open.get(&k.key).copied())
        .collect();
    let mut carriers = carriers.into_iter();
    let c = carriers.next()?;
    let others: Vec<NodeId> = carriers.collect();
    let add: Vec<Key> = keys
        .iter()
        .filter(|k| join_adds(s.ix.keys_open.get(&k.key).copied()))
        .cloned()
        .collect();
    let mut persons: Vec<PrincipalKey> = Vec::new();
    for k in keys {
        if let Some(p) = k.key.person()
            && !s.principal_ids.contains_key(&p)
            && !persons.contains(&p)
        {
            persons.push(p);
        }
    }
    let route: Vec<UserEntryId> = add
        .iter()
        .filter_map(|k| k.key.user_entry())
        .filter(|e| {
            s.user_entries
                .get(e)
                .is_some_and(|u| u.routing == Routing::Unrouted)
        })
        .collect();
    Some(JoinPlan {
        c,
        others,
        persons,
        add,
        route,
        request: request.cloned(),
    })
}

/// The oldest people question in state `Open` on `c` (§7's `open_question`).
fn oldest_open_question(s: &State, c: NodeId) -> Option<QuestionId> {
    s.ix.items
        .get(&c)?
        .open_pq
        .values()
        .filter_map(|qs| qs.first().copied())
        .min()
}

/// Executes the join (D.8.9): `PrincipalSeen` for each new `person:` principal; `KeyAdded` on the
/// carrier for every key no open node carries; the routing of each added `user:e<n>` key's
/// unrouted entry to the carrier; the request on the conversation node of its `where` (with
/// `KeyAdded` first in case 3), which is the entry's own request when its origin is a user's
/// message (`record_request`, §9.4); a `duplicate` signal with each other open carrier. No hold.
pub(super) fn join_exec(ex: &mut Exec<'_>, plan: JoinPlan) -> Reply {
    let c = plan.c;
    for p in &plan.persons {
        intern(ex, p);
    }
    for k in &plan.add {
        ex.emit(Ev::KeyAdded {
            node: c,
            key: k.clone(),
        });
    }
    // 1.4 (R3): the requests the routings record are the reply's `user_requests`, ascending.
    let mut user_requests: Vec<RequestId> = plan
        .route
        .iter()
        .filter_map(|&e| route_to(ex, e, c))
        .collect();
    user_requests.sort_unstable();
    let mut request = None;
    if let Some(r) = plan.request
        && let Ok(cn) = conv_node(ex.s(), c, &r.where_.key)
    {
        if cn.add_key {
            ex.emit(Ev::KeyAdded {
                node: cn.node,
                key: r.where_.clone(),
            });
        }
        let (rid, existed) = record_request(ex, cn.node, r.origin, r.where_.key, None, r.text);
        if !existed {
            request = Some(rid);
        }
    }
    for &o in &plan.others {
        open_signal(ex, SignalKind::Duplicate, c, o);
    }
    let now = ex.now();
    let s = ex.s();
    let holders: Vec<crate::types::SessionId> = crate::query::holders(s, c, now)
        .into_iter()
        .map(|(x, _)| x)
        .collect();
    Reply::Joined {
        existing: c,
        holders,
        open_question: oldest_open_question(s, c),
        added_keys: plan.add.into_iter().map(|k| k.key).collect(),
        request,
        user_requests,
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
pub(super) mod tests;
