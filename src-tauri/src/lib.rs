mod pty;
mod registry;
mod stats;
mod store;

use pty::{PtyManager, SpawnRequest};
use registry::{DetectedAgent, LaunchKind, LaunchSpec};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, Emitter, Manager, State};

/// Last detection result, so spawning doesn't re-walk PATH.
#[derive(Default)]
struct Detected(Mutex<Vec<DetectedAgent>>);

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

/// Start a detected agent in `cwd`. Returns its id; attach a pane with
/// `attach_agent`. Retries for a few seconds if the CLI is updating itself.
#[tauri::command]
async fn spawn_agent(
    app: AppHandle,
    agent_def_id: String,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let find = |agents: &[DetectedAgent]| agents.iter().find(|a| a.def.id == agent_def_id).cloned();
        let mut name = agent_def_id.clone();
        let result = retry_mid_update(
            |n| {
                // Retries detect again: an update can move or replace the exe.
                let cached = if n == 0 { find(&app.state::<Detected>().0.lock().unwrap_or_else(|e| e.into_inner())) } else { None };
                let agent = cached
                    .or_else(|| find(&detect_and_store(&app)))
                    .ok_or_else(|| format!("agent `{agent_def_id}` is not installed"))?;
                name = agent.def.name.clone();
                app.state::<PtyManager>().spawn(SpawnRequest {
                    launch: agent.launch,
                    args: vec![],
                    cwd: cwd.clone(),
                    env: vec![],
                    cols,
                    rows,
                })
            },
            std::thread::sleep,
        );
        result.map_err(|e| {
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

/// Start a user-typed command line in `cwd` through the shell.
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
        // With /s, cmd strips the outer quotes the pty layer adds around the line.
        let launch = LaunchSpec {
            kind: LaunchKind::Exe,
            program: PathBuf::from(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())),
            args: vec!["/d".into(), "/s".into(), "/c".into(), command],
        };
        app.state::<PtyManager>().spawn(SpawnRequest { launch, args: vec![], cwd, env: vec![], cols, rows })
    })
    .await
    .map_err(|e| e.to_string())?
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
async fn write(pty: State<'_, PtyManager>, agent_id: String, data: String) -> Result<(), String> {
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

/// Serializes saves so two writes never race on the temp file.
#[derive(Default)]
struct StoreLock(Mutex<()>);

fn projects_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("projects.json"))
}

/// The saved projects file, or null when there is none yet.
#[tauri::command]
async fn load_projects(app: AppHandle, lock: State<'_, StoreLock>) -> Result<Option<serde_json::Value>, String> {
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
        .manage(Detected::default())
        .manage(StoreLock::default())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(PtyManager::new(Arc::new(move |id, code| {
                let _ = handle.emit("agent:exit", AgentExit { agent_id: id.into(), code });
            })));
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
            attach_agent,
            detach_agent,
            write,
            resize,
            kill_agent,
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
