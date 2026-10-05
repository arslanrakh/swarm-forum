//! `node create` (its create branch), `update`, `close`, and `hide` (design D.8.9, D.8.20).
//! Owner: WP-D3.
//!
//! Later packages own parts of these commands (D.13.1): the join branch of create-or-join is
//! WP-D4's (`cmd/keys.rs`), and the consequences of `repeat` (a series' occurrences and the
//! coverage warning, at create and update) are WP-D6's (`crate::recurring`).
//!
//! Every validation follows D.8.24's order: the target, the token (`stale_token`, H4), the shape
//! (`invalid`), the other references (`not_found`), the versions (`stale_version`), state
//! conflicts (`closed`, `cycle`, `invalid` by state), resources (`full`), and the safety valves
//! (`limit`).

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::apply::{check_token, node_of as resolve_ref, target};
use crate::cmd::{Cmd, CreateArgs, HideArg, NodeCmd, Patch, RepeatArg};
use crate::error::{Error, Field, Hint, LimitKind, What, Why};
use crate::event::{
    ConfirmVia, EndCause, EntryBody, EntryText, Ev, HideTarget, NodeInit, NodePatch, SystemNote,
    TextField,
};
use crate::helpers::Exec;
use crate::helpers::close::{Closing, assume_marks_off, close_node};
use crate::helpers::conversation::{
    conv_node, default_settles, record_request, route_to, try_settle,
};
use crate::helpers::holds::{
    bind_holder_notes, issue_hold, live_holds_of, paused_by, slot_free, under_limit,
};
use crate::helpers::principals::intern;
use crate::helpers::tree::reaches;
use crate::helpers::user::e1_check;
use crate::reply::{
    NoHoldWhy, NotSettled, NotSettledWhy, Reply, Warning, WarningCode, WarningDetail,
};
use crate::state::{OccPhase, Origin, Repeat, Role, State};
use crate::types::{
    Change, DecisionId, Key, KeyStr, Label, LinkRel, Name, NodeId, NodeRef, Outcome, PEOPLE,
    PrincipalKey, RequestId, SaidId, SessionId, TEAMS, Text, Time, UserEntryId,
};

use super::Vcx;
use super::hold::confirm_pending;
use super::view::show;

/// Resolves a list of node references, or `not_found(Node)`.
fn resolve_all(s: &State, refs: &[NodeRef]) -> Result<BTreeSet<NodeId>, Error> {
    refs.iter().map(|r| resolve_ref(s, r)).collect()
}

/// The shape checks of a `repeat` (D.8.9, D.8.20): `run_within > 0`, and a valid frozen tz table.
fn repeat_shape(r: &RepeatArg) -> Result<(), Error> {
    if r.run_within.0 == 0 {
        return Err(Error::invalid(Why::RunWithinZero));
    }
    if !forum_cron::valid_table(&r.tz) {
        return Err(Error::invalid(Why::BadTimeZone));
    }
    Ok(())
}

/// A schedule with a slot after `now` within `forum_cron::MAX_SEARCH_DAYS` (§12).
fn fires(r: &RepeatArg, now: Time) -> bool {
    forum_cron::next_slot(&r.cron, &r.tz, now.0).is_some()
}

/// The zone's offsets from `now` on: the offset in force at `now` and every transition after it.
fn offsets_from(tz: &forum_cron::TzTable, now: Time) -> (i32, Vec<(i64, i32)>) {
    let s = i64::try_from(now.0.checked_div(1_000).unwrap_or(0)).unwrap_or(i64::MAX);
    (
        forum_cron::offset_at(tz, s),
        tz.transitions
            .iter()
            .filter(|t| t.at > s)
            .map(|t| (t.at, t.offset))
            .collect(),
    )
}

/// Whether `new` changes the schedule `old` (A-75): its meaning, not its stored value: the parsed
/// cron, `run_within`, the zone's name, and the zone's offsets from `now` on.
fn schedule_differs(old: &Repeat, new: &RepeatArg, now: Time) -> bool {
    old.cron != new.cron
        || old.run_within != new.run_within
        || old.tz.name != new.tz.name
        || offsets_from(&old.tz, now) != offsets_from(&new.tz, now)
}

/// Whether a `repeat` change changes the node's role or schedule (A-75).
fn repeat_changes(role: &Role, r: &Change<RepeatArg>, now: Time) -> bool {
    let old = match role {
        Role::Series(x) => Some(&x.repeat),
        Role::Plain | Role::Occurrence(_) | Role::Signal(_) => None,
    };
    match r {
        Change::Keep => false,
        Change::Clear => old.is_some(),
        Change::Set(new) => old.is_none_or(|o| schedule_differs(o, new, now)),
    }
}

// ------------------------------------------------------------------------------------------------
// create

/// Whether `create` issues a hold, and if not, why (D.8.9 step 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldChoice {
    /// `take: false`: no hold, no tag, no warning.
    None,
    /// The hold is issued.
    Issue,
    /// No hold; the `offer` tag and `no_hold(HoldsLimit | Paused)`.
    Offer(NoHoldWhy),
    /// No hold, no tag; `no_hold(NoCapacity)` (A-65).
    NoCapacity,
}

/// The request a `create` records (A-52, A-61), in either branch.
#[derive(Clone, Debug)]
pub(super) struct NewRequest {
    /// The message it came in.
    pub(super) origin: Key,
    /// Its conversation.
    pub(super) where_: Key,
    /// The request and its requester.
    pub(super) text: EntryText,
}

