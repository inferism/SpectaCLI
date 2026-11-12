mod panic_log;
mod pty;
mod registry;
mod session;
mod shell;
mod stats;
mod status;
mod store;
mod worktree;

use pty::{PtyManager, SpawnRequest};
use status::StatusHub;
use registry::DetectedAgent;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, Emitter, Manager, State};

/// Last detection result, so spawning doesn't re-walk PATH.
#[derive(Default)]
struct Detected(Mutex<Vec<DetectedAgent>>);

fn lock_detected(app: &AppHandle) -> std::sync::MutexGuard<'_, Vec<DetectedAgent>> {
    app.state::<Detected>().inner().0.lock().unwrap_or_else(|e| e.into_inner())
}

fn detect_and_store(app: &AppHandle) -> Vec<DetectedAgent> {
    let dir = app.path().app_config_dir().ok();
    let agents = registry::detect_agents(dir.as_deref());
    *app.state::<Detected>().0.lock().unwrap_or_else(|e| e.into_inner()) = agents.clone();
    agents
}

/// Installed agents from the registry. Runs off the main thread; repeat calls
/// are fast because probe results are cached.
#[tauri::command]
async fn detect_agents(app: AppHandle) -> Result<Vec<DetectedAgent>, String> {
    tauri::async_runtime::spawn_blocking(move || detect_and_store(&app)).await.map_err(|e| e.to_string())
}

/// Windows errors from starting an exe that its own updater is replacing:
/// half-written (216 bad machine type, 193 not a valid app), locked (32), or
/// briefly gone between delete and rename (2).
fn is_mid_update(err: &str) -> bool {
    ["(os error 216)", "(os error 193)", "(os error 32)", "(os error 2)"].iter().any(|c| err.contains(c))
}

/// Waits between attempts while a CLI updates itself (about 10 s in all).
const UPDATE_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(3),
    Duration::from_secs(4),
];

/// Run `attempt(n)` and retry while it fails the way a mid-update exe does.
/// Once one attempt looked mid-update, "not installed" also retries: the
/// detection probe can reject the exe while it is still being written.
fn retry_mid_update(
    mut attempt: impl FnMut(usize) -> Result<String, String>,
    sleep: impl Fn(Duration),
) -> Result<String, RetryError> {
    let mut updating = false;
    for n in 0..=UPDATE_RETRY_DELAYS.len() {
        match attempt(n) {
            Ok(id) => return Ok(id),
            Err(e) => {
                updating |= is_mid_update(&e);
                let retryable = is_mid_update(&e) || (updating && e.contains("is not installed"));
                match UPDATE_RETRY_DELAYS.get(n) {
                    Some(d) if retryable => sleep(*d),
                    _ => return Err(RetryError { detail: e, updating }),
                }
            }
        }
    }
    unreachable!("the last attempt always returns")
}

