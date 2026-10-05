//! The dispatcher (design D.13.1, D.8.1 steps 4 and 5): one validation and one execution per `Cmd`
//! variant, and one per `AskTo` arm, each in a named file owned by the package that implements it:
//!
//! | File | Commands | Owner |
//! |---|---|---|
//! | `cmd/session.rs` | `session hello` (its `from_user` through `talk_user`, WP-D5a's) | WP-D3 |
//! | `cmd/view.rs` | the `view` actions | WP-D3 |
//! | `cmd/node_basic.rs` | `node create` without keys and repeat, `update` without the occurrence consequences, `close`, and `hide` | WP-D3 |
//! | `cmd/keys.rs` | `node link` and `node key`, and create-or-join in `node create` | WP-D4 |
//! | `cmd/restructure.rs` | `node reopen`, `move`, and `merge` | WP-D4 |
//! | `cmd/hold.rs` | the `hold` actions | WP-D3 |
//! | `cmd/talk_user.rs` | `talk from_user`, `to_user`, `route`, and `ask` to a human; `hello {from_user}` | WP-D5a |
//! | `cmd/talk_people.rs` | `talk request`, `said`, its confirmation, `heard`, and `ask` to a person | WP-D5b |
//! | `cmd/talk_agents.rs` | `talk say`, `ask` to agents, `answer`, `wait`, `who`, and `session status` | WP-D5b |
//! | `cmd/review.rs` | `talk decide`, `approve`, `object`, `resolve`, and `retract` | WP-D5b |
//! | `cmd/control.rs` | the `control` actions and `AdminCmd::SetParams` | WP-D3 |
//!
//! `validate(cx, cmd) -> Result<Plan, Error>` is pure and reads only the state: the plan holds the
//! resolved ids, the chosen branch, and the warnings computed on the pre-state, so that execution
//! reads no unresolved reference. `execute(ex, plan)` emits the events through `Exec`, which applies
//! each at once, and cannot fail (D.0.2, principle 5). `apply` (WP-D3) runs the two around the
//! session, `Touched`, `late`, and `OpRecorded` steps of D.8.1.
//!
//! This file is the module `dispatch` of the crate: `src/cmd.rs` is the module `cmd` with the
//! command types (D.5.2), and Rust cannot load both as one module.

mod control;
mod hold;
mod keys;
mod node_basic;
mod restructure;
mod review;
mod session;
mod talk_agents;
mod talk_people;
mod talk_user;
mod view;

/// The tests of the 1.4 catch-up of WP-D1 to WP-D4 (WP-D1.4).
#[cfg(test)]
mod catch_up_tests;
/// The restore of an entity whose leaving time passed with no event since (D.4.12, WP-D6).
#[cfg(test)]
mod restore_tests;

/// The recurring work of WP-D6 (D.8.18).
#[cfg(test)]
mod recurring_tests;

/// WP-D6's cross-cutting flows (E1, D1, Settled, M1 with memory).
#[cfg(test)]
mod cross_tests;

/// 1.8's corrections, intents, and tidying (§5, §10.2, §11.3, §14).
#[cfg(test)]
mod tidy_tests;

/// 1.10's semantics: second verdicts, `no_basis`, a checkpoint that ends a wait, and an intent
/// under a pause (§10.1, §10.2, §11.3, §19).
#[cfg(test)]
mod semantics_tests;

/// 1.11's reviews and conversations: deferred reviews, a retracted decision kept, the voice by its
/// cap, a `said` sent at once, and a note at a closed node (§9.1, §9.2, §10.3, §11.3, §13).
#[cfg(test)]
mod review_conv_tests;

/// 1.12's finishing, notes, and questions: a release that settles, notes at a closed node, a
/// person nobody serves, a withdrawn question, a reopen that offers, and answers with nothing
/// asked (§8.4, §9.1, §9.2, §9.3, §10.3, §13, §16).
#[cfg(test)]
mod finish_tests;

use crate::cmd::{
    AdminCmd, AskTo, Cmd, ControlCmd, Ctx, HoldCmd, NodeCmd, SessionCmd, TalkCmd, ViewCmd,
};
use crate::error::Error;
use crate::helpers::Exec;
use crate::reply::Reply;
use crate::state::State;
use crate::types::{SessionId, Time};