/// The plan of `node create` (D.8.9): a new node, or a join of an open carrier (WP-D4's branch,
/// `cmd/keys.rs`); boxed, since a create carries a whole `NodeInit`.
#[derive(Debug)]
pub(crate) enum CreatePlan {
    /// The create branch.
    New(Box<Create>),
    /// The join branch.
    Join(Box<super::keys::JoinPlan>),
}

/// What `node create` creates.
#[derive(Debug)]
pub(crate) struct Create {
    /// The creating session.
    sess: SessionId,
    /// The existing node the missing steps hang from.
    parent: NodeId,
    /// The missing ancestors' names, outermost first, and their `needs` (CS-26).
    ancestors: Vec<Name>,
    ancestor_needs: BTreeSet<Label>,
    name: Name,
    init: NodeInit,
    /// `person:` keys whose principal is new (CS-1).
    persons: Vec<PrincipalKey>,
    /// Unrouted user entries named by `user:e<n>` keys.
    route: Vec<UserEntryId>,
    request: Option<NewRequest>,
    hold: HoldChoice,
    preds: Vec<NodeId>,
    /// 1.9 (§20.5): the labels of the call's `needs` that no session can meet now
    /// (`unmet_needs`).
    unmet: Vec<Label>,
}

impl CreatePlan {
    /// Whether the create issues a hold (a join never does).
    pub(crate) fn issues_hold(&self) -> bool {
        matches!(self, CreatePlan::New(c) if c.hold == HoldChoice::Issue)
    }
}

/// The keys of a create, deduplicated by `KeyStr`, the first kept (D.8.9 step 1).
fn dedupe(keys: &[Key]) -> Vec<Key> {
    let mut seen: BTreeSet<&KeyStr> = BTreeSet::new();
    keys.iter()
        .filter(|k| seen.insert(&k.key))
        .cloned()
        .collect()
}

/// The request of a create, with its defaults (D.8.9 step 2): `origin := origin`, else `where`,
/// else the first conversation key among `keys`, else the first key (`invalid(NoRequestOrigin)`
/// without one); `where := where`, else that conversation key, else `origin`; a `repeat:` origin or
/// `where` is `invalid(ReservedKey)` (A-90).
fn request_of(s: &State, a: &CreateArgs, keys: &[Key]) -> Result<Option<NewRequest>, Error> {
    let Some(req) = &a.request else {
        return Ok(None);
    };
    let k0 = keys
        .iter()
        .find(|k| k.key.is_conversation(&s.params.conversation_schemes));
    let origin = req
        .origin
        .clone()
        .or_else(|| req.where_.clone())
        .or_else(|| k0.cloned())
        .or_else(|| keys.first().cloned())
        .ok_or(Error::invalid(Why::NoRequestOrigin))?;
    let where_ = req
        .where_
        .clone()
        .or_else(|| k0.cloned())
        .unwrap_or_else(|| origin.clone());
    if origin.key.is_reserved() || where_.key.is_reserved() {
        return Err(Error::invalid(Why::ReservedKey));
    }
    Ok(Some(NewRequest {
        origin,
        where_,
        text: EntryText {
            main: req.text.clone(),
            extra: alloc::vec![(TextField::Requester, req.requester.clone())],
        },
    }))
}

