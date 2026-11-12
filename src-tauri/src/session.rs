//! Session arguments from the registry's templates (PLAN.md feature 2).
//!
//! Best to weakest: assign the id up front (`new_session_args` with
//! `{session_uuid}`) and resume by it; resume "the latest here" (templates
//! with no placeholder, like Codex `resume --last`); or no resume at all.

use crate::registry::AgentDef;
use serde::Serialize;

pub const PLACEHOLDER: &str = "{session_uuid}";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArgs {
    /// Goes before any other args (Codex's `resume` is a subcommand).
    #[serde(skip)]
    pub args: Vec<String>,
    /// The id the CLI was told to use; save it to resume later.
    pub session_id: Option<String>,
    /// The launch continues an earlier session.
    pub resumed: bool,
    /// The CLI can resume at all, so the agent can be hibernated.
    pub resumable: bool,
}

fn uses_id(template: &[String]) -> bool {
    template.iter().any(|a| a.contains(PLACEHOLDER))
}

fn fill(template: &[String], id: &str) -> Vec<String> {
    template.iter().map(|a| a.replace(PLACEHOLDER, id)).collect()
}

/// Only real UUIDs reach the command line: the saved id comes from a file
/// the user can edit, and it must never turn into a flag.
fn valid_id(id: Option<&str>) -> Option<String> {
    id.and_then(|s| uuid::Uuid::parse_str(s).ok()).map(|u| u.hyphenated().to_string())
}

/// Arguments for launching `def`. `resume` asks to continue `session_id` (or
/// the latest session, for CLIs that pick their own ids); when that can't be
/// done the launch starts a new session instead, keeping `session_id`.
pub fn args(def: &AgentDef, session_id: Option<&str>, resume: bool) -> SessionArgs {
    let id = valid_id(session_id);
    let resumable = !def.resume_args.is_empty();
    if resume && resumable {
        match (uses_id(&def.resume_args), &id) {
            (true, Some(id)) => {
                return SessionArgs { args: fill(&def.resume_args, id), session_id: Some(id.clone()), resumed: true, resumable };
            }
            (false, _) => return SessionArgs { args: def.resume_args.clone(), session_id: None, resumed: true, resumable },
            (true, None) => {} // nothing to resume by: start fresh
        }
    }
    if uses_id(&def.new_session_args) {
        let id = id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        SessionArgs { args: fill(&def.new_session_args, &id), session_id: Some(id), resumed: false, resumable }
    } else {
        SessionArgs { args: def.new_session_args.clone(), session_id: None, resumed: false, resumable }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::merge;

    fn def(id: &str) -> AgentDef {
        merge(include_str!("../resources/agents.toml"), None).unwrap().into_iter().find(|d| d.id == id).unwrap()
    }

    const ID: &str = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";

    #[test]
    fn claude_new_session_gets_an_id_up_front() {
        let s = args(&def("claude"), None, false);
        let id = s.session_id.clone().unwrap();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(s.args, ["--session-id", id.as_str()]);
        assert!(!s.resumed && s.resumable);
    }

    #[test]
    fn claude_resumes_by_id() {
        let s = args(&def("claude"), Some(ID), true);
        assert_eq!(s.args, ["--resume", ID]);
        assert_eq!(s.session_id.as_deref(), Some(ID));
        assert!(s.resumed);
    }

    #[test]
    fn claude_fresh_launch_keeps_a_given_id() {
        // The fallback after a failed resume: same id, new session.
        let s = args(&def("claude"), Some(ID), false);
        assert_eq!(s.args, ["--session-id", ID]);
    }

    #[test]
    fn claude_resume_without_id_starts_fresh() {
        let s = args(&def("claude"), None, true);
        assert_eq!(s.args[0], "--session-id");
        assert!(!s.resumed);
    }

    #[test]
    fn invalid_saved_id_never_reaches_the_command_line() {
        for bad in ["--dangerously-skip-permissions", "", "123", "x y"] {
            let s = args(&def("claude"), Some(bad), true);
            assert!(!s.resumed);
            assert!(!s.args.iter().any(|a| a.contains(bad) && !bad.is_empty()), "{bad}: {:?}", s.args);
        }
    }

    #[test]
    fn codex_resumes_the_latest_and_starts_plain() {
        let r = args(&def("codex"), None, true);
        assert_eq!(r.args, ["resume", "--last"]);
        assert!(r.resumed && r.session_id.is_none());
        let n = args(&def("codex"), None, false);
        assert!(n.args.is_empty() && !n.resumed && n.resumable);
    }

    #[test]
    fn cli_without_resume_is_not_resumable() {
        let s = args(&def("gemini"), Some(ID), true);
        assert!(s.args.is_empty() && !s.resumed && !s.resumable && s.session_id.is_none());
    }
}