/// What a validation reads: the state, the clamped `now`, the context, and the calling session.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vcx<'a> {
    /// The state before the command.
    pub(crate) s: &'a State,
    /// The command's clamped `now` (T1).
    pub(crate) now: Time,
    /// The call's context.
    pub(crate) ctx: &'a Ctx,
    /// The calling session: an existing one, or, for `SessionCtx::New`, the id its `SessionOpened`
    /// will carry (`counters.next_session`); None under `SessionCtx::System`.
    pub(crate) sess: Option<SessionId>,
}

/// One plan per command, and one per `AskTo` arm.
#[derive(Debug)]
pub(crate) enum Plan {
    /// `session hello`.
    SessionHello(session::HelloPlan),
    /// `session status`.
    SessionStatus(talk_agents::StatusPlan),
    /// `view node`.
    ViewNode(view::NodePlan),
    /// `view find`.
    ViewFind(view::FindPlan),
    /// `view feed`.
    ViewFeed(view::FeedPlan),
    /// `view list`.
    ViewList(view::ListPlan),
    /// `view agreements` (1.4).
    ViewAgreements(view::AgreementsPlan),
    /// `view history`.
    ViewHistory(view::HistoryPlan),
    /// `view requests`.
    ViewRequests(view::RequestsPlan),
    /// `view user_log`.
    ViewUserLog(view::UserLogPlan),
    /// `view constitution`.
    ViewConstitution(view::ConstitutionPlan),
    /// `node create` (create-or-join: WP-D4; the `repeat` consequences: WP-D6).
    NodeCreate(node_basic::CreatePlan),
    /// `node update` (the occurrence consequences: WP-D6).
    NodeUpdate(node_basic::UpdatePlan),
    /// `node link`.
    NodeLink(keys::LinkPlan),
    /// `node key`.
    NodeKey(keys::KeyPlan),
    /// `node close`.
    NodeClose(node_basic::ClosePlan),
    /// `node reopen`.
    NodeReopen(restructure::ReopenPlan),
    /// `node move`.
    NodeMove(restructure::MovePlan),
    /// `node merge`.
    NodeMerge(restructure::MergePlan),
    /// `node hide`.
    NodeHide(node_basic::HidePlan),
    /// `hold take`.
    HoldTake(hold::TakePlan),
    /// `hold next`.
    HoldNext(hold::NextPlan),
    /// `hold checkpoint`.
    HoldCheckpoint(hold::CheckpointPlan),
    /// `hold renew`.
    HoldRenew(hold::RenewPlan),
    /// `hold release`.
    HoldRelease(hold::ReleasePlan),
    /// `hold check`.
    HoldCheck(hold::CheckPlan),
    /// `talk from_user`.
    TalkFromUser(talk_user::FromUserPlan),
    /// `talk to_user`.
    TalkToUser(talk_user::ToUserPlan),
    /// `talk route`.
    TalkRoute(talk_user::RoutePlan),
    /// `talk request`.
    TalkRequest(talk_people::RequestPlan),
    /// `talk said`.
    TalkSaid(talk_people::SaidPlan),
    /// `talk said {said_id, confirmed}`.
    TalkSaidConfirm(talk_people::SaidConfirmPlan),
    /// `talk heard`.
    TalkHeard(talk_people::HeardPlan),
    /// `talk ask` to a person.
    TalkAskPerson(talk_people::AskPersonPlan),
    /// `talk ask` to agents.
    TalkAskAgent(talk_agents::AskAgentPlan),
    /// `talk ask` to a human the swarm serves.
    TalkAskUser(talk_user::AskUserPlan),
    /// `talk answer`.
    TalkAnswer(talk_agents::AnswerPlan),
    /// `talk say`.
    TalkSay(talk_agents::SayPlan),
    /// `talk decide`.
    TalkDecide(review::DecidePlan),
    /// `talk approve`.
    TalkApprove(review::ApprovePlan),
    /// `talk object`.
    TalkObject(review::ObjectPlan),
    /// `talk resolve`.
    TalkResolve(review::ResolvePlan),
    /// `talk retract`.
    TalkRetract(review::RetractPlan),
    /// `talk wait`.
    TalkWait(talk_agents::WaitPlan),
    /// `talk who`.
    TalkWho(talk_agents::WhoPlan),
    /// `control pause`.
    ControlPause(control::PausePlan),
    /// `control unpause`.
    ControlUnpause(control::UnpausePlan),
    /// `control health`.
    ControlHealth(control::HealthPlan),
    /// `control constitution`.
    ControlConstitution(control::ConstitutionPlan),
    /// `control export`.
    ControlExport(control::ExportPlan),
    /// `AdminCmd::SetParams`, under `SessionCtx::System` only.
    AdminSetParams(control::SetParamsPlan),
}