/// Validates `node create` (D.8.9): the keys and the request, in both branches, in the order of
/// D.8.9's steps 1 and 2 and D.8.24's row: a `repeat:` key (`invalid(ReservedKey)`), a `user:e<n>`
/// key naming no user entry (`not_found(UserEntry)`), then the request (`invalid(NoRequestOrigin)`,
/// and `invalid(ReservedKey)` for a `repeat:` origin or `where`); a key that an open node carries
/// makes it a join (WP-D4's); then the create: `invalid(EmptyText(Title))`, `invalid(DigestTooLong)`,
/// `invalid(RunWithinZero)`, `invalid(BadTimeZone)` → `not_found(Node)` for `depends_on` and
/// `owners` → `invalid(NameTaken)`, `invalid(ScheduleNeverFires)` → `limit(Depth)`,
/// `limit(Children)` (L1; no child limit under `@people` or `@teams`, A-113).
#[allow(
    clippy::too_many_lines,
    reason = "one validation in D.8.24's order, the steps of D.8.9"
)]
pub(crate) fn create(cx: &Vcx<'_>, cmd: &Cmd) -> Result<CreatePlan, Error> {
    let Cmd::Node(NodeCmd::Create(a)) = cmd else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let sess = cx.sess.ok_or(Error::invalid(Why::BadParams))?;
    // Both branches: the keys (D.8.9 step 1: a `repeat:` key, then the user entries they name),
    // then the request (step 2).
    let keys = dedupe(&a.keys);
    if keys.iter().any(|k| k.key.is_reserved()) {
        return Err(Error::invalid(Why::ReservedKey));
    }
    let mut route = Vec::new();
    for k in &keys {
        if let Some(e) = k.key.user_entry() {
            let u = s
                .user_entries
                .get(&e)
                .ok_or(Error::not_found(What::UserEntry))?;
            if u.routing == crate::state::Routing::Unrouted {
                route.push(e);
            }
        }
    }
    let request = request_of(s, a, &keys)?;
    // A key an open node carries: a join, WP-D4's branch (D.8.9 step 3, D.13.1).
    if let Some(j) = super::keys::join(s, &keys, request.as_ref()) {
        return Ok(CreatePlan::Join(Box::new(j)));
    }
    // The create: shape.
    if a.title.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::Title)));
    }
    if a.digest
        .as_ref()
        .is_some_and(|d| d.byte_len() > u64::from(s.params.digest_max))
    {
        return Err(Error::invalid(Why::DigestTooLong));
    }
    if let Some(r) = &a.repeat {
        repeat_shape(r)?;
    }
    // References.
    let depends_on = resolve_all(s, &a.depends_on)?;
    let owners = resolve_all(s, &a.owners)?;
    // State: the place, and the schedule.
    let Some((name, parent_steps)) = a.path.0.split_last() else {
        return Err(Error::invalid(Why::NameTaken));
    };
    let mut parent = crate::types::ROOT;
    let mut ancestors: Vec<Name> = Vec::new();
    for step in parent_steps {
        if ancestors.is_empty() {
            let key = (parent, step.clone());
            if let Some(&m) = s.names.get(&key).or_else(|| s.aliases.get(&key)) {
                parent = m;
                continue;
            }
        }
        ancestors.push(step.clone());
    }
    if ancestors.is_empty() && s.names.contains_key(&(parent, name.clone())) {
        return Err(Error::invalid(Why::NameTaken));
    }
    if let Some(r) = &a.repeat
        && !fires(r, cx.now)
    {
        return Err(Error::invalid(Why::ScheduleNeverFires));
    }
    // Safety valves (L1): the deepest new node, a series' occurrences one level below it (D.14.5),
    // and the existing parent's open agent children.
    let depth = s.ix.node.get(&parent).map_or(0, |x| x.depth);
    let new_nodes = u32::try_from(ancestors.len())
        .unwrap_or(u32::MAX)
        .saturating_add(1)
        .saturating_add(u32::from(a.repeat.is_some()));
    if depth.saturating_add(new_nodes) > s.params.tree_max_depth {
        return Err(Error::limit(LimitKind::Depth));
    }
    let children = s.ix.node.get(&parent).map_or(0, |x| x.open_agent_children);
    if parent != PEOPLE
        && parent != TEAMS
        && children.saturating_add(1) > s.params.tree_max_children
    {
        return Err(Error::limit(LimitKind::Children));
    }
    // The plan.
    let parent_needs = s
        .nodes
        .get(&parent)
        .map(|x| x.needs.clone())
        .unwrap_or_default();
    let mut persons: Vec<PrincipalKey> = Vec::new();
    for k in &keys {
        if let Some(p) = k.key.person()
            && !s.principal_ids.contains_key(&p)
            && !persons.contains(&p)
        {
            persons.push(p);
        }
    }
    let mut preds: Vec<NodeId> = Vec::new();
    for k in &keys {
        if let Some(&p) = s.ix.keys_all.get(&k.key).and_then(|x| x.last())
            && !preds.contains(&p)
        {
            preds.push(p);
        }
    }
    let capacity = a.capacity.unwrap_or(1);
    let take = a.take.unwrap_or(true);
    let paused = paused_by(s, parent, cx.now).is_some();
    let full = !under_limit(
        u32::try_from(live_holds_of(s, sess, cx.now).len()).unwrap_or(u32::MAX),
        s.params.hold_per_session,
    );
    let hold = match (take, slot_free(0, capacity)) {
        (false, _) => HoldChoice::None,
        (true, false) => HoldChoice::NoCapacity,
        (true, true) if paused => HoldChoice::Offer(NoHoldWhy::Paused),
        (true, true) if full => HoldChoice::Offer(NoHoldWhy::HoldsLimit),
        (true, true) => HoldChoice::Issue,
    };
    let init = NodeInit {
        title: a.title.clone(),
        body: a.body.clone().unwrap_or_default(),
        fields: a.fields.clone(),
        status: Text::default(),
        digest: a.digest.clone().unwrap_or_default(),
        agreements: a.agreements.clone().unwrap_or_default(),
        kind: a.kind.clone(),
        tags: a.tags.clone(),
        capacity,
        priority: a.priority.unwrap_or(0),
        est: a.est,
        needs: a.needs.clone().unwrap_or_else(|| parent_needs.clone()),
        due: a.due.map(|w| w.resolve(cx.now)),
        depends_on,
        owners,
        keys: keys.iter().map(|k| (k.key.clone(), k.nv)).collect(),
        links: preds.iter().map(|&p| (LinkRel::Continues, p)).collect(),
        role: a
            .repeat
            .as_ref()
            .map_or(Role::Plain, crate::recurring::series_role),
    };
    Ok(CreatePlan::New(Box::new(Create {
        sess,
        parent,
        ancestors,
        ancestor_needs: parent_needs,
        name: name.clone(),
        init,
        persons,
        route,
        request,
        hold,
        preds,
        unmet: a.needs.as_ref().map_or_else(Vec::new, |n| {
            crate::helpers::holds::unmet_needs(s, cx.now, Some(sess), cx.ctx.caps.as_ref(), n)
        }),
    })))
}

