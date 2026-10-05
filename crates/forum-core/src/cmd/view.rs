//! The `view` actions (design D.8.25). Owner: WP-D3.
//!
//! Views read: the shell renders what they name from the post-state and its own tables. The core
//! validates the node references (`not_found(Node)`) and records what a view showed: `Shown` for a
//! node whose `seq` raises the session's `seen` (CS-14, A-85), and nothing else, so a re-read of an
//! unchanged node appends nothing.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::apply::target;
use crate::cmd::{BoardState, Cmd, FindBy, ViewCmd};
use crate::error::{Error, What, Why};
use crate::event::Ev;
use crate::helpers::Exec;
use crate::query::{ListFilter, PAGE_DEFAULT, PAGE_MAX};
use crate::reply::{BoardScope, Reply};
use crate::types::{Eid, EntryType, NodeId, Seq, ViewMode};

use super::Vcx;

/// `Shown { sess, n, seq }` when `seq` raises the session's `seen` of `n` (CS-14, A-85).
pub(crate) fn show_upto(ex: &mut Exec<'_>, n: NodeId, seq: Seq) {
    let Some(sess) = ex.sess() else { return };
    let seen = ex
        .s()
        .sessions
        .get(&sess)
        .and_then(|x| x.shown.get(&n).copied())
        .unwrap_or(Seq(0));
    if seq > seen && ex.s().nodes.contains_key(&n) {
        ex.emit(Ev::Shown {
            session: sess,
            node: n,
            seq,
        });
    }
}

/// `Shown` up to the node's current `seq`, when that raises the session's `seen` (D.8.6).
pub(crate) fn show(ex: &mut Exec<'_>, n: NodeId) {
    let seq = ex.s().nodes.get(&n).map_or(Seq(0), |x| x.seq);
    show_upto(ex, n, seq);
}

/// A page size: the given one, else `PAGE_DEFAULT`, at most `PAGE_MAX` (A-78).
fn page(limit: Option<u32>) -> u32 {
    limit.unwrap_or(PAGE_DEFAULT).min(PAGE_MAX)
}

/// The nodes a view names, after `target` resolved them.
fn nodes_of(cx: &Vcx<'_>, targets: &[crate::types::NodeRef]) -> Result<Vec<NodeId>, Error> {
    targets
        .iter()
        .map(|r| crate::apply::node_of(cx.s, r))
        .collect()
}

// ------------------------------------------------------------------------------------------------
// node

/// The plan of `view node`.
#[derive(Debug)]
pub(crate) struct NodePlan {
    nodes: Vec<NodeId>,
    view: ViewMode,
    page: u32,
}

