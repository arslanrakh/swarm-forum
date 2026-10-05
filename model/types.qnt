// -*- mode: Bluespec; -*-
/**
 * Types of the Swarm Forum model (design D.10.3): ids, sum types (keys and names included), and the
 * records of the abstract state. The model holds no text.
 *
 * Conventions:
 * - Node ids, entry ids (eids), session ids, principal ids, versions, fences, seqs, and times are
 *   `int`. Times are ticks (D.10.5).
 * - `-1` stands for an absent optional id, time, or count (Rust's `None`): `Node.parent` of ROOT,
 *   `Node.due`, `Node.est`, `Request.answered_by`, `UQuestion.about`, `*.moved_from`, and so on,
 *   and for an absent integer field of a command (`Cmd`, where the presence rules are listed).
 *   Where an absent value changes what an entity is, a sum type is used instead (`OptWait`,
 *   `Settlement`, `LifeAt`).
 * - `""` stands for an absent label or principal key (`Node.kind`, a command's `principal`).
 * - Sum types that share a domain word carry a short prefix (`Pq…`, `Uq…`, `Sd…`), because Quint
 *   constructors live in one namespace. The names D.10.3 gives (`Conv`, `Msg`, `User`, `Person`,
 *   `Lit`, `Slot`, `SlotDup`, `Signal`, `BNode`, `BEntry`) are kept as they are, because the
 *   conformance projection parses Rust's strings into them (D.10.8). D.10.3's `Repeat` is
 *   `RepeatSlot`: the TLA+ that Apalache and TLC check extends Apalache's standard module, which
 *   defines an operator `Repeat`, and TLC refuses the second definition.
 * - No identifier may repeat an operator of the TLA+ modules that generated TLA+ extends
 *   (Integers, Sequences, FiniteSets, TLC, Apalache, Variants), for the same reason.
 */
module types {
  // ------------------------------------------------------------------ keys and names (D.10.3)

  /// Quint strings are opaque: there is no concatenation and no integer-to-string conversion, so
  /// every key and name the server builds from ids is a sum type, not a string.
  /// - `Conv(c)`: an agent's conversation key (`chat:…` in Rust);
  /// - `Msg((c, n))`: message `n` of conversation `c`, a request's origin (A-61); a conversation key
  ///   too, because its scheme is a conversation scheme (D.2.4);
  /// - `Obj(o)`: an agent's object key, whose scheme is not a conversation scheme (a paper, a proof:
  ///   `lean:<o>` in a replay), so that a node can carry an object key beside a conversation key
  ///   (CS-36). D.10.3's `Key` has no such variant; it is added here (README, "Deviations");
  /// - `User(e)`: `user:e<e>`, the routing key of user entry `e`;
  /// - `RepeatSlot((series, slot))`: `repeat:<series>:<slot>`, the key of an occurrence;
  /// - `Person(p)`: `person:<p>`, one alias of a person, `p` a principal key (D.2.2).
  type Key = Conv(str) | Msg((str, int)) | Obj(str) | User(int) | RepeatSlot((int, int)) | Person(str)

  type SignalKind = SigDuplicate | SigConflict

  /// - `Lit(s)`: a name an agent gave (`Lit("w1")`), or a reserved node's name;
  /// - `Slot(t)`: the `YYYYMMDDTHHMMZ` name of an occurrence at slot `t`;
  /// - `SlotDup((t, id))`: that name with `-<id>`, when the slot name is live already;
  /// - `Signal((kind, a, b, id))`: `<kind>-<a>-<b>-<id>`, a signal node's name.
  type Name = Lit(str) | Slot(int) | SlotDup((int, int)) | Signal((SignalKind, int, int, int))

  // ------------------------------------------------------------------ small enums (D.2.5)

  type LinkRel = Continues | DerivedFrom | ConflictsWith | Duplicate | SupersededBy

  /// §5's conventional outcomes and the server's `missed`. Rust's `Other(label)` is not modelled.
  type Outcome = Done | Failed | Dropped | Superseded | Missed

