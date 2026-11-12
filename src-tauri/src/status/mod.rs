//! Agent status (PLAN.md feature 4): `starting → working → idle | needs_input
//! → exited`. This module holds the state machine and the hook source: a
//! named pipe that the `spectacli-notify` helper writes to when a CLI's hook
//! fires. Terminal signals and output heuristics feed the same `Tracker`.

mod pipe;
mod signals;

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use regex::Regex;
use signals::{Parser, Signal};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    /// An injected CLI hook: accurate, and once an agent has reported one,
    /// the weaker sources below are ignored for it.
    Hook,
    /// BEL or an OSC notification/progress sequence in the output.
    Terminal,
    /// A guess: the prompt regex, user input, or a quiet spell after output.
    Heuristic,
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

/// Silence after output that makes a `working` agent without hooks `idle`.
const QUIET: Duration = Duration::from_secs(3);
/// Characters of recent output the prompt regex is matched against.
const TAIL: usize = 400;
/// Used for agents whose registry entry has no `prompt_regex`.
pub const DEFAULT_PROMPT: &str = r"(?i)\(y/n\)|\[y/n\]|\(y\)es|do you want to |allow\?";

struct Entry {
    status: Status,
    /// A hook reported for this agent: it is the only source we trust now.
    hooked: bool,
    parser: Parser,
    /// Recent printable output, for `prompt`.
    tail: String,
    prompt: Option<Regex>,
    last_output: Option<Instant>,
}

/// Current status per tracked agent. Only changes are emitted; events for
/// agents that are unknown or already exited are dropped.
pub struct Tracker {
    agents: Mutex<HashMap<String, Entry>>,
    emit: Emit,
}

impl Tracker {
    pub fn new(emit: Emit) -> Self {
        Self { agents: Mutex::new(HashMap::new()), emit }
    }

    /// Start tracking an agent that is about to launch. `prompt` is the
    /// regex for the output heuristic.
    pub fn start(&self, id: &str, prompt: Option<Regex>) {
        let entry = Entry {
            status: Status::Starting,
            hooked: false,
            parser: Parser::default(),
            tail: String::new(),
            prompt,
            last_output: None,
        };
        lock(&self.agents).insert(id.into(), entry);
        self.send(id, Status::Starting, Source::Launch);
    }

    /// Stop tracking without an event (the launch failed).
    pub fn forget(&self, id: &str) {
        lock(&self.agents).remove(id);
    }

    pub fn set(&self, id: &str, status: Status, source: Source) {
        {
            let mut agents = lock(&self.agents);
            let Some(e) = agents.get_mut(id) else { return };
            if source == Source::Hook {
                e.hooked = true;
            } else if e.hooked {
                return;
            }
            if e.status == status {
                return;
            }
            e.status = status;
        }
        self.send(id, status, source);
    }

    /// The process is gone. Later events for this id are ignored.
    pub fn exited(&self, id: &str) {
        if lock(&self.agents).remove(id).is_some() {
            self.send(id, Status::Exited, Source::Exit);
        }
    }

    /// Output bytes from the agent: terminal signals, then the prompt regex.
    /// Does nothing for agents that report through hooks.
    pub fn output(&self, id: &str, data: &[u8], now: Instant) {
        let mut found = vec![];
        let mut asked = false;
        {
            let mut agents = lock(&self.agents);
            let Some(e) = agents.get_mut(id).filter(|e| !e.hooked) else { return };
            e.last_output = Some(now);
            let mut text = vec![];
            e.parser.feed(data, &mut found, &mut text);
            if let Some(re) = &e.prompt
                && !text.is_empty()
            {
                e.tail.push_str(&String::from_utf8_lossy(&text));
                let cut = e.tail.char_indices().rev().nth(TAIL).map_or(0, |(i, _)| i);
                e.tail.drain(..cut);
                if re.is_match(&e.tail) {
                    e.tail.clear();
                    asked = true;
                }
            }
        }
        // Not under the lock: `set` takes it again.
        for s in found {
            self.apply(id, s);
        }
        if asked {
            self.set(id, Status::NeedsInput, Source::Heuristic);
        }
    }