/// Step 4 of D.8.1: validates `cmd` against `cx.s`, in the order of D.8.24.
pub(crate) fn validate(cx: &Vcx<'_>, cmd: &Cmd) -> Result<Plan, Error> {
    match cmd {
        Cmd::Session(SessionCmd::Hello { .. }) => session::hello(cx, cmd).map(Plan::SessionHello),
        Cmd::Session(SessionCmd::Status { .. }) => {
            talk_agents::status(cx, cmd).map(Plan::SessionStatus)
        }
        Cmd::View(ViewCmd::Node { .. }) => view::node(cx, cmd).map(Plan::ViewNode),
        Cmd::View(ViewCmd::Find { .. }) => view::find(cx, cmd).map(Plan::ViewFind),
        Cmd::View(ViewCmd::Feed { .. }) => view::feed(cx, cmd).map(Plan::ViewFeed),
        Cmd::View(ViewCmd::List { .. }) => view::list(cx, cmd).map(Plan::ViewList),
        Cmd::View(ViewCmd::Agreements { .. }) => {
            view::agreements(cx, cmd).map(Plan::ViewAgreements)
        }
        Cmd::View(ViewCmd::History { .. }) => view::history(cx, cmd).map(Plan::ViewHistory),
        Cmd::View(ViewCmd::Requests { .. }) => view::requests(cx, cmd).map(Plan::ViewRequests),
        Cmd::View(ViewCmd::UserLog { .. }) => view::user_log(cx, cmd).map(Plan::ViewUserLog),
        Cmd::View(ViewCmd::Constitution) => view::constitution(cx, cmd).map(Plan::ViewConstitution),
        Cmd::Node(NodeCmd::Create(_)) => node_basic::create(cx, cmd).map(Plan::NodeCreate),
        Cmd::Node(NodeCmd::Update { .. }) => node_basic::update(cx, cmd).map(Plan::NodeUpdate),
        Cmd::Node(NodeCmd::Link { .. }) => keys::link(cx, cmd).map(Plan::NodeLink),
        Cmd::Node(NodeCmd::Key { .. }) => keys::key(cx, cmd).map(Plan::NodeKey),
        Cmd::Node(NodeCmd::Close { .. }) => node_basic::close(cx, cmd).map(Plan::NodeClose),
        Cmd::Node(NodeCmd::Reopen { .. }) => restructure::reopen(cx, cmd).map(Plan::NodeReopen),
        Cmd::Node(NodeCmd::Move { .. }) => restructure::move_node(cx, cmd).map(Plan::NodeMove),
        Cmd::Node(NodeCmd::Merge { .. }) => restructure::merge(cx, cmd).map(Plan::NodeMerge),
        Cmd::Node(NodeCmd::Hide { .. }) => node_basic::hide(cx, cmd).map(Plan::NodeHide),
        Cmd::Hold(HoldCmd::Take { .. }) => hold::take(cx, cmd).map(Plan::HoldTake),
        Cmd::Hold(HoldCmd::Next { .. }) => hold::next(cx, cmd).map(Plan::HoldNext),
        Cmd::Hold(HoldCmd::Checkpoint { .. }) => {
            hold::checkpoint(cx, cmd).map(Plan::HoldCheckpoint)
        }
        Cmd::Hold(HoldCmd::Renew { .. }) => hold::renew(cx, cmd).map(Plan::HoldRenew),
        Cmd::Hold(HoldCmd::Release { .. }) => hold::release(cx, cmd).map(Plan::HoldRelease),
        Cmd::Hold(HoldCmd::Check { .. }) => hold::check(cx, cmd).map(Plan::HoldCheck),
        Cmd::Talk(TalkCmd::FromUser { .. }) => {
            talk_user::from_user(cx, cmd).map(Plan::TalkFromUser)
        }
        Cmd::Talk(TalkCmd::ToUser { .. }) => talk_user::to_user(cx, cmd).map(Plan::TalkToUser),
        Cmd::Talk(TalkCmd::Route { .. }) => talk_user::route(cx, cmd).map(Plan::TalkRoute),
        Cmd::Talk(TalkCmd::Request { .. }) => talk_people::request(cx, cmd).map(Plan::TalkRequest),
        Cmd::Talk(TalkCmd::Said { .. }) => talk_people::said(cx, cmd).map(Plan::TalkSaid),
        Cmd::Talk(TalkCmd::SaidConfirm { .. }) => {
            talk_people::said_confirm(cx, cmd).map(Plan::TalkSaidConfirm)
        }
        Cmd::Talk(TalkCmd::Heard { .. }) => talk_people::heard(cx, cmd).map(Plan::TalkHeard),
        Cmd::Talk(TalkCmd::Ask {
            to: AskTo::Person { .. },
            ..
        }) => talk_people::ask_person(cx, cmd).map(Plan::TalkAskPerson),
        Cmd::Talk(TalkCmd::Ask {
            to: AskTo::Agent(_),
            ..
        }) => talk_agents::ask_agent(cx, cmd).map(Plan::TalkAskAgent),
        Cmd::Talk(TalkCmd::Ask {
            to: AskTo::User(_), ..
        }) => talk_user::ask_user(cx, cmd).map(Plan::TalkAskUser),
        Cmd::Talk(TalkCmd::Answer { .. }) => talk_agents::answer(cx, cmd).map(Plan::TalkAnswer),
        Cmd::Talk(TalkCmd::Say { .. }) => talk_agents::say(cx, cmd).map(Plan::TalkSay),
        Cmd::Talk(TalkCmd::Decide { .. }) => review::decide(cx, cmd).map(Plan::TalkDecide),
        Cmd::Talk(TalkCmd::Approve { .. }) => review::approve(cx, cmd).map(Plan::TalkApprove),
        Cmd::Talk(TalkCmd::Object { .. }) => review::object(cx, cmd).map(Plan::TalkObject),
        Cmd::Talk(TalkCmd::Resolve { .. }) => review::resolve(cx, cmd).map(Plan::TalkResolve),
        Cmd::Talk(TalkCmd::Retract { .. }) => review::retract(cx, cmd).map(Plan::TalkRetract),
        Cmd::Talk(TalkCmd::Wait { .. }) => talk_agents::wait(cx, cmd).map(Plan::TalkWait),
        Cmd::Talk(TalkCmd::Who { .. }) => talk_agents::who(cx, cmd).map(Plan::TalkWho),
        Cmd::Control(ControlCmd::Pause { .. }) => control::pause(cx, cmd).map(Plan::ControlPause),
        Cmd::Control(ControlCmd::Unpause { .. }) => {
            control::unpause(cx, cmd).map(Plan::ControlUnpause)
        }
        Cmd::Control(ControlCmd::Health) => control::health(cx, cmd).map(Plan::ControlHealth),
        Cmd::Control(ControlCmd::Constitution) => {
            control::constitution(cx, cmd).map(Plan::ControlConstitution)
        }
        Cmd::Control(ControlCmd::Export { .. }) => {
            control::export(cx, cmd).map(Plan::ControlExport)
        }
        Cmd::Admin(AdminCmd::SetParams(_)) => {
            control::set_params(cx, cmd).map(Plan::AdminSetParams)
        }
    }
}