/// A missing ancestor (§4, D.8.4): capacity 1, title its name, no tags, no hold, its parent's
/// needs (CS-26). `node move` makes them too (D.8.20).
pub(super) fn ancestor_init(name: &Name, needs: &BTreeSet<Label>) -> NodeInit {
    NodeInit {
        title: Text(alloc::string::String::from(name.as_str())),
        body: Text::default(),
        fields: BTreeMap::new(),
        status: Text::default(),
        digest: Text::default(),
        agreements: Text::default(),
        kind: None,
        tags: BTreeSet::new(),
        capacity: 1,
        priority: 0,
        est: None,
        needs: needs.clone(),
        due: None,
        depends_on: BTreeSet::new(),
        owners: BTreeSet::new(),
        keys: BTreeMap::new(),
        links: BTreeSet::new(),
        role: Role::Plain,
    }
}

/// Executes `node create` (D.8.9): a join is WP-D4's (`keys::join_exec`); the create branch makes
/// the new principals, the missing ancestors, the node with its `continues` links, the routing of
/// its unrouted user entries, its request, the hold or the `offer` tag, the `unmet_needs` warning
/// for needs that no session can meet now (1.9), and `Shown`.
pub(crate) fn create_exec(ex: &mut Exec<'_>, plan: CreatePlan) -> Reply {
    let plan = match plan {
        CreatePlan::New(c) => *c,
        CreatePlan::Join(j) => return super::keys::join_exec(ex, *j),
    };
    let sess = plan.sess;
    for p in &plan.persons {
        intern(ex, p);
    }
    let mut parent = plan.parent;
    for name in &plan.ancestors {
        let id = ex.s().counters.next_node;
        ex.emit(Ev::NodeCreated {
            id,
            parent,
            name: name.clone(),
            origin: Origin::Ancestor(sess),
            init: ancestor_init(name, &plan.ancestor_needs),
        });
        parent = id;
    }
    let id = ex.s().counters.next_node;
    let series = matches!(plan.init.role, Role::Series(_));
    ex.emit(Ev::NodeCreated {
        id,
        parent,
        name: plan.name,
        origin: Origin::Agent(sess),
        init: plan.init,
    });
    // Step 3 (WP-D6): a series' first occurrence and the coverage warning.
    if series {
        crate::recurring::series_created(ex, id);
    }
    // 1.4 (R3): the requests the routings of its `user:e<n>` keys record, ascending.
    let mut user_requests: Vec<RequestId> = plan
        .route
        .into_iter()
        .filter_map(|e| route_to(ex, e, id))
        .collect();
    user_requests.sort_unstable();
    let mut request = None;
    if let Some(r) = plan.request
        && let Ok(c) = conv_node(ex.s(), id, &r.where_.key)
    {
        if c.add_key {
            ex.emit(Ev::KeyAdded {
                node: c.node,
                key: r.where_.clone(),
            });
        }
        let (rid, existed) = record_request(ex, c.node, r.origin, r.where_.key, None, r.text);
        if !existed {
            request = Some(rid);
        }
    }
    let (mut token, mut lease_until) = (None, None);
    match plan.hold {
        HoldChoice::None => {}
        HoldChoice::Issue => {
            // A new node reopens nothing (1.4).
            let (t, l) = issue_hold(ex, id, None, None);
            bind_holder_notes(ex, id);
            token = Some(t);
            lease_until = Some(l);
        }
        HoldChoice::Offer(why) => {
            // The tag, unless the create's own tags carry it already.
            let tagged = ex
                .s()
                .nodes
                .get(&id)
                .is_some_and(|x| x.tags.iter().any(|t| t.as_str() == crate::types::OFFER));
            if !tagged {
                ex.emit(Ev::OfferSet { node: id, on: true });
            }
            no_hold(ex, id, why);
        }
        HoldChoice::NoCapacity => no_hold(ex, id, NoHoldWhy::NoCapacity),
    }
    unmet_needs_warning(ex, id, plan.unmet);
    show(ex, id);
    Reply::Created {
        id,
        token,
        lease_until,
        continues: plan.preds,
        request,
        user_requests,
    }
}

/// 1.9 (§20.5, §21): the `unmet_needs` warning of a `create` or an `update` whose needs name labels
/// that no session can meet now; none when there are none.
fn unmet_needs_warning(ex: &mut Exec<'_>, n: NodeId, needs: Vec<Label>) {
    if needs.is_empty() {
        return;
    }
    ex.warn(Warning {
        code: WarningCode::UnmetNeeds,
        id: Some(n),
        detail: WarningDetail::UnmetNeeds { needs, take: false },
    });
}

fn no_hold(ex: &mut Exec<'_>, n: NodeId, why: NoHoldWhy) {
    ex.warn(Warning {
        code: WarningCode::NoHold,
        id: Some(n),
        detail: WarningDetail::NoHold { why },
    });
}

// ------------------------------------------------------------------------------------------------
// update

/// The plan of `node update` (D.8.20).
#[derive(Debug)]
pub(crate) struct UpdatePlan {
    n: NodeId,
    /// None: no field differs, no event (O2). Boxed: a whole patch.
    patch: Option<Box<NodePatch>>,
    /// What the update does to the schedule (A-75), for its occurrence consequences (WP-D6).
    repeat: Option<crate::recurring::RepeatChange>,
    /// 1.9 (§20.5): the labels of `needs_add` that no session can meet now (`unmet_needs`).
    unmet: Vec<Label>,
}

