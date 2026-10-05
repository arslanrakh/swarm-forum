//! Session binding (design D.11.4, CS-17).
//!
//! The forum session of a call comes from, in order: the tool's `session` argument; the forum
//! session bound to the request's `Mcp-Session-Id`, for the caller's class; otherwise none, and
//! the call opens one (D.5.1) if it is a `session hello`, and is refused otherwise (1.6, §20.1). A
//! call through an MCP session that opens or names a forum session binds it. The map lives in
//! memory only: after a restart, agents pass `session`, which the status line always shows
//! (§20.1).
//!
//! **One opening per MCP session.** Calls of one MCP session may arrive together before any is
//! answered. The first that would open a forum session claims the opening ([`Claim::Open`]); the
//! others wait until it is answered and bound ([`Claim::Wait`]), and then run in its session, so
//! parallel first calls open one session, not one each (review E2-bugs-9). The opening ends when
//! its [`Opening`] is handed back ([`SessionBinding::opened`]), however the call ended; a call
//! refused without a session leaves the MCP session unbound, and the next waiter claims it.
//!
//! **The map follows the sessions.** The engine drops a session's entries when it commits the
//! session's `Fired { Silence }` or `SessionRetired` ([`SessionBinding::drop_session`]). The
//! transport does not say when an MCP session ends (an idle one expires without a word), so each
//! forum session keeps at most [`MCP_PER_SESSION`] MCP sessions, the least recently bound dropped
//! first (review E2-bugs-10): a client that opens an MCP session per call and passes `session`
//! cannot grow the map without bound. The map therefore holds at most [`MCP_PER_SESSION`] MCP
//! sessions for each live forum session. A silent session that calls again is bound again by that
//! call. Clients of MCP `2026-07-28` have no MCP session, and pass `session` on every call after
//! the first.

use std::collections::BTreeMap;

use forum_core::{ClassName, SessionCtx, SessionId};
use tokio::sync::watch;

/// The most MCP sessions bound to one forum session; binding one more drops the least recently
/// bound (review E2-bugs-10).
pub const MCP_PER_SESSION: usize = 8;

/// An MCP session of a class: the key of a binding.
type McpKey = (ClassName, String);

/// The bindings of MCP sessions to forum sessions.
#[derive(Debug, Default)]
pub struct SessionBinding {
    by_mcp: BTreeMap<McpKey, SessionId>,
    /// Each forum session's MCP sessions, the least recently bound first.
    by_session: BTreeMap<SessionId, Vec<McpKey>>,
    /// The MCP sessions whose first call is opening a forum session; a waiter's receiver sees
    /// the channel close when the opening ends.
    opening: BTreeMap<McpKey, watch::Receiver<()>>,
}

/// How a call finds its session ([`SessionBinding::claim`]).
#[derive(Debug)]
pub enum Claim {
    /// Run in this session.
    Ready(SessionCtx),
    /// Open a session (`SessionCtx::New`), and hand the opening back once the reply's session
    /// is bound.
    Open(Opening),
    /// Another call of the same MCP session is opening one: claim again once this receiver's
    /// channel closes.
    Wait(watch::Receiver<()>),
}

/// The claim of a call that opens a session for its MCP session. Hand it back with
/// [`SessionBinding::opened`]; dropping it also wakes the waiters, and the next claim then
/// replaces its stale mark.
#[derive(Debug)]
pub struct Opening {
    key: McpKey,
    tx: watch::Sender<()>,
}

impl SessionBinding {
    /// An empty map.
    #[must_use]
    pub fn new() -> SessionBinding {
        SessionBinding::default()
    }

    /// The session a call runs in: its `session` argument, else the binding of its MCP session,
    /// else a new one. It claims nothing: for a call the shell refuses, which opens no session.
    #[must_use]
    pub fn resolve(
        &self,
        class: &ClassName,
        mcp_session: Option<&str>,
        argument: Option<SessionId>,
    ) -> SessionCtx {
        if let Some(s) = argument {
            return SessionCtx::Existing(s);
        }
        mcp_session
            .and_then(|m| self.by_mcp.get(&(class.clone(), m.to_string())))
            .map_or(SessionCtx::New, |&s| SessionCtx::Existing(s))
    }

    /// Whether a call of `class` through `mcp_session` that names no session has none: its MCP
    /// session is bound to none, and no call of it is opening one. Such a call is refused unless
    /// it is a `session hello` (1.6, §20.1).
    #[must_use]
    pub fn unbound(&self, class: &ClassName, mcp_session: Option<&str>) -> bool {
        let opening = mcp_session.is_some_and(|m| {
            self.opening
                .get(&(class.clone(), m.to_string()))
                .is_some_and(|rx| rx.has_changed().is_ok())
        });
        self.resolve(class, mcp_session, None) == SessionCtx::New && !opening
    }

