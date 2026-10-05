//! The usability test's switch (feature `usability`, design D.13.2 WP-E3, WP-U): the arm without
//! guidance serves 1.2's instructions and `session` description and renders no `next_calls`
//! block; the arm with guidance is the server as specified. The configuration that picks the arm
//! is the engine's (WP-E4); this module holds what each arm serves.
//!
//! The feature is never enabled in a release build: the crate does not compile with it outside a
//! debug build.

#[cfg(not(debug_assertions))]
compile_error!(
    "the usability feature serves 1.2's texts for the usability test only: never in a release build"
);

use rmcp::model::Tool as McpTool;

use forum_core::query::guide::Guidance;

/// §22's instructions in 1.2, 490 characters.
pub const INSTRUCTIONS_1_2: &str = "Swarm forum: shared memory for agents that will not come \
back. Call session hello first; if a human wrote to you, pass their words verbatim and who they \
are as from_user (later messages: talk from_user). node create {keys} finds or creates work by \
key. Take work with hold next; checkpoint about every 5 min. Before any message to a person: talk \
said; to the user: talk to_user. Finished: talk decide, reply, node close. Unfinished: hold \
release. Leave every place tidier than you found it.";

/// §20.2's `session` description in 1.2.
pub const SESSION_1_2: &str = "Call first, then pass the returned session on every call. hello \
{principal?, from_user:{text,client}} logs a human's words verbatim and whose they are. Returns \
holds, inbox, offers, and the questions and digest for your principals: ask and summarise those. \
status {text}.";

/// The two arms of the usability test (WP-U).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Arm {
    /// The server as specified: next calls on every response the core computes.
    #[default]
    WithGuidance,
    /// 1.2's instructions and `session` description, and no `next_calls` block.
    WithoutGuidance,
}

impl Arm {
    /// The instructions this arm serves: §22's for the arm with guidance (`current`), 1.2's
    /// otherwise.
    #[must_use]
    pub fn instructions(self, current: &str) -> &str {
        match self {
            Arm::WithGuidance => current,
            Arm::WithoutGuidance => INSTRUCTIONS_1_2,
        }
    }

    /// The tools this arm serves: the `session` description is 1.2's without guidance.
    #[must_use]
    pub fn tools(self) -> Vec<McpTool> {
        let mut tools = crate::mcp::tools::tools();
        if self == Arm::WithoutGuidance {
            for t in &mut tools {
                if t.name == "session" {
                    t.description = Some(SESSION_1_2.into());
                }
            }
        }
        tools
    }

    /// The next calls the renderer shows: none without guidance.
    #[must_use]
    pub fn guidance(self, g: &Guidance) -> Option<&Guidance> {
        match self {
            Arm::WithGuidance => Some(g),
            Arm::WithoutGuidance => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arm_without_guidance_serves_1_2() {
        assert_eq!(
            INSTRUCTIONS_1_2.chars().count(),
            490,
            "§22 of 1.2: 490 characters"
        );
        let now = crate::mcp::tools::INSTRUCTIONS;
        assert_eq!(Arm::WithGuidance.instructions(now), now);
        assert_eq!(Arm::WithoutGuidance.instructions(now), INSTRUCTIONS_1_2);
        let session = |arm: Arm| {
            arm.tools()
                .into_iter()
                .find(|t| t.name == "session")
                .and_then(|t| t.description)
                .map(|d| d.to_string())
        };
        assert_eq!(
            session(Arm::WithGuidance).as_deref(),
            Some(crate::mcp::tools::SESSION)
        );
        assert_eq!(session(Arm::WithoutGuidance).as_deref(), Some(SESSION_1_2));
        let g = Guidance {
            items: Vec::new(),
            total: forum_core::Total::Exact(0),
            end: None,
        };
        assert!(Arm::WithGuidance.guidance(&g).is_some());
        assert!(Arm::WithoutGuidance.guidance(&g).is_none());
        // The other tools are unchanged.
        let a = Arm::WithGuidance.tools();
        let b = Arm::WithoutGuidance.tools();
        for (x, y) in a.iter().zip(&b) {
            if x.name != "session" {
                assert_eq!(x.description, y.description);
            }
        }
    }
}