  type SaidKind = SAck | SReply | SResult | SCorrection | SQuestion
  type ToUserKind = TuAck | TuQuestion | TuResult
  type ReleaseReason = Handoff | Blocked | Cannot

  /// §6 entry types, in this order.
  type EntryType =
    | ENote | ERequest | EQuestion | EAnswer | EDecision | ECheckpoint | ESaid | EHeard
    | EFromUser | EToUser | EApprove | EObject | EResolve | ERetract | ESystem | ELate

  /// The fourteen flags of §14, in table order (D.4.10), `stale` last (1.8). The model stores no
  /// flags: they are evaluated by `flags.qnt`.
  type Flag =
    | FUnrouted | FUnansweredRequest | FOpenCommitment | FUnconfirmedSaid | FOpenIntent
    | FQuestionRecheck | FProceedOnAssumption | FObjection | FBasisChanged | FTooBig | FOverdue
    | FReadyToClose | FMissed | FStale

  type Mark = MRecheck | MAssume

  /// §25 hold ends. `HeEndedByClose(CauseMove)` is a move by another session (A-6).
  type EndCause = CauseClose | CauseMerge | CauseMove
  type HoldEnd = HeReleased | HeExpired | HeEndedByClose(EndCause)

  // ------------------------------------------------------------------ nodes (D.4.3)

  type Origin =
    | OReserved          // the eight reserved nodes made by Genesis
    | OAgent(int)        // node create, by that session
    | OAncestor(int)     // a missing ancestor made by node create or move, by that session
    | OOccurrence        // made by the server for a series (§12)
    | OSignal            // made by the server under sys/signals (§14)

  /// `merged_into`: -1 for none (K5: only with outcome `Superseded`). `upheld` (1.8, §5, §11.3):
  /// the `resolve` entry of an objection upheld on the closed node, which corrected it; -1 for none.
  /// A node marked so is never a candidate, and a take of it is `closed`. `from_done` (1.10): the
  /// node was closed `done` before the correction that marks it, which a second, opposite verdict
  /// withdraws, restoring `done` (Rust's `corrected_from == Some(Done)`).
  /// 1.11 (§13): `by`, the session that closed the node, -1 for the server; a note without `to` at
  /// the closed node reaches it while it is live.
  type Closure = { at: int, outcome: Outcome, merged_into: int, upheld: int, from_done: bool, by: int }
  type Life = Open | Closed(Closure)

  /// One signal per (kind, unordered pair): a < b always.
  type SignalKey = { kind: SignalKind, a: int, b: int }

  /// A series' schedule (D.10.6): the slots are `offset + k × period`; the replay driver maps it to
  /// the cron `*/period * * * *` in UTC. `run_within > 0`.
  type RepeatSpec = { period: int, offset: int, run_within: int }
  type SeriesInfo = { repeat: RepeatSpec, last_missed: bool }

  type OccPhase = Waiting | Available
  type OccInfo = { series: int, slot: int, window_end: int, phase: OccPhase, window_fired: bool }

  type Role = Plain | Series(SeriesInfo) | Occurrence(OccInfo) | RoleSignal(SignalKey)

  /// What a wait waits on (D.4.3). The `what` of an external wait is text, so it is absent.
  type WaitOn =
    | WQuestion({ q: int, rechecks_at_set: int })   // a people question
    | WUserQuestion(int)                            // a question for the user (A-31, CS-2)
    | WNode(int)                                    // resolves while the target is closed
    | WTime(int)                                    // `{time}`
    | WExternal(int)                                // `{external: what, recheck_after}`

  /// `fired`: the one-shot marker of the Wait deadline of `WTime` and `WExternal` waits.
  type Wait = { on: WaitOn, set_at: int, fired: bool }
  type OptWait = NoWait | HasWait(Wait)

  /// An open intent; `eid = -1` (and `session = -1`) for none.
  type Intent = { eid: int, session: int }