/// `validate` with the tie-break of `next` by id instead of the ring (`apply_with(.., TieBreak::Id)`,
/// D.6.1, D.8.5): only with the test-only feature `conformance`.
#[cfg(feature = "conformance")]
pub(crate) fn validate_by_id(cx: &Vcx<'_>, cmd: &Cmd) -> Result<Plan, Error> {
    if let Cmd::Hold(HoldCmd::Next { .. }) = cmd {
        hold::next_by_id(cx, cmd).map(Plan::HoldNext)
    } else {
        validate(cx, cmd)
    }
}

/// Step 5 of D.8.1: executes a plan, emitting its events through `ex`.
pub(crate) fn execute(ex: &mut Exec<'_>, plan: Plan) -> Reply {
    match plan {
        Plan::SessionHello(p) => session::hello_exec(ex, p),
        Plan::SessionStatus(p) => talk_agents::status_exec(ex, p),
        Plan::ViewNode(p) => view::node_exec(ex, p),
        Plan::ViewFind(p) => view::find_exec(ex, p),
        Plan::ViewFeed(p) => view::feed_exec(ex, p),
        Plan::ViewList(p) => view::list_exec(ex, p),
        Plan::ViewAgreements(p) => view::agreements_exec(ex, p),
        Plan::ViewHistory(p) => view::history_exec(ex, p),
        Plan::ViewRequests(p) => view::requests_exec(ex, p),
        Plan::ViewUserLog(p) => view::user_log_exec(ex, p),
        Plan::ViewConstitution(p) => view::constitution_exec(ex, p),
        Plan::NodeCreate(p) => node_basic::create_exec(ex, p),
        Plan::NodeUpdate(p) => node_basic::update_exec(ex, p),
        Plan::NodeLink(p) => keys::link_exec(ex, p),
        Plan::NodeKey(p) => keys::key_exec(ex, p),
        Plan::NodeClose(p) => node_basic::close_exec(ex, p),
        Plan::NodeReopen(p) => restructure::reopen_exec(ex, p),
        Plan::NodeMove(p) => restructure::move_node_exec(ex, p),
        Plan::NodeMerge(p) => restructure::merge_exec(ex, p),
        Plan::NodeHide(p) => node_basic::hide_exec(ex, p),
        Plan::HoldTake(p) => hold::take_exec(ex, p),
        Plan::HoldNext(p) => hold::next_exec(ex, p),
        Plan::HoldCheckpoint(p) => hold::checkpoint_exec(ex, p),
        Plan::HoldRenew(p) => hold::renew_exec(ex, p),
        Plan::HoldRelease(p) => hold::release_exec(ex, p),
        Plan::HoldCheck(p) => hold::check_exec(ex, p),
        Plan::TalkFromUser(p) => talk_user::from_user_exec(ex, p),
        Plan::TalkToUser(p) => talk_user::to_user_exec(ex, p),
        Plan::TalkRoute(p) => talk_user::route_exec(ex, p),
        Plan::TalkRequest(p) => talk_people::request_exec(ex, p),
        Plan::TalkSaid(p) => talk_people::said_exec(ex, p),
        Plan::TalkSaidConfirm(p) => talk_people::said_confirm_exec(ex, p),
        Plan::TalkHeard(p) => talk_people::heard_exec(ex, p),
        Plan::TalkAskPerson(p) => talk_people::ask_person_exec(ex, p),
        Plan::TalkAskAgent(p) => talk_agents::ask_agent_exec(ex, p),
        Plan::TalkAskUser(p) => talk_user::ask_user_exec(ex, p),
        Plan::TalkAnswer(p) => talk_agents::answer_exec(ex, p),
        Plan::TalkSay(p) => talk_agents::say_exec(ex, p),
        Plan::TalkDecide(p) => review::decide_exec(ex, p),
        Plan::TalkApprove(p) => review::approve_exec(ex, p),
        Plan::TalkObject(p) => review::object_exec(ex, p),
        Plan::TalkResolve(p) => review::resolve_exec(ex, p),
        Plan::TalkRetract(p) => review::retract_exec(ex, p),
        Plan::TalkWait(p) => talk_agents::wait_exec(ex, p),
        Plan::TalkWho(p) => talk_agents::who_exec(ex, p),
        Plan::ControlPause(p) => control::pause_exec(ex, p),
        Plan::ControlUnpause(p) => control::unpause_exec(ex, p),
        Plan::ControlHealth(p) => control::health_exec(ex, p),
        Plan::ControlConstitution(p) => control::constitution_exec(ex, p),
        Plan::ControlExport(p) => control::export_exec(ex, p),
        Plan::AdminSetParams(p) => control::set_params_exec(ex, p),
    }
}
