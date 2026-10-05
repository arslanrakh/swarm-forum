//! `session hello` (design D.8.23, D.8.11). Owner: WP-D3; its `from_user` is WP-D5a's
//! (`talk_user::hello_from_user`, D.13.1).
//!
//! A first call's `SessionOpened` carries `hello`'s arguments and is emitted by `apply` before the
//! validation (A-38); on an existing session `hello` emits `SessionHello` with what it gives,
//! after a `PrincipalSeen` for a new principal key (CS-1); then the events of its `from_user`, if
//! any (D.8.11, D.8.24). Then `InboxRead` for the notes it
//! delivers, when there are any: at most `INBOX_DELIVER_MAX` (10), oldest first (1.5, §13,
//! D.8.16); the rest wait for the next `hello` or `talk wait`.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::cmd::{Cmd, SessionCmd, SessionCtx};
use crate::error::{Error, Why};
use crate::event::Ev;
use crate::helpers::Exec;
use crate::helpers::principals::intern;
use crate::reply::Reply;
use crate::types::{Label, PrincipalKey, SessionId, Text};

use super::Vcx;

/// The plan of `session hello`.
#[derive(Debug)]
pub(crate) struct HelloPlan {
    /// The session.
    sess: SessionId,
    /// The call opened it: its `SessionOpened` already carries the arguments.
    new: bool,
    /// The arguments to set on an existing session.
    caps: Option<BTreeSet<Label>>,
    horizon: Option<u32>,
    intent: Option<Text>,
    principal: Option<PrincipalKey>,
    /// The `from_user` to log and route (WP-D5a).
    from_user: Option<super::talk_user::FromUserPlan>,
}

/// Validates `session hello` (D.8.23), and its `from_user` (D.8.11: `invalid(AmbiguousPrincipal)`
/// when neither it nor `hello` names a principal and the session serves several persons, and,
/// 1.9, `invalid(NoPrincipal)` when they name none and the session serves none; a first `hello`
/// that fails so still opens its session, 1.5).
pub(crate) fn hello(cx: &Vcx<'_>, cmd: &Cmd) -> Result<HelloPlan, Error> {
    let Cmd::Session(SessionCmd::Hello {
        intent,
        caps,
        horizon,
        principal,
        from_user,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let sess = cx.sess.ok_or(Error::invalid(Why::BadParams))?;
    let from_user = from_user
        .as_ref()
        .map(|f| super::talk_user::hello_from_user(cx, f, principal.as_ref()))
        .transpose()?;
    Ok(HelloPlan {
        sess,
        new: cx.ctx.session == SessionCtx::New,
        // 1.6 (§20.3): the caps it passes are added to the token's default caps.
        caps: caps
            .as_ref()
            .and_then(|c| crate::cmd::hello_caps(Some(c), cx.ctx.caps.as_ref())),
        horizon: *horizon,
        intent: intent.clone(),
        principal: principal.clone(),
        from_user,
    })
}

/// Executes `session hello`.
pub(crate) fn hello_exec(ex: &mut Exec<'_>, plan: HelloPlan) -> Reply {
    if !plan.new {
        let principal = plan.principal.as_ref().map(|k| intern(ex, k));
        ex.emit(Ev::SessionHello {
            session: plan.sess,
            caps: plan.caps,
            horizon: plan.horizon,
            intent: plan.intent,
            principal,
        });
    }
    let user_entry = plan
        .from_user
        .map(|f| super::talk_user::hello_from_user_exec(ex, f));
    let now = ex.now();
    let inbox: Vec<_> = crate::helpers::agents::deliverable(ex.s(), plan.sess, now);
    if !inbox.is_empty() {
        ex.emit(Ev::InboxRead {
            session: plan.sess,
            notes: inbox.iter().copied().collect(),
        });
    }
    Reply::Hello {
        session: plan.sess,
        user_entry,
        inbox,
    }
}