  /// A node (D.4.3) with the abstractions of D.10.3:
  /// - `parent` and `name` are Rust's `place` (`parent = -1` and `name = Lit("")` only for ROOT);
  /// - `content` stands for title, body, fields, and files, `status` for the status, `digest` for
  ///   the digest, and `agreements` says whether the agreements text is non-empty, so that both
  ///   halves of O2 stay checkable: a status change bumps `version` but not `content_version`, and a
  ///   change of the other four bumps both (CS-34, §11.2: an approval survives a status change).
  ///   D.10.3 folds status into `content`, which cannot express a status-only update; the split is
  ///   recorded in the README ("Deviations"). Content 0 is the content a node starts with when the
  ///   server makes it (a reserved node's or an ancestor's name as title, a signal's generated
  ///   title); the replay maps any other value `v` to the title `c<v>` (D.10.8). Status 0 is no
  ///   status, and the replay maps any other value `v` to the status `s<v>`;
  /// - `held_ms`, `people_left`, and `left` are absent (D.10.3, D.10.8).
  type Node = {
    parent: int,
    name: Name,
    origin: Origin,
    created_at: int,
    content: int,
    status: int,
    digest: int,
    agreements: bool,
    kind: str,                 // "" for none
    tags: Set[str],
    capacity: int,
    priority: int,
    est: int,                  // minutes; -1 for none
    needs: Set[str],
    due: int,                  // -1 for none
    due_fired: bool,           // the one-shot marker of the Due deadline
    owners: Set[int],          // person or team nodes (D-3); empty: inherit
    keys: Set[Key],
    links: Set[(LinkRel, int)],
    depends_on: Set[int],
    wait: OptWait,
    role: Role,
    life: Life,
    version: int,
    digest_version: int,
    fence: int,
    seq: int,
    content_version: int,
    attempts: int,
    checkpoints: int,
    last_checkpoint: int,      // -1 for none
    open_intent: Intent,
    recheck_marks: Set[int],
    assume_marks: Set[int],
    touched: int,              // 1.8 (§5): when the node was last touched
    stale_fired: bool,         // 1.8: the one-shot marker of the Stale deadline
  }

  // ------------------------------------------------------------------ holds (D.4.4)

  /// The lease terms in force at issue (A-102).
  type LeaseTerms = { first: int, default: int, max: int, ceiling: int, renew_max: int }

  /// Holds are keyed by their token `(node, fence)`.
  type Hold = {
    session: int,
    taken_at: int,
    lease_until: int,
    last_extended: int,
    terms: LeaseTerms,
    l: int,
    renews: int,
    checkpointed: bool,
  }

  // ------------------------------------------------------------------ feed entries (D.4.5)

  /// The metadata of a kept entry (`keeps_meta`, CS-17). `by`: the author session, or 0 for the
  /// server (sessions start at 1). `principal`: -1 unless the entry is a `from_user` or a `to_user`.
  type EntryMeta = { node: int, seq: int, at: int, by: int, ty: EntryType, principal: int }

  type Requester = ReqUser(int) | ReqExternal

  /// Latched once, never cleared (Settled).
  type Settlement = Unsettled | SettledBy({ decision: int, answer: int })

  /// `conv` (1.3, §9.4, D.4.5, D.8.8): the conversation key the request's node was found by, its
  /// `where`, else its origin; for a routed user entry its `User(e)` key. Only a `said` whose own
  /// conversation key is `conv` answers the request.
  type Request = {
    node: int,
    origin: Key,               // a `Msg`, a `Conv`, or a `User` (A-61)
    conv: Key,
    from_user: int,            // -1 unless the origin is `User(e)`
    requester: Requester,
    answered_by: int,          // -1 for none
    settled: Settlement,
    moved_from: int,           // -1 for none
  }

  /// 1.12 (§9.3): `PqWithdrawn(x)`, withdrawn by the retract entry `x` while it was open, for good.
  type PqState = PqOpen | PqExpired | PqEscalated(int) | PqAnswered(int) | PqSuperseded(int) | PqWithdrawn(int)
  type Lapse = NoLapse | LapseExpired | LapseEscalated

