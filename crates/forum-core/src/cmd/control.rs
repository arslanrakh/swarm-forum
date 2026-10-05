//! The `control` actions and `AdminCmd::SetParams` (design D.8.21, D.8.25, D.11.9). Owner: WP-D3.
//!
//! Anyone may pause and anyone may lift a pause (§19, FQ-3); a pause stops new holds only (P2). A
//! pause's scope is a node id, so it follows moves (A-72).

use alloc::vec::Vec;

use crate::apply::target;
use crate::cmd::{AdminCmd, Cmd, ControlCmd};
use crate::error::{Error, Field, What, Why};
use crate::event::{EntryBody, EntryText, Ev, SystemNote};
use crate::helpers::Exec;
use crate::params::Params;
use crate::reply::Reply;
use crate::types::{NodeId, Text};

use super::Vcx;

/// `control export`'s page when none is given (D.11.15).
const EXPORT_DEFAULT: u32 = 1_000;

// ------------------------------------------------------------------------------------------------
// pause

/// The plan of `control pause` (D.8.21).
#[derive(Debug)]
pub(crate) struct PausePlan {
    scope: NodeId,
    reason: Text,
}

/// Validates `control pause {scope, reason}` (D.8.21): the scope resolves → a non-empty reason.
pub(crate) fn pause(cx: &Vcx<'_>, cmd: &Cmd) -> Result<PausePlan, Error> {
    let Cmd::Control(ControlCmd::Pause { reason, .. }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let scope = target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))?;
    if reason.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::Reason)));
    }
    Ok(PausePlan {
        scope,
        reason: reason.clone(),
    })
}

/// Executes `control pause`: `Paused` (a renewal when the scope is paused already, the renewer
/// becoming the source, §19, CS-9), then the pause's system entry with the reason.
pub(crate) fn pause_exec(ex: &mut Exec<'_>, plan: PausePlan) -> Reply {
    let now = ex.now();
    let ttl = ex.s().params.pause_ttl;
    let until = now.plus(ttl);
    let renewed = ex.s().pauses.contains_key(&plan.scope);
    let Some(sess) = ex.sess() else {
        return Reply::Paused {
            scope: plan.scope,
            until,
            renewed,
        };
    };
    let class = ex
        .s()
        .sessions
        .get(&sess)
        .map_or_else(crate::types::ClassName::unknown, |x| x.class.clone());
    ex.emit(Ev::Paused {
        scope: plan.scope,
        until,
        ttl,
        by: sess,
        class,
        renewed,
    });
    let by = ex.author();
    ex.append(
        plan.scope,
        by,
        EntryBody::System(SystemNote::Paused { until }),
        EntryText {
            main: plan.reason,
            extra: Vec::new(),
        },
    );
    Reply::Paused {
        scope: plan.scope,
        until,
        renewed,
    }
}

// ------------------------------------------------------------------------------------------------
// unpause

/// The plan of `control unpause` (D.8.21).
#[derive(Debug)]
pub(crate) struct UnpausePlan {
    scope: NodeId,
}

/// Validates `control unpause {scope}` (D.8.21): the scope resolves → an active pause there
/// (`not_found(Pause)`).
pub(crate) fn unpause(cx: &Vcx<'_>, cmd: &Cmd) -> Result<UnpausePlan, Error> {
    let scope = target(cx.s, cmd)?.ok_or(Error::not_found(What::Node))?;
    if cx.s.pauses.get(&scope).is_none_or(|p| p.until <= cx.now) {
        return Err(Error::not_found(What::Pause));
    }
    Ok(UnpausePlan { scope })
}

/// Executes `control unpause`: `Unpaused`, then its system entry on the scope.
pub(crate) fn unpause_exec(ex: &mut Exec<'_>, plan: UnpausePlan) -> Reply {
    ex.emit(Ev::Unpaused { scope: plan.scope });
    let by = ex.author();
    ex.append(
        plan.scope,
        by,
        EntryBody::System(SystemNote::Unpaused),
        EntryText::default(),
    );
    Reply::Unpaused { scope: plan.scope }
}