/// `(set − remove) ∪ add`: removals first, then additions (CS-34).
fn relabel(
    set: &BTreeSet<Label>,
    add: &BTreeSet<Label>,
    remove: &BTreeSet<Label>,
) -> BTreeSet<Label> {
    let mut out: BTreeSet<Label> = set.difference(remove).cloned().collect();
    out.extend(add.iter().cloned());
    out
}

/// A field of the patch: `Some(v)` when `v` is given and differs from the node's.
fn differs<T: PartialEq + Clone>(given: Option<&T>, current: &T) -> Option<T> {
    given.filter(|v| *v != current).cloned()
}

/// A clearable field of the patch: `Set` when given and different, `Clear` when cleared and set.
fn change<T: PartialEq + Clone>(given: &Change<T>, current: Option<&T>) -> Change<T> {
    match given {
        Change::Keep => Change::Keep,
        Change::Clear if current.is_some() => Change::Clear,
        Change::Clear => Change::Keep,
        Change::Set(v) if current != Some(v) => Change::Set(v.clone()),
        Change::Set(_) => Change::Keep,
    }
}

/// Validates `node update {path, version, digest_version?, token?, …}` (D.8.20): the node →
/// `stale_token` → `invalid(EmptyText(Title))`, `invalid(DigestTooLong)`,
/// `invalid(MissingDigestVersion)` (CS-11), `invalid(RunWithinZero)`, `invalid(BadTimeZone)` →
/// `not_found(Node)` for `depends_on` and `owners` → `stale_version` (O2, A-4) →
/// `invalid(ReservedNode)` for `capacity` or `repeat` on a reserved node, `invalid(RoleConflict)`,
/// `invalid(ScheduleNeverFires)`, `cycle` for a new dependency → `full` for a capacity below the
/// live holds (H1) → `limit(Depth)` for the occurrence of a new schedule (L1, D.14.5).
#[allow(
    clippy::too_many_lines,
    reason = "one validation in D.8.24's order, the steps of D.8.20"
)]
pub(crate) fn update(cx: &Vcx<'_>, cmd: &Cmd) -> Result<UpdatePlan, Error> {
    let Cmd::Node(NodeCmd::Update {
        version,
        digest_version,
        token,
        patch,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let n = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    if let Some(t) = token {
        check_token(cx, *t, n)?;
    }
    let p: &Patch = patch;
    // Shape.
    if p.title.as_ref().is_some_and(Text::is_empty) {
        return Err(Error::invalid(Why::EmptyText(Field::Title)));
    }
    if p.digest
        .as_ref()
        .is_some_and(|d| d.byte_len() > u64::from(s.params.digest_max))
    {
        return Err(Error::invalid(Why::DigestTooLong));
    }
    if p.digest.is_some() && digest_version.is_none() {
        return Err(Error::invalid(Why::MissingDigestVersion));
    }
    if let Change::Set(r) = &p.repeat {
        repeat_shape(r)?;
    }
    // References.
    let depends_on = p
        .depends_on
        .as_ref()
        .map(|r| resolve_all(s, r))
        .transpose()?;
    let owners = p.owners.as_ref().map(|r| resolve_all(s, r)).transpose()?;
    // Versions (O2).
    let Some(node) = s.nodes.get(&n) else {
        return Err(Error::not_found(What::Node));
    };
    if *version != node.version || digest_version.is_some_and(|d| d != node.digest_version) {
        return Err(Error::of(Hint::StaleVersion {
            current: node.version,
            digest_current: node.digest_version,
        }));
    }
    // State.
    let reserved = node.origin == Origin::Reserved || n.is_reserved();
    if reserved && (p.capacity.is_some() || !matches!(p.repeat, Change::Keep)) {
        return Err(Error::invalid(Why::ReservedNode));
    }
    if let Change::Set(r) = &p.repeat {
        if matches!(node.role, Role::Occurrence(_) | Role::Signal(_)) {
            return Err(Error::invalid(Why::RoleConflict));
        }
        if !fires(r, cx.now) {
            return Err(Error::invalid(Why::ScheduleNeverFires));
        }
    }
    if let Some(deps) = &depends_on {
        let added: Vec<NodeId> = deps
            .iter()
            .copied()
            .filter(|d| !node.depends_on.contains(d))
            .collect();
        let extra: Vec<(NodeId, NodeId)> = added.iter().map(|&d| (n, d)).collect();
        let removed: Vec<(NodeId, NodeId)> = node
            .depends_on
            .iter()
            .filter(|d| !deps.contains(d))
            .map(|&d| (n, d))
            .collect();
        for &d in &added {
            if d == n {
                return Err(Error::of(Hint::Cycle {
                    path: alloc::vec![n, n],
                }));
            }
            if let Some(path) = reaches(s, d, n, &extra, &removed) {
                let mut full = alloc::vec![n];
                full.extend(path);
                return Err(Error::of(Hint::Cycle { path: full }));
            }
        }
    }
    // Resources (H1).
    let live = s.ix.node.get(&n).map_or(0, |x| x.live);
    if let Some(c) = p.capacity
        && c < live
    {
        return Err(Error::of(Hint::Full {
            node: n,
            capacity: c,
            live,
        }));
    }
    // Safety valves: the occurrence of a new schedule lies one level below the node (L1, D.14.5).
    if matches!(p.repeat, Change::Set(_)) {
        let depth = s.ix.node.get(&n).map_or(0, |x| x.depth);
        if depth.saturating_add(1) > s.params.tree_max_depth {
            return Err(Error::limit(LimitKind::Depth));
        }
    }
    // The schedule, by A-75's comparison: a change of meaning only.
    let (repeat_patch, repeat) = if repeat_changes(&node.role, &p.repeat, cx.now) {
        use crate::recurring::{RepeatChange, repeat_of};
        match (&p.repeat, &node.role) {
            (Change::Set(r), Role::Series(_)) => {
                (Change::Set(repeat_of(r)), Some(RepeatChange::Changed))
            }
            (Change::Set(r), _) => (Change::Set(repeat_of(r)), Some(RepeatChange::Added)),
            (Change::Clear, _) => (Change::Clear, Some(RepeatChange::Removed)),
            (Change::Keep, _) => (Change::Keep, None),
        }
    } else {
        (Change::Keep, None)
    };
    // The patch: only the fields whose value differs (O2).
    let tags = relabel(&node.tags, &p.tags_add, &p.tags_remove);
    let needs = relabel(&node.needs, &p.needs_add, &p.needs_remove);
    let due = match &p.due {
        Change::Keep => Change::Keep,
        Change::Clear => Change::Clear,
        Change::Set(w) => Change::Set(w.resolve(cx.now)),
    };
    let out = NodePatch {
        title: differs(p.title.as_ref(), &node.title),
        body: differs(p.body.as_ref(), &node.body),
        fields: differs(p.fields.as_ref(), &node.fields),
        files: differs(p.files.as_ref(), &node.files),
        status: differs(p.status.as_ref(), &node.status),
        digest: differs(p.digest.as_ref(), &node.digest),
        agreements: differs(p.agreements.as_ref(), &node.agreements),
        kind: change(&p.kind, node.kind.as_ref()),
        tags: differs(Some(&tags), &node.tags),
        capacity: differs(p.capacity.as_ref(), &node.capacity),
        priority: differs(p.priority.as_ref(), &node.priority),
        est: change(&p.est, node.est.as_ref()),
        needs: differs(Some(&needs), &node.needs),
        due: change(&due, node.due.as_ref()),
        depends_on: differs(depends_on.as_ref(), &node.depends_on),
        owners: differs(owners.as_ref(), &node.owners),
        repeat: repeat_patch,
    };
    Ok(UpdatePlan {
        n,
        patch: (out != NodePatch::default()).then(|| Box::new(out)),
        repeat,
        unmet: crate::helpers::holds::unmet_needs(
            s,
            cx.now,
            cx.sess,
            cx.ctx.caps.as_ref(),
            &p.needs_add,
        ),
    })
}

/// Executes `node update` (D.8.20): `NodeUpdated` with the fields that differ, then, on an open
/// node, the occurrence consequences of a changed `repeat` and its coverage warning (D.8.18,
/// WP-D6), then E1 when the needs of a refused node changed (an added need can shrink the active
/// classes, CS-7); and the `unmet_needs` warning for added needs that no session can meet now
/// (1.9).
pub(crate) fn update_exec(ex: &mut Exec<'_>, plan: UpdatePlan) -> Reply {
    let n = plan.n;
    unmet_needs_warning(ex, n, plan.unmet);
    if let Some(patch) = plan.patch {
        let needs = patch.needs.is_some();
        ex.emit(Ev::NodeUpdated {
            node: n,
            patch: *patch,
        });
        if let Some(change) = plan.repeat {
            crate::recurring::repeat_updated(ex, n, change);
        }
        if needs && ex.s().ix.refused.contains_key(&n) {
            e1_check(ex, n);
        }
    }
    let node = ex.s().nodes.get(&n);
    Reply::Updated {
        version: node.map(|x| x.version).unwrap_or_default(),
        digest_version: node.map(|x| x.digest_version).unwrap_or_default(),
    }
}

// ------------------------------------------------------------------------------------------------
// close

/// The plan of `node close` (D.8.20).
#[derive(Debug)]
pub(crate) struct ClosePlan {
    n: NodeId,
    outcome: Outcome,
    summary: Text,
    text: EntryText,
    settles: BTreeSet<RequestId>,
    /// The requests `settles` lists that had settled before the close.
    settled_before: BTreeSet<RequestId>,
    unsent: BTreeSet<SaidId>,
    /// 1.8 (§5): the requests the default `settles` would have settled, left to the session that
    /// holds the node, since the closer does not.
    left: BTreeSet<RequestId>,
}

/// Validates `node close {path, outcome, summary, verified?, settles?, learnings?, token?, unsent?}`
/// (D.8.20): the node → `stale_token` → `invalid(ServerOutcome)` (A-66), `invalid(EmptyText(Summary))`
/// → `not_found(Request)`, `not_found(Said)` → `invalid(ReservedNode)`, `invalid(AlreadyClosed)`,
/// `invalid(OccurrenceNotYetAvailable)` (A-91). Without `settles`, the close settles the node's
/// requests a result answered (1.7, §5), when the closer holds the node or no other session does
/// (1.8): a close of a node another session holds, and the closer does not, settles only what
/// `settles` names, and leaves the rest to that holder.
pub(crate) fn close(cx: &Vcx<'_>, cmd: &Cmd) -> Result<ClosePlan, Error> {
    let Cmd::Node(NodeCmd::Close {
        outcome,
        summary,
        verified,
        settles,
        learnings,
        token,
        unsent,
        ..
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let n = target(s, cmd)?.ok_or(Error::not_found(What::Node))?;
    if let Some(t) = token {
        check_token(cx, *t, n)?;
    }
    if *outcome == Outcome::Missed {
        return Err(Error::invalid(Why::ServerOutcome));
    }
    if summary.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::Summary)));
    }
    if settles
        .iter()
        .flatten()
        .any(|r| !s.requests.contains_key(r))
    {
        return Err(Error::not_found(What::Request));
    }
    if unsent.iter().any(|x| !s.saids.contains_key(x)) {
        return Err(Error::not_found(What::Said));
    }
    let Some(node) = s.nodes.get(&n) else {
        return Err(Error::not_found(What::Node));
    };
    if node.origin == Origin::Reserved || n.is_reserved() {
        return Err(Error::invalid(Why::ReservedNode));
    }
    if !node.is_open() {
        return Err(Error::invalid(Why::AlreadyClosed));
    }
    if matches!(&node.role, Role::Occurrence(o) if o.phase == OccPhase::Waiting)
        && *outcome != Outcome::Dropped
    {
        return Err(Error::invalid(Why::OccurrenceNotYetAvailable));
    }
    let mut extra = Vec::new();
    if let Some(v) = verified {
        extra.push((TextField::Verified, v.clone()));
    }
    if let Some(l) = learnings {
        extra.push((TextField::Learnings, l.clone()));
    }
    // 1.8 (§5, l6 of the lab's third round): a stranger's close settled the holder's request with
    // "placeholder", and settling is final.
    let (settles, left) = match settles {
        Some(st) => (st.clone(), BTreeSet::new()),
        None if crate::helpers::holds::held_by_another(s, n, cx.sess, cx.now) => {
            (BTreeSet::new(), default_settles(s, n, unsent))
        }
        None => (default_settles(s, n, unsent), BTreeSet::new()),
    };
    let settled_before = settles
        .iter()
        .copied()
        .filter(|r| s.requests.get(r).is_some_and(|x| x.settled.is_some()))
        .collect();
    Ok(ClosePlan {
        n,
        outcome: outcome.clone(),
        summary: summary.clone(),
        text: EntryText {
            main: summary.clone(),
            extra,
        },
        settles,
        settled_before,
        unsent: unsent.clone(),
        left,
    })
}