  type PQuestion = {
    node: int,
    conv: Key,
    blocking: bool,
    said: int,
    state: PqState,
    rechecks: int,
    recheck_at: int,
    lapse: Lapse,
    assumed_by: int,           // -1 for none
    moved_from: int,           // -1 for none
  }

  type AqState = AqOpen | AqAnswered(int)
  type AQuestion = { node: int, asker: int, state: AqState }

  /// `digest` abstracts the SHA-256 of the masked text and options (D.8.12).
  type UqSource =
    | SrcAgent({ session: int, digest: int })
    | SrcEscalation(int)       // the blocking people question
    | SrcRefusal(int)          // the refused node

  type UqState = UqPending | UqAnswered(int) | UqDefaulted | UqLapsed

  type UQuestion = {
    source: UqSource,
    about: int,                // -1 for none
    to: List[int],             // the addressee chain (U1)
    stage: int,
    due: int,
    has_default: bool,
    state: UqState,
    assumed_by: int,           // -1 for none
  }

  type CommitmentState = CmOpen | CmFulfilled(int)
  type Commitment = {
    node: int,
    conv: Key,
    last_ack: int,
    followup_at: int,
    followup_fired: bool,
    state: CommitmentState,
    moved_from: int,
  }

  /// 1.9 (§9.2): `SdWithheld`, a `said` recorded while another session held its conversation node
  /// or work inside it and the caller held neither: not sent, and nobody is to send it.
  type SaidState = SdPending | SdConfirmed | SdUnconfirmed | SdWithheld
  type Said = { node: int, conv: Key, kind: SaidKind, session: int, state: SaidState, moved_from: int }

  type DecisionState = DActive | DSuperseded(int) | DRetracted(int)

  /// The outcome a cited node had when the decision was recorded (D.10.3 writes it as "an outcome or
  /// -1 for open").
  type LifeAt = AtOpen | AtClosed(Outcome)

  /// A basis reference (CS-22, D-4). The cited version and external references are absent.
  type BasisRef = BNode({ id: int, life_at: LifeAt }) | BEntry(int)

  /// What a node or entry reference points at, and what `retract` strikes out.
  type BasisTarget = TNode(int) | TEntry(int)

  type Decision = { node: int, settles: Set[int], state: DecisionState, basis: List[BasisRef] }

  type ObjectionState = ObOpen | ObResolved(int) | ObUpheld(int)
  type Objection = { node: int, state: ObjectionState, moved_from: int }

  type Routing =
    | Unrouted
    | RtRequest({ node: int, request: int })
    | RtClarifies(int)
    | RtAnswers(int)
    | RtNoAction

  type UserEntry = { session: int, principal: int, at: int, routing: Routing }

  type DigestItem =
    | DiAssumption(int)                            // a people question expired
    | DiDefaultApplied(int)                        // a user question defaulted
    | DiRefusalDrop({ node: int, question: int })  // a refused node was dropped
    | DiLapsed(int)                                // a user question lapsed

  type DigestState = DgPending | DgShown(int)
  type DigestEntry = { item: DigestItem, to: int, state: DigestState }

  type Addressee = ToSession(int) | ToHolder(int) | ToHolders(int) | ToPrincipal(int)

  type Note = {
    node: int,
    to: Addressee,
    until: int,
    recipients: Set[int],
    pending_holder: bool,
    read_by: Set[int],
  }

  // ------------------------------------------------------------------ refusals, classes, pauses (D.4.6)

  type RefusalEntry = { backoff_until: int, expires_at: int }

  /// One record per (node, class), with an entry per refusing session (CS-7).
  type Refusal = { quorum: int, by: int -> RefusalEntry }

  /// The caps a session of the class held at each open `next`, mapped to that call's window end.
  type ClassActivity = { caps_seen: Set[str] -> int }

  type Pause = { until: int, last_renewed: int, ttl: int, by: int, class: str }

  // ------------------------------------------------------------------ sessions (D.4.7)