    fn apply(&self, id: &str, s: Signal) {
        match s {
            Signal::Attention => self.set(id, Status::NeedsInput, Source::Terminal),
            Signal::Progress(true) => self.set(id, Status::Working, Source::Terminal),
            Signal::Progress(false) => self.set(id, Status::Idle, Source::Terminal),
        }
    }

    /// The user typed into the agent. Enter on an idle agent submits a
    /// prompt, and on one that asked for input it answers; either way it is
    /// working again. Does nothing for agents that report through hooks.
    pub fn user_input(&self, id: &str, data: &[u8], now: Instant) {
        if !data.contains(&b'\r') {
            return;
        }
        let answered = {
            let mut agents = lock(&self.agents);
            let Some(e) = agents.get_mut(id).filter(|e| !e.hooked) else { return };
            e.tail.clear();
            e.last_output = Some(now);
            matches!(e.status, Status::Idle | Status::NeedsInput)
        };
        if answered {
            self.set(id, Status::Working, Source::Heuristic);
        }
    }

    /// Call every few hundred ms: agents without hooks that went quiet
    /// after output are idle.
    pub fn tick(&self, now: Instant) {
        let quiet: Vec<String> = lock(&self.agents)
            .iter()
            .filter(|(_, e)| !e.hooked && matches!(e.status, Status::Starting | Status::Working))
            .filter(|(_, e)| e.last_output.is_some_and(|t| now.saturating_duration_since(t) >= QUIET))
            .map(|(id, _)| id.clone())
            .collect();
        for id in quiet {
            self.set(&id, Status::Idle, Source::Heuristic);
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
            "codex-notify" => codex_args(&self.helper, &self.pipe, agent_id, &codex_user_notify()),
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
/// JSON payload. Codex takes only one `notify`, so the user's own (`chain`)
/// goes after `--` and the helper runs it too with the same payload.
fn codex_args(helper: &Path, pipe: &str, agent_id: &str, chain: &[String]) -> Vec<String> {
    let mut cmd = vec![helper.to_string_lossy().into_owned(), pipe.into(), agent_id.into(), "codex".into()];
    if !chain.is_empty() {
        cmd.push("--".into());
        cmd.extend_from_slice(chain);
    }
    // A JSON array of strings is also a valid TOML array.
    vec!["-c".into(), format!("notify={}", serde_json::Value::from(cmd))]
}

/// The `notify` command from the user's Codex `config.toml`
/// (`$CODEX_HOME`, else `~/.codex`), if any. Ours is skipped so a stale
/// entry can't make the helper call itself.
fn codex_user_notify() -> Vec<String> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".codex")));
    let text = home.and_then(|h| std::fs::read_to_string(h.join("config.toml")).ok()).unwrap_or_default();
    parse_codex_notify(&text)
}