#[derive(Debug)]
struct RetryError {
    detail: String,
    /// Some attempt failed like an exe being replaced.
    updating: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Spawned {
    agent_id: String,
    #[serde(flatten)]
    session: session::SessionArgs,
}

/// Start a detected agent in `cwd`; attach a pane with `attach_agent`.
/// `resume` continues `session_id` (or the CLI's latest session here) when
/// the registry says how; otherwise it starts a new session. Retries for a
/// few seconds if the CLI is updating itself.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn spawn_agent(
    app: AppHandle,
    agent_def_id: String,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
    resume: Option<bool>,
) -> Result<Spawned, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let find = |agents: &[DetectedAgent]| agents.iter().find(|a| a.def.id == agent_def_id).cloned();
        let mut name = agent_def_id.clone();
        // Chosen up front: hook arguments carry it.
        let id = uuid::Uuid::now_v7().to_string();
        let status = app.state::<StatusHub>();
        let mut session = None;
        let result = retry_mid_update(
            |n| {
                // Retries detect again: an update can move or replace the exe.
                let cached = if n == 0 { find(&app.state::<Detected>().0.lock().unwrap_or_else(|e| e.into_inner())) } else { None };
                let agent = cached
                    .or_else(|| find(&detect_and_store(&app)))
                    .ok_or_else(|| format!("agent `{agent_def_id}` is not installed"))?;
                name = agent.def.name.clone();
                let s = session::args(&agent.def, session_id.as_deref(), resume.unwrap_or(false));
                // Session args first: Codex's `resume` is a subcommand.
                let mut args = s.args.clone();
                session = Some(s);
                // Tracking starts before launch so an early hook isn't lost.
                args.extend(status.prepare(&id, agent.def.hook.as_ref().map(|h| h.kind.as_str()), agent.def.prompt_regex.as_deref()));
                app.state::<PtyManager>().spawn(SpawnRequest {
                    id: Some(id.clone()),
                    launch: agent.launch,
                    args,
                    cwd: cwd.clone(),
                    env: vec![],
                    cols,
                    rows,
                })
            },
            std::thread::sleep,
        );
        result
            .map(|agent_id| Spawned { agent_id, session: session.expect("set before a successful spawn") })
            .map_err(|e| {
            status.tracker.forget(&id);
            eprintln!("spawn {name}: {}", e.detail);
            if e.updating {
                format!("{name} could not start because it is updating itself. Try again in a moment.")
            } else {
                e.detail
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start a user-typed command line in `cwd` through PowerShell (pwsh when installed).
#[tauri::command]
async fn spawn_custom(
    app: AppHandle,
    command: String,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    if command.trim().is_empty() {
        return Err("empty command".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let launch = shell::custom_command(&command, &path, &pathext);
        app.state::<PtyManager>().spawn(SpawnRequest { id: None, launch, args: vec![], cwd, env: vec![], cols, rows })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Installed WSL distributions (empty when WSL is missing).
#[tauri::command]
async fn list_wsl_distros() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(shell::list_distros).await.map_err(|e| e.to_string())
}

/// Start `wsl.exe -d <distro> --cd <wsl path of cwd>`: a login shell, or
/// `command` run by one.
#[tauri::command]
async fn spawn_wsl(
    app: AppHandle,
    distro: String,
    command: Option<String>,
    cwd: String,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let launch = shell::wsl_launch(&distro, command.as_deref(), &cwd);
        app.state::<PtyManager>().spawn(SpawnRequest {
            id: None,
            launch,
            args: vec![],
            cwd: PathBuf::from(&cwd),
            env: vec![],
            cols,
            rows,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `git worktree add` for one agent under `<project>/.spectacli/worktrees`.
#[tauri::command]
async fn create_worktree(project: PathBuf, name: String) -> Result<worktree::Worktree, String> {
    tauri::async_runtime::spawn_blocking(move || worktree::create(&project, &name)).await.map_err(|e| e.to_string())?
}

/// How many files a worktree has changed (0: safe to remove).
#[tauri::command]
async fn worktree_changes(path: PathBuf) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || worktree::changes(&path)).await.map_err(|e| e.to_string())?
}

/// Remove an agent's worktree; an error starting with `dirty:` means it has
/// uncommitted changes and needs `force`. The branch is kept.
#[tauri::command]
async fn remove_worktree(path: PathBuf, force: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || worktree::remove(&path, force)).await.map_err(|e| e.to_string())?
}

/// Show a folder in Explorer.
#[tauri::command]
async fn open_folder(path: PathBuf) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} does not exist", path.display()));
    }
    std::process::Command::new("explorer.exe").arg(&path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Route the agent's live output to `on_output` (raw bytes, batched) and
/// return the scrollback to write first. Replaces any previous pane.
/// `attach_id` is chosen by the pane and passed back to `detach_agent`.
#[tauri::command]
async fn attach_agent(
    pty: State<'_, PtyManager>,
    agent_id: String,
    attach_id: u64,
    on_output: Channel<InvokeResponseBody>,
) -> Result<Response, String> {
    let scrollback = pty.attach(&agent_id, attach_id, Box::new(move |d| on_output.send(InvokeResponseBody::Raw(d)).is_ok()))?;
    Ok(Response::new(scrollback))
}

/// Stop sending output (pane gone). The agent keeps running and buffering.
/// Ignored if another pane has attached since `attach_id`.
#[tauri::command]
async fn detach_agent(pty: State<'_, PtyManager>, agent_id: String, attach_id: u64) -> Result<(), String> {
    pty.detach(&agent_id, attach_id)
}

#[tauri::command]
async fn write(
    pty: State<'_, PtyManager>,
    status: State<'_, StatusHub>,
    agent_id: String,
    data: String,
) -> Result<(), String> {
    status.tracker.user_input(&agent_id, data.as_bytes(), Instant::now());
    pty.write(&agent_id, data.as_bytes())
}

#[tauri::command]
async fn resize(pty: State<'_, PtyManager>, agent_id: String, cols: u16, rows: u16) -> Result<(), String> {
    pty.resize(&agent_id, cols, rows)
}

#[tauri::command]
async fn kill_agent(pty: State<'_, PtyManager>, agent_id: String) -> Result<(), String> {
    pty.kill(&agent_id)
}

/// Gap between exit keys, and how long the CLI then gets to quit by itself.
const EXIT_KEY_GAP: Duration = Duration::from_millis(300);
const EXIT_GRACE: Duration = Duration::from_secs(3);

/// Stop an agent so its session can be resumed later: type the registry's
/// exit keys, wait briefly, then end the whole tree. Resolves once it is gone.
#[tauri::command]
async fn hibernate_agent(app: AppHandle, agent_id: String, agent_def_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let keys = lock_detected(&app).iter().find(|a| a.def.id == agent_def_id).map(|a| a.def.exit_keys.clone());
        app.state::<PtyManager>().stop_gracefully(&agent_id, &keys.unwrap_or_default(), EXIT_KEY_GAP, EXIT_GRACE)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Serializes saves so two writes never race on the temp file.
#[derive(Default)]
struct StoreLock(Mutex<()>);

fn projects_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("projects.json"))
}

/// The saved projects file (migrated to the current version), or null when
/// there is none yet. `notice` is set when the file had to be recovered.
#[tauri::command]
async fn load_projects(app: AppHandle, lock: State<'_, StoreLock>) -> Result<Option<store::Loaded>, String> {
    let path = projects_path(&app)?;
    let _g = lock.0.lock().unwrap_or_else(|e| e.into_inner());
    store::load(&path)
}

#[tauri::command]
async fn save_projects(app: AppHandle, lock: State<'_, StoreLock>, file: serde_json::Value) -> Result<(), String> {
    let path = projects_path(&app)?;
    let _g = lock.0.lock().unwrap_or_else(|e| e.into_inner());
    store::save(&path, &file)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentExit {
    agent_id: String,
    code: Option<u32>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Detected::default())
        .manage(StoreLock::default())
        .setup(|app| {
            if let Ok(dir) = app.path().app_config_dir() {
                panic_log::install(dir.join("logs"));
            }
            let handle = app.handle().clone();
            let hub = StatusHub::start(Arc::new(move |change| {
                let _ = handle.emit("agent:status", change);
            }));
            let tracker = hub.tracker.clone();
            let observed = hub.tracker.clone();
            app.manage(hub);
            let handle = app.handle().clone();
            let pty = PtyManager::new(Arc::new(move |id, code| {
                tracker.exited(id);
                let _ = handle.emit("agent:exit", AgentExit { agent_id: id.into(), code });
            }))
            .with_observer(Arc::new(move |id, data| observed.output(id, data, Instant::now())));
            app.manage(pty);
            stats::spawn(app.handle().clone());
            // Detect in the background at startup so the cache is warm before
            // the picker opens; the UI never waits on this.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let agents = detect_and_store(&handle);
                let _ = handle.emit("registry:changed", agents);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            detect_agents,
            spawn_agent,
            spawn_custom,
            list_wsl_distros,
            spawn_wsl,
            create_worktree,
            remove_worktree,
            worktree_changes,
            open_folder,
            attach_agent,
            detach_agent,
            write,
            resize,
            kill_agent,
            hibernate_agent,
            load_projects,
            save_projects
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn run(results: Vec<Result<&str, &str>>) -> (Result<String, RetryError>, usize, Vec<Duration>) {
        let calls = RefCell::new(0);
        let slept = RefCell::new(vec![]);
        let r = retry_mid_update(
            |n| {
                *calls.borrow_mut() += 1;
                results[n].map(String::from).map_err(String::from)
            },
            |d| slept.borrow_mut().push(d),
        );
        (r, calls.into_inner(), slept.into_inner())
    }

    const BAD_EXE: &str = "spawn claude.exe: CreateProcessW failed: not compatible (os error 216)";

    #[test]
    fn retries_while_exe_is_replaced_then_succeeds() {
        let (r, calls, slept) = run(vec![Err(BAD_EXE), Err("agent `claude` is not installed"), Ok("pty-1")]);
        assert_eq!(r.unwrap(), "pty-1");
        assert_eq!(calls, 3);
        assert_eq!(slept, UPDATE_RETRY_DELAYS[..2]);
    }

    #[test]
    fn other_errors_fail_at_once() {
        let (r, calls, slept) = run(vec![Err("agent `x` is not installed")]);
        let e = r.unwrap_err();
        assert!(!e.updating);
        assert_eq!(e.detail, "agent `x` is not installed");
        assert_eq!((calls, slept.len()), (1, 0));
    }

    #[test]
    fn gives_up_after_all_delays() {
        let (r, calls, slept) = run(vec![Err(BAD_EXE); UPDATE_RETRY_DELAYS.len() + 1]);
        assert!(r.unwrap_err().updating);
        assert_eq!(calls, UPDATE_RETRY_DELAYS.len() + 1);
        assert_eq!(slept, UPDATE_RETRY_DELAYS);
    }
}