/// The sessions other than `me` with a live hold on `n` (`held_by_other`).
fn other_holders(s: &State, n: NodeId, me: Option<SessionId>, now: Time) -> Vec<SessionId> {
    crate::query::holders(s, n, now)
        .into_iter()
        .map(|(x, _)| x)
        .filter(|x| Some(*x) != me)
        .collect()
}

/// Executes `node close` (D.8.20): the decision its `settles` records, with the node's assumptions,
/// and the settling; the session's pending `said` entries confirmed but the unsent; the warnings,
/// evaluated after those; `close_node`; and, for a series, `series_stopped` naming the occurrence
/// the close dropped, which the reply names too (1.4, R2, R9).
pub(crate) fn close_exec(ex: &mut Exec<'_>, plan: ClosePlan) -> Reply {
    let n = plan.n;
    let now = ex.now();
    let by = ex.author();
    // 1. The summary's decision (steps 1, 3, and 4 of `talk decide`).
    let mut decision: Option<DecisionId> = None;
    let mut settled: Vec<RequestId> = Vec::new();
    if !plan.settles.is_empty() {
        let assumes = ex
            .s()
            .nodes
            .get(&n)
            .map(|x| x.assume_marks.clone())
            .unwrap_or_default();
        let d = ex.append(
            n,
            by,
            EntryBody::Decision {
                settles: plan.settles.clone(),
                supersedes: None,
                assumes: assumes.clone(),
                evidence: None,
                basis: Vec::new(),
            },
            EntryText {
                main: plan.summary.clone(),
                extra: Vec::new(),
            },
        );
        assume_marks_off(ex, n, d, &assumes);
        for &r in &plan.settles {
            if try_settle(ex, r) {
                settled.push(r);
            }
        }
        decision = Some(d);
    }
    // 2.
    confirm_pending(ex, &plan.unsent, ConfirmVia::Close);
    // 3. The warnings, evaluated now, and the requests the close leaves unsettled (1.7): those it
    // lists, then the node's own, each once, with why.
    let s = ex.s();
    let mut left: BTreeSet<RequestId> = plan
        .settles
        .iter()
        .copied()
        .filter(|r| !settled.contains(r))
        .collect();
    if let Some(it) = s.ix.items.get(&n) {
        left.extend(it.unsettled.iter().copied());
    }
    let not_settled: Vec<NotSettled> = left
        .into_iter()
        .map(|r| NotSettled {
            request: r,
            why: if !plan.settled_before.contains(&r)
                && s.requests.get(&r).is_some_and(|x| x.answered_by.is_some())
            {
                NotSettledWhy::AwaitsDecision
            } else {
                super::review::not_settled_why(s, r, &plan.settled_before)
            },
        })
        .collect();
    let mut warnings = Vec::new();
    if let Some(it) = s.ix.items.get(&n) {
        if !it.unsettled.is_empty() {
            warnings.push(Warning {
                code: WarningCode::UnsettledRequests,
                id: Some(n),
                detail: WarningDetail::UnsettledRequests {
                    requests: it.unsettled.iter().copied().collect(),
                },
            });
        }
        if !it.open_commitments.is_empty() {
            warnings.push(Warning {
                code: WarningCode::OpenCommitments,
                id: Some(n),
                detail: WarningDetail::OpenCommitments {
                    commitments: it.open_commitments.iter().copied().collect(),
                },
            });
        }
        if !it.open_objections.is_empty() {
            warnings.push(Warning {
                code: WarningCode::OpenObjections,
                id: Some(n),
                detail: WarningDetail::OpenObjections {
                    objections: it.open_objections.iter().copied().collect(),
                },
            });
        }
    }
    let sources = crate::query::reads::agreement_sources(s, n);
    if !sources.is_empty() {
        warnings.push(Warning {
            code: WarningCode::Agreement,
            id: Some(n),
            detail: WarningDetail::Agreement {
                sources,
                approvals: crate::query::reads::current_approvals(s, n),
                content_version: s
                    .nodes
                    .get(&n)
                    .map(|x| x.content_version)
                    .unwrap_or_default(),
            },
        });
    }
    let me = ex.sess();
    let mut holders = other_holders(s, n, me, now);
    if let Some(x) = s.nodes.get(&n)
        && matches!(x.role, Role::Series(_))
    {
        for &o in s.ix.series_open.get(&n).into_iter().flatten() {
            for h in other_holders(s, o, me, now) {
                if !holders.contains(&h) {
                    holders.push(h);
                }
            }
        }
    }
    if !holders.is_empty() {
        warnings.push(Warning {
            code: WarningCode::HeldByOther,
            id: Some(n),
            detail: WarningDetail::HeldByOther {
                holders,
                left: plan.left.iter().copied().collect(),
            },
        });
    }
    for w in warnings {
        ex.warn(w);
    }
    // 4.
    let dropped = close_node(
        ex,
        n,
        Closing {
            outcome: plan.outcome,
            by,
            merged_into: None,
            cause: EndCause::Close,
            text: plan.text,
            reslot: None,
        },
    );
    // 5. A series' close dropped its open occurrence: `series_stopped` (1.4, §12, R2), a warning
    // only.
    if let Some(o) = dropped {
        ex.warn(series_stopped(n, o));
    }
    Reply::Closed {
        node: n,
        decision,
        settled,
        not_settled,
        dropped,
        leave_better: crate::query::leave_better(ex.s(), Some(n), ex.sess()),
    }
}