  type Session = {
    class: str,
    started_at: int,
    last_seen: int,
    silence_at: int,
    retire_at: int,
    silent: bool,
    caps: Set[str],
    horizon: int,              // minutes; -1 for none
    principals: Set[int],
    shown: int -> int,         // node -> the highest seq shown
    branch_seen: int -> int,   // scope node -> the newest `holders:` note read (D-6)
    handed_off: HandedOff,     // 1.12 (§10.4): the hold it last released as handoff
  }

  /// 1.12 (§10.4): a hold its session released as `handoff`, `(node, fence)`, and until when
  /// `next` keeps the node from that session (the release plus the hold's `lease.first`); node -1
  /// for none.
  type HandedOff = { node: int, fence: int, until: int }

  // ------------------------------------------------------------------ commands (D.10.4)

  type RepeatChange = RepKeep | RepClear | RepSet(RepeatSpec)
  type DueChange = DueKeep | DueClear | DueSet(int)
  type IdsChange = IdsKeep | IdsSet(Set[int])
  type OptToken = NoToken | SomeToken((int, int))
  type OptKey = NoKey | SomeKey(Key)
  type WaitArg = WaNone | WaQuestion(int) | WaNode(int) | WaTime(int) | WaExternal(int)
  /// A basis reference as the agent wrote it (D.5.2's `BasisArg`): a node, at a version (`#12@4`)
  /// or not (`version = -1`, `#12`), or an entry. The version is read only by `decide`'s validation
  /// (`invalid(VersionAhead)`, A-71) and is not stored (`BasisRef`, D.10.3).
  type BasisArg = BaNode({ id: int, version: int }) | BaEntry(int)
  type AgentTo = AgSession(int) | AgHolder(int) | AgHolders(int)
  type SayTo =
    | SayNone | SaySession(int) | SayHolder(int) | SayHolders(int) | SayUser | SayPrincipal(str)