// ------------------------------------------------------------------------------------------------
// health, constitution, export

/// The plan of `control health`: the shell renders its cache of `query::health` (D.8.22).
#[derive(Debug)]
pub(crate) struct HealthPlan;

/// Validates `control health {}`.
pub(crate) fn health(_cx: &Vcx<'_>, cmd: &Cmd) -> Result<HealthPlan, Error> {
    let Cmd::Control(ControlCmd::Health) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(HealthPlan)
}

/// Executes `control health`.
pub(crate) fn health_exec(_ex: &mut Exec<'_>, _plan: HealthPlan) -> Reply {
    Reply::Health
}

/// The plan of `control constitution`: read-only in the first release (A-21).
#[derive(Debug)]
pub(crate) struct ConstitutionPlan;

/// Validates `control constitution {}`.
pub(crate) fn constitution(_cx: &Vcx<'_>, cmd: &Cmd) -> Result<ConstitutionPlan, Error> {
    let Cmd::Control(ControlCmd::Constitution) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(ConstitutionPlan)
}

/// Executes `control constitution`.
pub(crate) fn constitution_exec(_ex: &mut Exec<'_>, _plan: ConstitutionPlan) -> Reply {
    Reply::Constitution
}

/// The plan of `control export` (D.11.15): the shell streams the events.
#[derive(Debug)]
pub(crate) struct ExportPlan {
    after: Option<u64>,
    limit: u32,
}

/// Validates `control export {after?, limit?}`: a page of the history after `after`, at most
/// `limit` events, 1,000 by default (D.11.15).
pub(crate) fn export(_cx: &Vcx<'_>, cmd: &Cmd) -> Result<ExportPlan, Error> {
    let Cmd::Control(ControlCmd::Export { after, limit }) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    Ok(ExportPlan {
        after: *after,
        limit: limit.unwrap_or(EXPORT_DEFAULT),
    })
}

/// Executes `control export`.
pub(crate) fn export_exec(_ex: &mut Exec<'_>, plan: ExportPlan) -> Reply {
    Reply::Export {
        after: plan.after,
        limit: plan.limit,
    }
}

// ------------------------------------------------------------------------------------------------
// AdminCmd::SetParams

/// The plan of `AdminCmd::SetParams` (D.11.9, A-21).
#[derive(Debug)]
pub(crate) struct SetParamsPlan {
    params: Params,
}

/// Validates `AdminCmd::SetParams(p)` under `SessionCtx::System` (D.11.8 step 6): `p` passes
/// `Params::valid`, and fits current usage: no session holds more live holds than a lowered
/// `hold.per_session` (H2), and no node lies deeper than a lowered `tree.max_depth` (L1). Every
/// other parameter is read through snapshots or by later commands only (A-102, A-103, CS-7, CS-8,
/// CS-10, A-114).
pub(crate) fn set_params(cx: &Vcx<'_>, cmd: &Cmd) -> Result<SetParamsPlan, Error> {
    let Cmd::Admin(AdminCmd::SetParams(p)) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    if !p.valid() {
        return Err(Error::invalid(Why::BadParams));
    }
    let over_holds = cx.s.ix.holds_by_session.values().any(|ts| {
        let live = ts
            .iter()
            .filter(|t| cx.s.holds.get(t).is_some_and(|h| cx.now < h.lease_until))
            .count();
        u32::try_from(live).unwrap_or(u32::MAX) > p.hold_per_session
    });
    let too_deep = cx.s.ix.node.values().any(|x| x.depth > p.tree_max_depth);
    if over_holds || too_deep {
        return Err(Error::invalid(Why::BadParams));
    }
    Ok(SetParamsPlan { params: p.clone() })
}

/// Executes `AdminCmd::SetParams`: `ParamsChanged` (no `Touched`, no `OpRecorded`).
pub(crate) fn set_params_exec(ex: &mut Exec<'_>, plan: SetParamsPlan) -> Reply {
    ex.emit(Ev::ParamsChanged {
        params: plan.params,
    });
    Reply::ParamsSet
}
