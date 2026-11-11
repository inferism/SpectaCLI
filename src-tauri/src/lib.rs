mod registry;

use registry::DetectedAgent;
use tauri::{Emitter, Manager};

/// Installed agents from the registry. Runs off the main thread; repeat calls
/// are fast because probe results are cached.
#[tauri::command]
async fn detect_agents(app: tauri::AppHandle) -> Result<Vec<DetectedAgent>, String> {
    let dir = app.path().app_config_dir().ok();
    tauri::async_runtime::spawn_blocking(move || registry::detect_agents(dir.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Detect in the background at startup so the cache is warm before
            // the picker opens; the UI never waits on this.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let dir = handle.path().app_config_dir().ok();
                let agents = registry::detect_agents(dir.as_deref());
                let _ = handle.emit("registry:changed", agents);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![detect_agents])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
