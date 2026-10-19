// Minimal Tauri 2 shell. All scoring lives in agent-value-core; the shell only
// exposes core results to the display-only web UI. Nothing is scored yet.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[tauri::command]
fn core_version() -> String {
    agent_value_core::CORE_VERSION.to_string()
}

fn main() {
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_version])
        .run(tauri::generate_context!());
    if let Err(err) = result {
        eprintln!("agent-value failed to start: {err}");
        std::process::exit(1);
    }
}
