mod pty;
mod registry;
mod store;

use pty::{PtyManager, SpawnRequest};
use registry::{DetectedAgent, LaunchKind, LaunchSpec};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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

/// Start a detected agent in `cwd`. Returns its id; attach a pane with
/// `attach_agent`. (Projects and profiles hook in here in later tasks.)
#[tauri::command]
async fn spawn_agent(
    app: AppHandle,
    agent_def_id: String,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let find = |agents: &[DetectedAgent]| agents.iter().find(|a| a.def.id == agent_def_id).map(|a| a.launch.clone());
        let cached = find(&app.state::<Detected>().0.lock().unwrap_or_else(|e| e.into_inner()));
        let launch = cached
            .or_else(|| find(&detect_and_store(&app)))
            .ok_or_else(|| format!("agent `{agent_def_id}` is not installed"))?;
        app.state::<PtyManager>().spawn(SpawnRequest { launch, args: vec![], cwd, env: vec![], cols, rows })
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
#[tauri::command]
async fn attach_agent(
    pty: State<'_, PtyManager>,
    agent_id: String,
    on_output: Channel<InvokeResponseBody>,
) -> Result<Response, String> {
    let scrollback = pty.attach(&agent_id, Box::new(move |d| on_output.send(InvokeResponseBody::Raw(d)).is_ok()))?;
    Ok(Response::new(scrollback))
}

/// Stop sending output (pane hidden). The agent keeps running and buffering.
#[tauri::command]
async fn detach_agent(pty: State<'_, PtyManager>, agent_id: String) -> Result<(), String> {
    pty.detach(&agent_id)
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