/// The `series_stopped` warning of a close of series `n`, or a merge from it, that dropped its open
/// occurrence `o` (1.4, §12, §21, R2): no further occurrence opens until the series is reopened.
pub(crate) fn series_stopped(n: NodeId, o: NodeId) -> Warning {
    Warning {
        code: WarningCode::SeriesStopped,
        id: Some(n),
        detail: WarningDetail::SeriesStopped { occurrence: o },
    }
}

// ------------------------------------------------------------------------------------------------
// hide

/// The plan of `node hide` (D.8.20).
#[derive(Debug)]
pub(crate) struct HidePlan {
    target: HideTarget,
    node: NodeId,
    reason: Text,
}

/// Validates `node hide {entry | file, reason}` (D.8.20, D.8.24's row): a file's node resolves →
/// the entry exists (`eid < next_eid`, and its node, which the shell adds from its `entries` table,
/// exists: an entry the table lacks reaches the core with the shell's `UNKNOWN_NODE`, D.14.6;
/// `not_found(Entry)` for either), or the node has that file (`not_found(Node)`) →
/// `invalid(EmptyText(Reason))`.
pub(crate) fn hide(cx: &Vcx<'_>, cmd: &Cmd) -> Result<HidePlan, Error> {
    let Cmd::Node(NodeCmd::Hide {
        target: arg,
        reason,
    }) = cmd
    else {
        return Err(Error::invalid(Why::BadParams));
    };
    let s = cx.s;
    let file_node = target(s, cmd)?;
    // D.8.24's row and D.8.20: the entry, or the node and its file, exists (`not_found`) before
    // the reason is checked (`invalid(EmptyText)`).
    let (t, node) = match arg {
        HideArg::Entry { eid, node } => {
            if *eid >= s.counters.next_eid || !s.nodes.contains_key(node) {
                return Err(Error::not_found(What::Entry));
            }
            (HideTarget::Entry(*eid), *node)
        }
        HideArg::File { name, .. } => {
            let n = file_node.ok_or(Error::not_found(What::Node))?;
            if !s.nodes.get(&n).is_some_and(|x| x.files.contains_key(name)) {
                return Err(Error::not_found(What::Node));
            }
            (
                HideTarget::File {
                    node: n,
                    name: name.clone(),
                },
                n,
            )
        }
    };
    if reason.is_empty() {
        return Err(Error::invalid(Why::EmptyText(Field::Reason)));
    }
    Ok(HidePlan {
        target: t,
        node,
        reason: reason.clone(),
    })
}

/// Executes `node hide`: `Hidden`, then its system entry on the target's node (O3: the core never
/// reads hidden status).
pub(crate) fn hide_exec(ex: &mut Exec<'_>, plan: HidePlan) -> Reply {
    ex.emit(Ev::Hidden {
        target: plan.target.clone(),
        reason: plan.reason.clone(),
    });
    let by = ex.author();
    ex.append(
        plan.node,
        by,
        EntryBody::System(SystemNote::Hidden(plan.target)),
        EntryText {
            main: plan.reason,
            extra: Vec::new(),
        },
    );
    Reply::Hidden
}