  /// The abstract command: one variant per action of D.10.4, with the action's parameters except
  /// the session, the `op`, and `long` (a recorded command never has `long`, D.10.4). Its value is
  /// the `op_id` digest: "same digest" is equality of `Cmd` values (D.10.3). `CInit`, `CTick`, and
  /// `CTickNext` only name the time actions in `last_action`.
  ///
  /// Presence. Rust's validation depends on whether an optional field is present (D.8.20:
  /// `MissingDigestVersion` for a digest without `digest_version`, `ReservedNode` for a capacity on
  /// a reserved node), so every optional field says how it is absent:
  /// - `-1` is absent for the integer fields: `CUpdate.dver`, `content`, `status`, `digest`, and
  ///   `capacity` (a value `>= 0` is present: `content`, `status`, and `digest` are values of their
  ///   small domains, D.10.3), `CFromUser.answers`, `clarifies`, and `node`, `CDecide.supersedes`,
  ///   and `BaNode.version`;
  /// - `""` is absent for a principal key (`CHello`, `CFromUser`, `CToUser`; `CAskUser.to` is
  ///   `"user"` or a principal key);
  /// - the `…Keep` variants (`RepKeep`, `DueKeep`, `IdsKeep`) are absent for `repeat`, `due`,
  ///   `depends_on`, and `owners`, and empty sets are absent for the additions and removals of needs
  ///   and tags (CS-34);
  /// - `NoToken`, `NoKey`, `WaNone`, and `SayNone` are absent for a token, a `where`, a wait, and a
  ///   `say` addressee.
  ///
  /// Beyond D.10.4's parameter lists (README, "Deviations"): `CFromUser.clarifies` (§8.1, §8.2,
  /// A-99; §32 step 1 uses it), `CAskUser.again` and `CAskUser.digest` (§8.4 "Asking only once":
  /// the digest of the masked text and options, from a small domain), `BaNode.version` (A-71), and
  /// `CCreate.tags` (Rust's `CreateArgs::tags`), which the seed of D.10.7 needs ("created with
  /// `take: false` and tagged `offer`") and `act_create` leaves empty.
  ///
  /// 1.12: `CRelease.settles` (§10.3: the requests the decision lists that the release records
  /// after itself, as `decide` would; empty: no decision).
  ///
  /// 1.11: `CResolve.defer` (§11.3: a review that cannot be decided yet, which backs the caller's
  /// class off a closed node as a `cannot` release does; never with `upheld`).
  ///
  /// 1.8: `CResolve.upheld` (§11.3: an upheld objection corrects a closed node), and
  /// `CCheckIntent(n)`, `hold check {target: n, intent_done}` (§10.2), the one form of `check` that
  /// writes; a `check` without `intent_done` reads and is not modelled.
  type Cmd =
    | CInit
    | CTick(int)
    | CTickNext
    | CHello({ caps: Set[str], principal: str })
    | CCreate({
        parent: int, name: Name, keys: Set[Key], take: bool, capacity: int, owners: Set[int],
        repeat: RepeatChange, tags: Set[str]
      })
    | CUpdate({
        node: int, ver: int, dver: int, content: int, status: int, digest: int, capacity: int,
        needs_add: Set[str], needs_remove: Set[str], tags_add: Set[str], tags_remove: Set[str],
        due: DueChange, depends: IdsChange, owners: IdsChange, repeat: RepeatChange
      })
    | CLink({ node: int, rel: LinkRel, to: int })
    | CKeyAdd({ node: int, key: Key })
    | CKeyRemove({ node: int, key: Key })
    | CClose({ node: int, outcome: Outcome, settles: Set[int], token: OptToken })
    | CReopen(int)
    | CMove({ node: int, parent: int, name: Name })
    | CMerge({ from: int, into: int })
    | CTake({ node: int, lease: int })
    | CNext
    | CCheckpoint({ token: (int, int), intent: bool, intent_done: bool })
    | CRenew((int, int))
    | CRelease({ token: (int, int), reason: ReleaseReason, wait: WaitArg, settles: Set[int] })
    | CFromUser({ principal: str, answers: int, clarifies: int, node: int })
    | CToUser({ principal: str, kind: ToUserKind, re: Set[int] })
    | CRoute({ entry: int, to: int })
    | CRequest({ at: int, origin: Key, where_key: OptKey })
    | CSaid({ at: int, kind: SaidKind, where_key: OptKey })
    | CConfirm(int)
    | CHeard({ at: int, answers: int })
    | CAskPerson({ at: int, blocking: bool, again: bool })
    | CAskUser({ at: int, has_default: bool, to: str, again: bool, digest: int })
    | CAskAgent({ at: int, to: AgentTo })
    | CAnswer(int)
    | CDecide({ at: int, settles: Set[int], supersedes: int, basis: List[BasisArg] })
    | CRetract(BasisTarget)
    | CApprove({ node: int, ver: int })
    | CObject(int)
    | CResolve({ objection: int, upheld: bool, defer: bool })
    | CCheckIntent(int)
    | CSay({ at: int, to: SayTo })
    | CWait(int)
    | CHide(int)
    | CPause(int)
    | CUnpause(int)
    | CView(int)

  /// The retry store (O1, CS-8): the abstract command in place of Rust's SHA-256 digest.
  type OpRecord = { digest: Cmd, session: int, expires_at: int }

  // ------------------------------------------------------------------ outcomes (D.6.3)

  /// `WritesPerHour` and `Busy` are the shell's and are not modelled.
  type LimitKind = LkRenew | LkDepth | LkChildren | LkTextTooLong

  type What =
    | WhatNode | WhatSession | WhatEntry | WhatRequest | WhatQuestion | WhatSaid | WhatDecision
    | WhatObjection | WhatPause | WhatKey | WhatUserEntry
    | WhatAddressee   // 1.4 (R12): a `session:` addressee that does not exist; 1.10 in the model