    /// The session a call that goes to the engine runs in, as [`SessionBinding::resolve`]; but a
    /// call that would open one through an MCP session claims the opening, or waits for the call
    /// that holds it.
    pub fn claim(
        &mut self,
        class: &ClassName,
        mcp_session: Option<&str>,
        argument: Option<SessionId>,
    ) -> Claim {
        let ctx = self.resolve(class, mcp_session, argument);
        let (SessionCtx::New, Some(m)) = (ctx, mcp_session) else {
            return Claim::Ready(ctx);
        };
        let key = (class.clone(), m.to_string());
        // A mark whose channel has closed is stale: its opening was dropped without being handed
        // back, and nobody would end it.
        if let Some(rx) = self.opening.get(&key)
            && rx.has_changed().is_ok()
        {
            return Claim::Wait(rx.clone());
        }
        let (tx, rx) = watch::channel(());
        self.opening.insert(key.clone(), rx);
        Claim::Open(Opening { key, tx })
    }

    /// Ends an opening, once the reply's session, if any, is bound: the waiters claim again.
    pub fn opened(&mut self, opening: Opening) {
        let Opening { key, tx } = opening;
        if self
            .opening
            .get(&key)
            .is_some_and(|rx| rx.same_channel(&tx.subscribe()))
        {
            self.opening.remove(&key);
        }
        drop(tx);
    }

    /// Binds the MCP session `mcp_session` of `class` to `session`, replacing an earlier binding of
    /// that MCP session, and dropping the least recently bound of `session`'s MCP sessions beyond
    /// [`MCP_PER_SESSION`].
    pub fn bind(&mut self, class: &ClassName, mcp_session: &str, session: SessionId) {
        let key = (class.clone(), mcp_session.to_string());
        if let Some(old) = self.by_mcp.insert(key.clone(), session)
            && old != session
        {
            self.unlink(old, &key);
        }
        let keys = self.by_session.entry(session).or_default();
        keys.retain(|k| *k != key);
        keys.push(key);
        if keys.len() > MCP_PER_SESSION {
            let oldest = keys.remove(0);
            self.by_mcp.remove(&oldest);
        }
    }

    /// Drops every binding to `session`: at its silence or its retirement (CS-17).
    pub fn drop_session(&mut self, session: SessionId) {
        if let Some(keys) = self.by_session.remove(&session) {
            for k in keys {
                self.by_mcp.remove(&k);
            }
        }
    }

