//! kansa desktop app — one Tauri command that dispatches into `kansa_core::api::call`
//! (`ui~core-parity~1`): the same table the `kansa serve` dev bridge uses.

use serde_json::Value;

/// Run a core command off the UI thread (git and file IO must never block rendering).
#[tauri::command]
async fn call(name: String, args: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        kansa_core::api::call(&name, &args).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![call])
        .run(tauri::generate_context!())
        .expect("error while running kansa");
}