fn parse_codex_notify(config: &str) -> Vec<String> {
    let table: toml::Table = toml::from_str(config).unwrap_or_default();
    let cmd: Vec<String> = table
        .get("notify")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let ours = cmd.first().is_some_and(|p| p.to_ascii_lowercase().ends_with("spectacli-notify.exe"));
    if ours { vec![] } else { cmd }
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
        let ticker = tracker.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                ticker.tick(Instant::now());
            }
        });
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

    /// Launch args for `hook_kind`, and start tracking `agent_id`. Agents
    /// without a hook are tracked too: terminal signals and the `prompt`
    /// regex (default `DEFAULT_PROMPT`) are their only sources.
    pub fn prepare(&self, agent_id: &str, hook_kind: Option<&str>, prompt: Option<&str>) -> Vec<String> {
        let args = match (&self.hooks, hook_kind) {
            (Some(h), Some(kind)) => h.args(kind, agent_id),
            _ => vec![],
        };
        let prompt = Regex::new(prompt.unwrap_or(DEFAULT_PROMPT))
            .map_err(|e| eprintln!("status: bad prompt regex for {agent_id}: {e}"))
            .ok();
        self.tracker.start(agent_id, prompt);
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
        t.start("a", None);
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

    fn quiet_tracker(prompt: Option<&str>) -> (Tracker, Log) {
        let (t, log) = tracker();
        t.start("a", prompt.map(|p| Regex::new(p).unwrap()));
        (t, log)
    }

    #[test]
    fn terminal_signals_set_status_until_a_hook_reports() {
        let (t, log) = quiet_tracker(None);
        let now = Instant::now();
        t.output("a", b"]9;4;3;", now);
        t.output("a", b"", now);
        t.set("a", Status::Idle, Source::Hook);
        t.output("a", b"", now); // ignored: hooks own this agent now
        t.tick(now + Duration::from_secs(60));
        assert_eq!(
            *lock(&log),
            [
                (Status::Starting, Source::Launch),
                (Status::Working, Source::Terminal),
                (Status::NeedsInput, Source::Terminal),
                (Status::Idle, Source::Hook)
            ]
        );
    }

    #[test]
    fn prompt_regex_matches_across_chunks_and_ignores_escapes() {
        let (t, log) = quiet_tracker(Some(r"\(y/n\)"));
        let now = Instant::now();
        t.output("a", b"[1mApply?[0m (y", now);
        assert_eq!(lock(&log).len(), 1);
        t.output("a", b"/n)", now);
        assert_eq!(lock(&log).last(), Some(&(Status::NeedsInput, Source::Heuristic)));
    }

    #[test]
    fn enter_means_working_and_quiet_means_idle() {
        let (t, log) = quiet_tracker(None);
        let t0 = Instant::now();
        t.output("a", b"banner", t0);
        t.tick(t0 + Duration::from_secs(1)); // still within the quiet window
        t.tick(t0 + QUIET);
        t.user_input("a", b"hello", t0 + QUIET); // no Enter yet
        t.user_input("a", b"\r", t0 + QUIET);
        t.tick(t0 + QUIET * 2); // Enter reset the quiet clock
        t.tick(t0 + QUIET * 2 + QUIET);
        let states: Vec<_> = lock(&log).iter().map(|s| s.0).collect();
        assert_eq!(states, [Status::Starting, Status::Idle, Status::Working, Status::Idle]);
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

    /// Live check against the installed CLI, on the same path the app uses:
    /// registry → hook args → ConPTY → helper → pipe → tracker. Sends one
    /// prompt and prints every status. Uses your CLI account (one short
    /// prompt). Needs `npm run notify` first:
    /// `cargo test live_ -- --ignored --nocapture --test-threads 1`
    #[cfg(windows)]
    fn root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    #[cfg(windows)]
    fn live(agent: &str, cwd: &Path, extra: &[&str], prompt: &str, done: impl Fn(&[Status]) -> bool) -> Vec<(Status, u128)> {
        use crate::pty::{PtyManager, SpawnRequest};
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let root = root();
        let helper = root.join("target/debug/spectacli-notify.exe");
        assert!(helper.is_file(), "run `npm run notify` first");
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let tracker = Arc::new(Tracker::new(Arc::new(move |c| drop(lock(&tx).send(c)))));
        let start = Instant::now();
        let pipe = format!("spectacli-live-{}", uuid::Uuid::now_v7().simple());
        let t = tracker.clone();
        pipe::serve(&pipe, move |a, e, p| {
            eprintln!("{:>6} ms  hook {e}", start.elapsed().as_millis());
            if let Some(s) = classify(e, p) {
                t.set(a, s, Source::Hook);
            }
        })
        .unwrap();
        let hooks = Hooks { pipe, helper };

        let det = crate::registry::detect_agents(None).into_iter().find(|a| a.def.id == agent).expect("installed");
        let id = format!("live-{agent}");
        let mut args = hooks.args(&det.def.hook.as_ref().unwrap().kind, &id);
        args.extend(extra.iter().map(|a| a.to_string()));
        assert!(!args.is_empty());
        tracker.start(&id, None);
        let pty = PtyManager::new(Arc::new(|_, _| {}));
        let out = Arc::new(Mutex::new(Vec::new()));
        let o = out.clone();
        pty.spawn(SpawnRequest {
            id: Some(id.clone()),
            launch: det.launch,
            args,
            cwd: cwd.into(),
            env: vec![],
            cols: 120,
            rows: 40,
        })
        .unwrap();
        pty.attach(&id, 1, Box::new(move |d| {
            lock(&o).extend_from_slice(&d);
            true
        }))
        .unwrap();

        let mut seen = vec![];
        let mut sent = false;
        while start.elapsed() < Duration::from_secs(120) && !done(&seen.iter().map(|s: &(Status, u128)| s.0).collect::<Vec<_>>()) {
            if let Ok(c) = rx.recv_timeout(Duration::from_millis(250)) {
                eprintln!("{:>6} ms  -> {:?} via {:?}", start.elapsed().as_millis(), c.status, c.source);
                seen.push((c.status, start.elapsed().as_millis()));
            }
            // Type once the input box is up (idle from a hook, or after a wait).
            let ready = seen.iter().any(|s| s.0 == Status::Idle) || start.elapsed() > Duration::from_secs(10);
            // Codex may ask to review the user's own changed hooks, which
            // eats the prompt; skip it (Esc, saves no trust decision) and
            // type the prompt again.
            if seen.len() < 2 && String::from_utf8_lossy(&lock(&out)).contains("Review hooks") {
                eprintln!("{:>6} ms  skipping Codex hook review", start.elapsed().as_millis());
                lock(&out).clear();
                pty.write(&id, b"\x1b").unwrap();
                std::thread::sleep(Duration::from_millis(1000));
                sent = false;
            }
            if !sent && ready {
                std::thread::sleep(Duration::from_millis(1500));
                pty.write(&id, prompt.as_bytes()).unwrap();
                std::thread::sleep(Duration::from_millis(500));
                pty.write(&id, b"\r").unwrap();
                eprintln!("{:>6} ms  prompt sent", start.elapsed().as_millis());
                sent = true;
            }
        }
        let text = String::from_utf8_lossy(&lock(&out)).into_owned();
        let tail: String = text.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect();
        eprintln!("--- last output ---\n{tail}\n---");
        let _ = pty.write(&id, b"\x1b");
        pty.kill(&id).unwrap();
        seen
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_claude() {
        let seen = live("claude", root().parent().unwrap(), &["--permission-mode", "default"], "Use the Write tool to create C:/Windows/Temp/spectacli-live-check.txt containing ok", |s| {
            s.contains(&Status::Working) && (s.contains(&Status::NeedsInput) || s.ends_with(&[Status::Idle]))
        });
        let s: Vec<_> = seen.iter().map(|s| s.0).collect();
        assert!(s.contains(&Status::Working), "{s:?}");
        assert!(s.contains(&Status::NeedsInput) || s.ends_with(&[Status::Idle]), "{s:?}");
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_codex() {
        // An empty folder, trusted for this launch only, so Codex skips its
        // trust dialog without saving anything to the user's config.
        let dir = std::env::temp_dir().join(format!("spectacli-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.to_string_lossy().to_lowercase();
        let trust = format!("projects.{}.trust_level=\"trusted\"", serde_json::Value::from(key));
        let seen = live("codex", &dir, &["-c", &trust], "Reply with just the word OK. Do not run any commands.", |s| s.contains(&Status::Idle));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(seen.iter().any(|s| s.0 == Status::Idle), "{seen:?}");
    }

    #[test]
    fn builds_codex_notify_as_toml() {
        let notify = |chain: &[String]| {
            let args = codex_args(Path::new(r"C:\Program Files\x.exe"), "spectacli-1-ab", "id1", chain);
            assert_eq!(args[0], "-c");
            let v: toml::Table = toml::from_str(&args[1]).unwrap();
            v["notify"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect::<Vec<_>>()
        };
        assert_eq!(notify(&[]), [r"C:\Program Files\x.exe", "spectacli-1-ab", "id1", "codex"]);
        let user = [r#"C:\a "b"\n.exe"#.to_string(), "turn-ended".into()];
        assert_eq!(notify(&user), [r"C:\Program Files\x.exe", "spectacli-1-ab", "id1", "codex", "--", r#"C:\a "b"\n.exe"#, "turn-ended"]);
    }

    #[test]
    fn reads_user_codex_notify() {
        let cfg = "model = \"x\"\nnotify = [ \"C:\\\\tools\\\\n.exe\", \"turn-ended\" ]\n[tui]\nx = 1\n";
        assert_eq!(parse_codex_notify(cfg), [r"C:\tools\n.exe", "turn-ended"]);
        assert!(parse_codex_notify("[tui]\nx = 1").is_empty());
        assert!(parse_codex_notify("not toml [").is_empty());
        assert!(parse_codex_notify(r#"notify = ['C:\x\Spectacli-Notify.exe', "p"]"#).is_empty(), "never chain to ourselves");
    }
}
