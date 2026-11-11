//! Data-driven agent registry: built-in `agents.toml` merged with the user's
//! `agents.user.toml`, plus PATH detection (see `detect`) and npm shim
//! resolution (see `shim`).

pub mod detect;
pub mod shim;

use serde::{Deserialize, Serialize};
use std::path::Path;

pub use detect::DetectedAgent;

const BUILTIN: &str = include_str!("../../resources/agents.toml");
pub const USER_FILE: &str = "agents.user.toml";

/// One CLI the app knows how to launch. TOML uses snake_case keys; the
/// frontend receives camelCase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct AgentDef {
    pub id: String,
    pub name: String,
    pub commands: Vec<String>,
    #[serde(default)]
    pub version_args: Vec<String>,
    #[serde(default)]
    pub version_regex: Option<String>,
    #[serde(default)]
    pub new_session_args: Vec<String>,
    #[serde(default)]
    pub resume_args: Vec<String>,
    #[serde(default)]
    pub continue_args: Vec<String>,
    #[serde(default)]
    pub hook: Option<HookSpec>,
    #[serde(default)]
    pub config_dir_env: Option<String>,
    #[serde(default)]
    pub image_paste: Option<ImagePaste>,
    #[serde(default = "default_submit")]
    pub submit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookSpec {
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagePaste {
    Path,
    Native,
}

fn default_submit() -> String {
    "\r".into()
}

#[derive(Deserialize)]
struct RegistryFile {
    #[serde(default)]
    agent: Vec<toml::Table>,
}

/// Merge built-in and user TOML. User entries replace built-in keys for the
/// same `id`; new ids are appended. A user entry may omit `id` (defaults to
/// its first command) and `name` (defaults to the id), so a custom command can
/// be just `commands = ["mytool"]`.
pub fn merge(builtin: &str, user: Option<&str>) -> Result<Vec<AgentDef>, String> {
    let mut tables = parse(builtin, "built-in registry")?;
    if let Some(user) = user {
        for entry in parse(user, USER_FILE)? {
            match tables.iter_mut().find(|t| t.get("id") == entry.get("id")) {
                Some(base) => base.extend(entry),
                None => tables.push(entry),
            }
        }
    }
    tables.into_iter().map(into_def).collect()
}

fn parse(src: &str, what: &str) -> Result<Vec<toml::Table>, String> {
    let file: RegistryFile = toml::from_str(src).map_err(|e| format!("{what}: {e}"))?;
    file.agent
        .into_iter()
        .enumerate()
        .map(|(i, mut t)| {
            if !t.contains_key("id") {
                let first = t
                    .get("commands")
                    .and_then(|c| c.as_array())
                    .and_then(|c| c.first())
                    .and_then(|c| c.as_str())
                    .ok_or_else(|| format!("{what}: entry {} has neither `id` nor `commands`", i + 1))?
                    .to_string();
                t.insert("id".into(), first.into());
            }
            Ok(t)
        })
        .collect()
}

fn into_def(mut t: toml::Table) -> Result<AgentDef, String> {
    let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("?").to_string();
    if !t.contains_key("name") {
        t.insert("name".into(), id.clone().into());
    }
    let def: AgentDef = t.try_into().map_err(|e| format!("agent `{id}`: {e}"))?;
    if def.commands.is_empty() {
        return Err(format!("agent `{id}`: `commands` is empty"));
    }
    if let Some(re) = &def.version_regex {
        regex::Regex::new(re).map_err(|e| format!("agent `{id}`: bad version_regex: {e}"))?;
    }
    Ok(def)
}

/// Load the registry, reading `agents.user.toml` from `config_dir` if present.
/// A broken user file is reported and ignored so the built-ins still work.
pub fn load(config_dir: Option<&Path>) -> Vec<AgentDef> {
    let user = config_dir.and_then(|d| std::fs::read_to_string(d.join(USER_FILE)).ok());
    merge(BUILTIN, user.as_deref()).unwrap_or_else(|e| {
        eprintln!("registry: {e}; using built-in agents only");
        merge(BUILTIN, None).expect("built-in agents.toml is valid")
    })
}

/// Load the registry and detect which agents are installed. Blocking: call it
/// off the main thread.
pub fn detect_agents(config_dir: Option<&Path>) -> Vec<DetectedAgent> {
    detect::detect_all(&load(config_dir), config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_parses() {
        let defs = merge(BUILTIN, None).unwrap();
        let claude = defs.iter().find(|d| d.id == "claude").unwrap();
        assert_eq!(claude.name, "Claude Code");
        assert_eq!(claude.submit, "\r");
        assert_eq!(claude.image_paste, Some(ImagePaste::Path));
        assert_eq!(claude.hook.as_ref().unwrap().kind, "claude-settings");
        assert!(defs.iter().any(|d| d.id == "copilot"));
    }

    #[test]
    fn user_entry_overrides_by_key_and_adds_custom() {
        let user = r#"
            [[agent]]
            id = "claude"
            commands = ["claude-beta", "claude"]

            [[agent]]
            commands = ["mytool"]
        "#;
        let defs = merge(BUILTIN, Some(user)).unwrap();
        let claude = defs.iter().find(|d| d.id == "claude").unwrap();
        assert_eq!(claude.commands, ["claude-beta", "claude"]);
        assert_eq!(claude.name, "Claude Code", "untouched keys keep built-in values");
        let custom = defs.last().unwrap();
        assert_eq!((custom.id.as_str(), custom.name.as_str()), ("mytool", "mytool"));
        assert!(custom.version_args.is_empty());
    }

    /// Manual check against this machine: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn detect_on_this_machine() {
        for a in detect_agents(None) {
            println!("{:<14} {:?} {:?} {:?}", a.def.id, a.version, a.launch.kind, a.launch.program);
        }
    }

    #[test]
    fn bad_user_entries_are_errors() {
        assert!(merge(BUILTIN, Some("[[agent]]\nname = \"x\"")).is_err());
        assert!(merge(BUILTIN, Some("[[agent]]\nid = \"x\"\ncommands = []")).is_err());
        assert!(merge(BUILTIN, Some("[[agent]]\ncommands = [\"x\"]\nversion_regex = \"(\"")).is_err());
        assert!(merge(BUILTIN, Some("not toml [")).is_err());
    }
}