    /// How many MCP sessions are bound.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_mcp.len()
    }

    /// Whether none is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_mcp.is_empty()
    }

    fn unlink(&mut self, session: SessionId, key: &McpKey) {
        if let Some(keys) = self.by_session.get_mut(&session) {
            keys.retain(|k| k != key);
            if keys.is_empty() {
                self.by_session.remove(&session);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(s: &str) -> ClassName {
        ClassName::new(s).unwrap()
    }

    /// Every MCP key of `by_session` is bound to that session, and every binding is listed there
    /// once.
    fn consistent(b: &SessionBinding) -> bool {
        let listed: usize = b.by_session.values().map(Vec::len).sum();
        listed == b.by_mcp.len()
            && b.by_session.iter().all(|(s, keys)| {
                !keys.is_empty() && keys.iter().all(|k| b.by_mcp.get(k) == Some(s))
            })
    }

    #[test]
    fn the_argument_wins_then_the_binding_then_a_new_session() {
        let mut b = SessionBinding::new();
        let ide = class("ide-agent");
        assert_eq!(b.resolve(&ide, Some("m1"), None), SessionCtx::New);
        assert_eq!(b.resolve(&ide, None, None), SessionCtx::New);
        b.bind(&ide, "m1", SessionId(4));
        assert_eq!(
            b.resolve(&ide, Some("m1"), None),
            SessionCtx::Existing(SessionId(4))
        );
        assert_eq!(
            b.resolve(&ide, Some("m1"), Some(SessionId(9))),
            SessionCtx::Existing(SessionId(9))
        );
        assert_eq!(b.resolve(&ide, Some("m2"), None), SessionCtx::New);
        // Another class's MCP session id is another key.
        assert_eq!(b.resolve(&class("ci"), Some("m1"), None), SessionCtx::New);
    }

    #[test]
    fn rebinding_moves_the_entry_and_silence_drops_it() {
        let mut b = SessionBinding::new();
        let c = class("coding-agent");
        b.bind(&c, "m1", SessionId(1));
        b.bind(&c, "m2", SessionId(1));
        b.bind(&c, "m3", SessionId(2));
        assert_eq!(b.len(), 3);
        // m2 now names s-2: s-1 keeps only m1.
        b.bind(&c, "m2", SessionId(2));
        assert!(consistent(&b));
        b.drop_session(SessionId(1));
        assert_eq!(b.resolve(&c, Some("m1"), None), SessionCtx::New);
        assert_eq!(
            b.resolve(&c, Some("m2"), None),
            SessionCtx::Existing(SessionId(2))
        );
        assert_eq!(b.len(), 2);
        b.drop_session(SessionId(2));
        assert!(b.is_empty());
        assert!(b.by_session.is_empty(), "no reverse entry is left behind");
        // Dropping what is not there is harmless.
        b.drop_session(SessionId(7));
        assert!(b.is_empty());
    }

    /// A client that opens an MCP session for every call and passes `session` binds a new MCP
    /// session each time: the session keeps the most recently bound eight (review E2-bugs-10).
    #[test]
    fn a_session_keeps_its_most_recently_bound_mcp_sessions() {
        let mut b = SessionBinding::new();
        let c = class("coding-agent");
        for i in 0..100 {
            b.bind(&c, &format!("m{i}"), SessionId(1));
            assert!(b.len() <= MCP_PER_SESSION);
            assert!(consistent(&b));
        }
        assert_eq!(b.len(), MCP_PER_SESSION);
        assert_eq!(b.resolve(&c, Some("m91"), None), SessionCtx::New);
        assert_eq!(
            b.resolve(&c, Some("m92"), None),
            SessionCtx::Existing(SessionId(1))
        );
        // An MCP session in use is bound again by each call, and stays.
        b.bind(&c, "m92", SessionId(1));
        for i in 100..107 {
            b.bind(&c, &format!("m{i}"), SessionId(1));
        }
        assert_eq!(
            b.resolve(&c, Some("m92"), None),
            SessionCtx::Existing(SessionId(1))
        );
        assert_eq!(b.resolve(&c, Some("m93"), None), SessionCtx::New);
        // Other sessions have their own eight.
        b.bind(&c, "x", SessionId(2));
        assert_eq!(b.len(), MCP_PER_SESSION + 1);
        assert!(consistent(&b));
    }

    /// The first call of an unbound MCP session claims the opening; the others wait, and claim
    /// again once it ends: then the binding answers, or, if the opening bound nothing, the next
    /// claims it (review E2-bugs-9).
    #[test]
    fn one_call_opens_the_session_of_an_mcp_session() {
        let mut b = SessionBinding::new();
        let c = class("coding-agent");
        let Claim::Open(first) = b.claim(&c, Some("m1"), None) else {
            panic!("the first call opens");
        };
        let Claim::Wait(rx) = b.claim(&c, Some("m1"), None) else {
            panic!("the second waits");
        };
        // Neither the argument nor another MCP session or class waits.
        assert!(matches!(
            b.claim(&c, Some("m1"), Some(SessionId(3))),
            Claim::Ready(SessionCtx::Existing(SessionId(3)))
        ));
        assert!(matches!(
            b.claim(&c, None, None),
            Claim::Ready(SessionCtx::New)
        ));
        let Claim::Open(other) = b.claim(&class("ci"), Some("m1"), None) else {
            panic!("another class's MCP session id is another key");
        };
        b.opened(other);
        b.bind(&c, "m1", SessionId(5));
        b.opened(first);
        assert!(rx.has_changed().is_err(), "the waiter's channel closed");
        assert!(matches!(
            b.claim(&c, Some("m1"), None),
            Claim::Ready(SessionCtx::Existing(SessionId(5)))
        ));
        // An opening that bound nothing (a refusal without a session) passes to the next call.
        let Claim::Open(first) = b.claim(&c, Some("m2"), None) else {
            panic!("opens");
        };
        drop(b.claim(&c, Some("m2"), None));
        b.opened(first);
        let Claim::Open(dropped) = b.claim(&c, Some("m2"), None) else {
            panic!("opens");
        };
        // An opening dropped without being handed back leaves a stale mark, which the next
        // claim replaces rather than waiting on a closed channel for ever.
        drop(dropped);
        assert!(matches!(b.claim(&c, Some("m2"), None), Claim::Open(_)));
    }
}