/// Validates `view node {path | paths, view?, page?}`: every target resolves.
pub(crate) fn node(cx: &Vcx<'_>, cmd: &Cmd) -> Result<NodePlan, Error> {
    let Cmd::View(ViewCmd::Node {
        targets,
        view,
        page,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    target(cx.s, cmd)?;
    Ok(NodePlan {
        nodes: nodes_of(cx, targets)?,
        view: view.unwrap_or(ViewMode::Brief),
        page: page.unwrap_or(0),
    })
}

/// Executes `view node`: `Shown` for each node whose `seq` raises the session's `seen`.
pub(crate) fn node_exec(ex: &mut Exec<'_>, plan: NodePlan) -> Reply {
    let mut once: BTreeSet<NodeId> = BTreeSet::new();
    for &n in &plan.nodes {
        if once.insert(n) {
            show(ex, n);
        }
    }
    Reply::ViewNodes {
        nodes: plan.nodes,
        view: plan.view,
        page: plan.page,
    }
}

// ------------------------------------------------------------------------------------------------
// find

/// The plan of `view find`.
#[derive(Debug)]
pub(crate) struct FindPlan {
    by: FindBy,
}

/// Validates `view find {key | text}`: nothing to resolve; the shell renders `query::find_key` or
/// its own search.
pub(crate) fn find(_cx: &Vcx<'_>, cmd: &Cmd) -> Result<FindPlan, Error> {
    let Cmd::View(ViewCmd::Find { by }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(FindPlan { by: by.clone() })
}

/// Executes `view find`.
pub(crate) fn find_exec(_ex: &mut Exec<'_>, plan: FindPlan) -> Reply {
    Reply::ViewFind { by: plan.by }
}

// ------------------------------------------------------------------------------------------------
// feed

/// The plan of `view feed`.
#[derive(Debug)]
pub(crate) struct FeedPlan {
    node: NodeId,
    after: Seq,
    upto: Seq,
    types: BTreeSet<EntryType>,
    limit: u32,
}

/// Validates `view feed {at, since?, types?, limit?}` (D.8.25): `at` resolves; the page is the seqs
/// `after + 1 ..= upto` with `upto = min(after + limit, n.seq)`, which the shell lists (CS-17).
pub(crate) fn feed(cx: &Vcx<'_>, cmd: &Cmd) -> Result<FeedPlan, Error> {
    let Cmd::View(ViewCmd::Feed {
        since,
        types,
        limit,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))?;
    let after = since.unwrap_or(Seq(0));
    let limit = page(*limit);
    let seq = cx.s.nodes.get(&n).map_or(Seq(0), |x| x.seq);
    let upto = Seq(after.0.saturating_add(u64::from(limit)).min(seq.0));
    Ok(FeedPlan {
        node: n,
        after,
        upto,
        types: types.clone(),
        limit,
    })
}

/// Executes `view feed`: with no `types`, `Shown` up to `upto` when that raises `seen`.
pub(crate) fn feed_exec(ex: &mut Exec<'_>, plan: FeedPlan) -> Reply {
    if plan.types.is_empty() {
        show_upto(ex, plan.node, plan.upto);
    }
    Reply::ViewFeed {
        node: plan.node,
        after: plan.after,
        upto: plan.upto,
        types: plan.types,
        limit: plan.limit,
    }
}

// ------------------------------------------------------------------------------------------------
// list

/// The plan of `view list`.
#[derive(Debug)]
pub(crate) struct ListPlan {
    filter: ListFilter,
}

/// `list`'s default page and its largest (§20.4).
const LIST_DEFAULT: u32 = 50;
/// The largest page of `view list`.
const LIST_MAX: u32 = 500;

/// Validates `view list {under?, kind?, tags?, status?, fields?, flags?, closed?, limit?}`: `under`
/// resolves.
pub(crate) fn list(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ListPlan, Error> {
    let Cmd::View(ViewCmd::List {
        kind,
        tags,
        status,
        fields,
        flags,
        closed,
        limit,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let under = target(cx.s, cmd)?;
    Ok(ListPlan {
        filter: ListFilter {
            under,
            kind: kind.clone(),
            tags: tags.clone(),
            status: status.clone(),
            fields: fields.clone(),
            flags: *flags,
            closed: *closed,
            limit: limit.unwrap_or(LIST_DEFAULT).min(LIST_MAX),
        },
    })
}

/// Executes `view list`: the page of `query::list`.
pub(crate) fn list_exec(ex: &mut Exec<'_>, plan: ListPlan) -> Reply {
    let now = ex.now();
    let p = crate::query::list(ex.s(), &plan.filter, now, &mut crate::query::NoMeter);
    Reply::ViewList {
        nodes: p.nodes,
        total: p.total,
    }
}

// ------------------------------------------------------------------------------------------------
// agreements

/// The plan of `view agreements` (1.4, D.14.5).
#[derive(Debug)]
pub(crate) struct AgreementsPlan {
    under: Option<NodeId>,
    limit: u32,
}

/// Validates `view agreements {under?, limit?}` (1.4, §20.4, D.14.5): `under` resolves; the page is
/// 50 by default and at most 500, as `list`'s.
pub(crate) fn agreements(cx: &Vcx<'_>, cmd: &Cmd) -> Result<AgreementsPlan, Error> {
    let Cmd::View(ViewCmd::Agreements { limit, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let under = target(cx.s, cmd)?;
    Ok(AgreementsPlan {
        under,
        limit: limit.unwrap_or(LIST_DEFAULT).min(LIST_MAX),
    })
}

/// Executes `view agreements`: the page of `query::agreements`.
pub(crate) fn agreements_exec(ex: &mut Exec<'_>, plan: AgreementsPlan) -> Reply {
    let p = crate::query::agreements(ex.s(), plan.under, plan.limit, &mut crate::query::NoMeter);
    Reply::Agreements {
        items: p.items,
        total: p.total,
    }
}

// ------------------------------------------------------------------------------------------------
// history

/// The plan of `view history`.
#[derive(Debug)]
pub(crate) struct HistoryPlan {
    node: NodeId,
    before: Option<u64>,
    limit: u32,
}

/// Validates `view history {path, before?, limit?}`: the node resolves; the shell pages its
/// events (A-78).
pub(crate) fn history(cx: &Vcx<'_>, cmd: &Cmd) -> Result<HistoryPlan, Error> {
    let Cmd::View(ViewCmd::History { before, limit, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let n = target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))?;
    Ok(HistoryPlan {
        node: n,
        before: *before,
        limit: page(*limit),
    })
}

/// Executes `view history`.
pub(crate) fn history_exec(_ex: &mut Exec<'_>, plan: HistoryPlan) -> Reply {
    Reply::ViewHistory {
        node: plan.node,
        before: plan.before,
        limit: plan.limit,
    }
}

// ------------------------------------------------------------------------------------------------
// requests

/// The plan of `view requests`.
#[derive(Debug)]
pub(crate) struct RequestsPlan {
    status: Option<BoardState>,
    scope: BoardScope,
}

/// Validates `view requests {status?, requester?}`: the scope by `query::board_scope` (a key that
/// names no interned principal gives an empty scope, not an error, D.8.25).
pub(crate) fn requests(cx: &Vcx<'_>, cmd: &Cmd) -> Result<RequestsPlan, Error> {
    let Cmd::View(ViewCmd::Requests { status, requester }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let sess = cx.sess.ok_or(Error::invalid(Why::BadParams))?;
    Ok(RequestsPlan {
        status: *status,
        scope: crate::query::board_scope(cx.s, sess, requester.as_ref()),
    })
}

/// Executes `view requests`.
pub(crate) fn requests_exec(_ex: &mut Exec<'_>, plan: RequestsPlan) -> Reply {
    Reply::ViewRequests {
        status: plan.status,
        scope: plan.scope,
    }
}

// ------------------------------------------------------------------------------------------------
// user_log

/// The plan of `view user_log`.
#[derive(Debug)]
pub(crate) struct UserLogPlan {
    since: Option<Eid>,
    limit: u32,
}

/// Validates `view user_log {since?, limit?}`: the shell pages `@user` (CS-17).
pub(crate) fn user_log(_cx: &Vcx<'_>, cmd: &Cmd) -> Result<UserLogPlan, Error> {
    let Cmd::View(ViewCmd::UserLog { since, limit }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(UserLogPlan {
        since: *since,
        limit: page(*limit),
    })
}

/// Executes `view user_log`.
pub(crate) fn user_log_exec(_ex: &mut Exec<'_>, plan: UserLogPlan) -> Reply {
    Reply::ViewUserLog {
        since: plan.since,
        limit: plan.limit,
    }
}

// ------------------------------------------------------------------------------------------------
// constitution

/// The plan of `view constitution`.
#[derive(Debug)]
pub(crate) struct ConstitutionPlan;

/// Validates `view constitution {}`.
pub(crate) fn constitution(_cx: &Vcx<'_>, cmd: &Cmd) -> Result<ConstitutionPlan, Error> {
    let Cmd::View(ViewCmd::Constitution) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(ConstitutionPlan)
}

/// Executes `view constitution`.
pub(crate) fn constitution_exec(_ex: &mut Exec<'_>, _plan: ConstitutionPlan) -> Reply {
    Reply::ViewConstitution
}