  type Why =
    | WhyNameTaken | WhyReservedNode | WhyReservedKey | WhyAlreadyClosed | WhyNotClosed
    | WhyOccurrenceReopen | WhyOccurrenceNotYetAvailable | WhySelfLink | WhySelfMerge
    | WhyMergeFromClosed | WhyMergeIntoOwnOccurrence | WhyIntentOpen | WhyNoIntent
    | WhyNoConversationKey | WhyAmbiguousConversationKey | WhyNoRequestOrigin | WhyScheduleNeverFires
    | WhyBadTimeZone | WhyRunWithinZero | WhyEmptyText | WhyDigestTooLong | WhyMissingDigestVersion
    | WhySaidKindQuestion | WhyNotPeopleQuestion | WhyNotAgentQuestion | WhyNotUserQuestion
    | WhyNotUserEntry | WhyNotReportable | WhyDecisionNotActive | WhyRoleConflict
    | WhyServerOutcome | WhyClassMismatch | WhyVersionAhead | WhyAmbiguousPrincipal | WhyNoPrincipal
    | WhyBadParams

  type ErrorCode =
    | ErrFull | ErrHoldsLimit | ErrPaused | ErrStaleToken | ErrStaleVersion | ErrCycle | ErrClosed
    | ErrOpIdConflict | ErrLimit(LimitKind) | ErrInvalid(Why) | ErrNotFound(What)

  /// A command's outcome as conformance compares it (D.10.8): `Ok` with the ids of its reply (the
  /// list each command gives is documented at `execute` in `commands.qnt`), the `Replay` marker,
  /// or the error. `OutNone` for `init` and the two time actions, which run no command.
  type CmdOutcome =
    | OutNone
    | OutOk(List[int])
    | OutReplay
    | OutErr(ErrorCode)

  /// The flags of the settle that ends an action (`deadlines.qnt`'s `Settle`): a command's
  /// post-settle at its `now` (D.8.1), a time action's settle to its clock. `inv_settle_bound` and
  /// `inv_T4` read them (D.10.4); `init` settles nothing, and both are false.
  type SettleFlags = { saturated: bool, stuck: bool }

  /// `last_action` (D.10.2, D.10.8): the action's name, its parameters (the session, the class of
  /// the call, the `op` with "" for none, `long`, and the command), its outcome, and the flags of
  /// the settle that ended it. The class is the session's own when the session exists, and the
  /// action's `cls` pick otherwise (a call that names a session that never opened or has retired,
  /// whose token's class the state no longer records); a `hello` picks it, and the time actions
  /// and `init` have none ("").
  type Action = {
    name: str,
    sess: int,
    class: str,
    op: str,
    long: bool,
    cmd: Cmd,
    outcome: CmdOutcome,
    settle: SettleFlags,
  }

  // ------------------------------------------------------------------ the abstract state (D.10.3)

  type State = {
    last_now: int,
    next_node: int,
    next_eid: int,
    next_session: int,
    next_principal: int,
    anomalies: int,
    principal_ids: str -> int,                    // "user" -> 0 from genesis (CS-1)
    user_msgs: int -> List[int],                  // per principal, ascending (CS-2)
    nodes: int -> Node,
    names: (int, Name) -> int,
    aliases: (int, Name) -> int,
    entries: int -> EntryMeta,                    // keeps_meta (CS-17)
    holds: (int, int) -> Hold,                    // keyed by token (node, fence)
    requests: int -> Request,
    pquestions: int -> PQuestion,
    aquestions: int -> AQuestion,                 // dropped with their note at `until`
    uquestions: int -> UQuestion,
    commitments: int -> Commitment,
    saids: int -> Said,
    decisions: int -> Decision,
    retracted: Set[BasisTarget],
    objections: int -> Objection,
    user_entries: int -> UserEntry,
    digest: int -> DigestEntry,
    notes: int -> Note,                           // dropped at `until` (CS-17)
    approvals: (int, int) -> int,
    refusals: (int, str) -> Refusal,              // (node, class)
    classes: str -> ClassActivity,
    signals: (SignalKind, int, int) -> int,
    pauses: int -> Pause,
    sessions: int -> Session,
    ops: (str, str) -> OpRecord,                  // (class, op)
  }
}
