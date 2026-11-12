//! Agent status (PLAN.md feature 4): `starting → working → idle | needs_input
//! → exited`. This module holds the state machine and the hook source: a
//! named pipe that the `spectacli-notify` helper writes to when a CLI's hook
//! fires. Terminal signals and output heuristics feed the same `Tracker`.

mod pipe;

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Starting,
    Working,
    Idle,
    NeedsInput,
    Exited,
}

/// Where a status came from, shown in the pane header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Launch,
    Hook,
    Exit,
}

/// Payload of the `agent:status` event.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusChange {
    pub agent_id: String,
    pub status: Status,
    pub source: Source,
    /// Unix time in ms.
    pub at: u64,
}

pub type Emit = Arc<dyn Fn(StatusChange) + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Current status per tracked agent. Only changes are emitted; events for
/// agents that are unknown or already exited are dropped.
pub struct Tracker {
    agents: Mutex<HashMap<String, Status>>,
    emit: Emit,
}

impl Tracker {
    pub fn new(emit: Emit) -> Self {
        Self { agents: Mutex::new(HashMap::new()), emit }
    }

    /// Start tracking an agent that is about to launch.
    pub fn start(&self, id: &str) {
        lock(&self.agents).insert(id.into(), Status::Starting);
        self.send(id, Status::Starting, Source::Launch);
    }

    /// Stop tracking without an event (the launch failed).
    pub fn forget(&self, id: &str) {
        lock(&self.agents).remove(id);
    }

    pub fn set(&self, id: &str, status: Status, source: Source) {
        {
            let mut agents = lock(&self.agents);
            match agents.get_mut(id) {
                Some(s) if *s != status => *s = status,
                _ => return,
            }
        }
        self.send(id, status, source);
    }

    /// The process is gone. Later events for this id are ignored.
    pub fn exited(&self, id: &str) {
        if lock(&self.agents).remove(id).is_some() {
            self.send(id, Status::Exited, Source::Exit);
        }
    }

    fn send(&self, id: &str, status: Status, source: Source) {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        (self.emit)(StatusChange { agent_id: id.into(), status, source, at });
    }
}

/// Map a hook event from the helper to a status. `None` means "no change".
///
/// Claude (`--settings` hooks, JSON on stdin):
/// - `claude-session-start` → idle: the prompt is up.
/// - `claude-prompt` (UserPromptSubmit) → working.
/// - `claude-tool` (PostToolUse) → working: also ends `needs_input` once a
///   permission prompt was answered with yes.
/// - `claude-notification` → needs input, except the "waiting for your input"
///   reminder Claude sends after 60 s idle, and auth notices.
/// - `claude-stop` → idle: the turn finished.
///
/// Codex (`notify`, JSON as the last argument): `agent-turn-complete` → idle;
/// any approval notification → needs input.
pub fn classify(event: &str, payload: &[u8]) -> Option<Status> {
    let json = || serde_json::from_slice::<serde_json::Value>(payload).unwrap_or_default();
    let field = |v: &serde_json::Value, k: &str| v.get(k).and_then(|s| s.as_str()).unwrap_or("").to_ascii_lowercase();
    match event {
        "claude-session-start" | "claude-stop" => Some(Status::Idle),
        "claude-prompt" | "claude-tool" => Some(Status::Working),
        "claude-notification" => {
            let v = json();
            let (kind, message) = (field(&v, "notification_type"), field(&v, "message"));
            let reminder = kind == "idle_prompt" || message.contains("waiting for your input");
            let auth = kind.starts_with("auth");
            (!reminder && !auth).then_some(Status::NeedsInput)
        }
        "codex" => {
            let kind = field(&json(), "type");
            if kind == "agent-turn-complete" {
                Some(Status::Idle)
            } else if kind.contains("approval") {
                Some(Status::NeedsInput)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Everything needed to inject hooks into a launch.
pub struct Hooks {
    pipe: String,
    helper: PathBuf,
}

/// Seconds Claude waits for a hook before giving up on it.
const CLAUDE_HOOK_TIMEOUT: u32 = 5;

impl Hooks {
    /// Extra launch args that make a CLI with hook support `kind` report to
    /// us. Empty for kinds we do not know.
    pub fn args(&self, kind: &str, agent_id: &str) -> Vec<String> {
        match kind {
            "claude-settings" => claude_args(&self.helper, &self.pipe, agent_id).unwrap_or_default(),
            "codex-notify" => codex_args(&self.helper, &self.pipe, agent_id),
            _ => vec![],
        }
    }
}

/// `--settings <json>` with our hooks. Claude layers these over the user's
/// own settings files, so their hooks still run.
fn claude_args(helper: &Path, pipe: &str, agent_id: &str) -> Option<Vec<String>> {
    // Claude runs hook commands through a shell (Git Bash on Windows, if
    // present). A quoted forward-slash path works in bash and cmd alike;
    // refuse paths with characters either shell would expand.
    let exe = helper.to_str()?.replace('\\', "/");
    if exe.contains(['"', '`', '$', '%', '!']) {
        eprintln!("status: helper path {exe:?} can't be quoted for a shell; Claude hooks disabled");
        return None;
    }
    let hook = |event: &str| {
        serde_json::json!([{ "hooks": [{
            "type": "command",
            "command": format!("\"{exe}\" {pipe} {agent_id} {event}"),
            "timeout": CLAUDE_HOOK_TIMEOUT,
        }]}])
    };
    let settings = serde_json::json!({ "hooks": {
        "SessionStart": hook("claude-session-start"),
        "UserPromptSubmit": hook("claude-prompt"),
        "PostToolUse": hook("claude-tool"),
        "Notification": hook("claude-notification"),
        "Stop": hook("claude-stop"),
    }});
    Some(vec!["--settings".into(), settings.to_string()])
}

/// `-c notify=[...]`. Codex runs it directly (no shell) and appends its
/// JSON payload. This replaces a `notify` the user set in config.toml for
/// this launch only.
fn codex_args(helper: &Path, pipe: &str, agent_id: &str) -> Vec<String> {
    // A JSON array of strings is also a valid TOML array.
    let cmd = [helper.to_string_lossy().into_owned(), pipe.into(), agent_id.into(), "codex".into()];
    vec!["-c".into(), format!("notify={}", serde_json::Value::from(cmd.to_vec()))]
}

/// The status tracker plus, when the pipe and helper are available, the hook
/// injection for launches.
pub struct StatusHub {
    pub tracker: Arc<Tracker>,
    pub hooks: Option<Hooks>,
}

impl StatusHub {
    /// Start the pipe server and find the helper next to our exe. Without
    /// either, agents still run; they just get no hook status.
    pub fn start(emit: Emit) -> Self {
        let tracker = Arc::new(Tracker::new(emit));
        let helper = std::env::current_exe().ok().map(|e| e.with_file_name("spectacli-notify.exe"));
        let hooks = match helper.filter(|h| h.is_file()) {
            None => {
                eprintln!("status: spectacli-notify.exe not found next to the app; hook status disabled");
                None
            }
            Some(helper) => {
                let pipe = format!("spectacli-{}-{}", std::process::id(), uuid::Uuid::now_v7().simple());
                let t = tracker.clone();
                match pipe::serve(&pipe, move |agent, event, payload| {
                    if let Some(s) = classify(event, payload) {
                        t.set(agent, s, Source::Hook);
                    }
                }) {
                    Ok(()) => Some(Hooks { pipe, helper }),
                    Err(e) => {
                        eprintln!("status: {e}; hook status disabled");
                        None
                    }
                }
            }
        };
        Self { tracker, hooks }
    }

    /// Launch args for `hook_kind` and start tracking `agent_id`. Agents
    /// without a hook are not tracked (no source would ever update them).
    pub fn prepare(&self, agent_id: &str, hook_kind: Option<&str>) -> Vec<String> {
        let args = match (&self.hooks, hook_kind) {
            (Some(h), Some(kind)) => h.args(kind, agent_id),
            _ => vec![],
        };
        if !args.is_empty() {
            self.tracker.start(agent_id);
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Log = Arc<Mutex<Vec<(Status, Source)>>>;

    fn tracker() -> (Tracker, Log) {
        let log = Log::default();
        let l = log.clone();
        (Tracker::new(Arc::new(move |c| lock(&l).push((c.status, c.source)))), log)
    }

    #[test]
    fn emits_changes_only_for_live_agents() {
        let (t, log) = tracker();
        t.set("a", Status::Working, Source::Hook); // unknown: dropped
        t.start("a");
        t.set("a", Status::Working, Source::Hook);
        t.set("a", Status::Working, Source::Hook); // unchanged: dropped
        t.set("a", Status::Idle, Source::Hook);
        t.exited("a");
        t.set("a", Status::Working, Source::Hook); // after exit: dropped
        t.exited("a");
        assert_eq!(
            *lock(&log),
            [
                (Status::Starting, Source::Launch),
                (Status::Working, Source::Hook),
                (Status::Idle, Source::Hook),
                (Status::Exited, Source::Exit)
            ]
        );
    }

    #[test]
    fn classifies_claude_events() {
        assert_eq!(classify("claude-stop", b""), Some(Status::Idle));
        assert_eq!(classify("claude-prompt", b"{}"), Some(Status::Working));
        let perm = br#"{"hook_event_name":"Notification","message":"Claude needs your permission to use Bash","notification_type":"permission_prompt"}"#;
        assert_eq!(classify("claude-notification", perm), Some(Status::NeedsInput));
        let old = br#"{"message":"Claude needs your permission to use Bash"}"#;
        assert_eq!(classify("claude-notification", old), Some(Status::NeedsInput));
        let idle = br#"{"message":"Claude is waiting for your input"}"#;
        assert_eq!(classify("claude-notification", idle), None);
        assert_eq!(classify("claude-notification", br#"{"notification_type":"idle_prompt"}"#), None);
        assert_eq!(classify("claude-notification", br#"{"notification_type":"auth_success"}"#), None);
    }

    #[test]
    fn classifies_codex_events() {
        let done = br#"{"type":"agent-turn-complete","thread-id":"x","last-assistant-message":"ok"}"#;
        assert_eq!(classify("codex", done), Some(Status::Idle));
        assert_eq!(classify("codex", br#"{"type":"approval-requested"}"#), Some(Status::NeedsInput));
        assert_eq!(classify("codex", b"not json"), None);
        assert_eq!(classify("bogus", b""), None);
    }

    #[test]
    fn builds_claude_settings() {
        let args = claude_args(Path::new(r"C:\Program Files\SpectaCLI\spectacli-notify.exe"), "spectacli-1-ab", "id1").unwrap();
        assert_eq!(args[0], "--settings");
        let v: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, r#""C:/Program Files/SpectaCLI/spectacli-notify.exe" spectacli-1-ab id1 claude-stop"#);
        for e in ["SessionStart", "UserPromptSubmit", "PostToolUse", "Notification"] {
            assert!(v["hooks"][e].is_array(), "{e}");
        }
        assert!(claude_args(Path::new(r"C:\a%b\x.exe"), "p", "i").is_none());
    }

    #[test]
    fn builds_codex_notify_as_toml() {
        let args = codex_args(Path::new(r"C:\Program Files\x.exe"), "spectacli-1-ab", "id1");
        assert_eq!(args[0], "-c");
        let v: toml::Table = toml::from_str(&args[1]).unwrap();
        let cmd: Vec<_> = v["notify"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
        assert_eq!(cmd, [r"C:\Program Files\x.exe", "spectacli-1-ab", "id1", "codex"]);
    }
}
