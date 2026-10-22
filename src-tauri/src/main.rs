// Minimal Tauri 2 shell. All scoring lives in agent-value-core; the shell only
// exposes core results to the display-only web UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod demo_example6;
mod store_cmd;

use tauri::Manager;

#[tauri::command]
fn core_version() -> String {
    agent_value_core::CORE_VERSION.to_string()
}

/// Example 6 diagnostic anchor `A_C`, computed by agent-value-core and
/// printed the same way the core tests print it.
#[tauri::command]
fn demo_example6_layer() -> Result<String, String> {
    demo_example6::example6_diagnostic_a_c().map(demo_example6::print_a_c)
}

#[tauri::command]
fn list_arms(state: tauri::State<store_cmd::AppState>) -> Result<Vec<store_cmd::ArmRow>, String> {
    store_cmd::read_arms(&state)
}

#[tauri::command]
fn upsert_arm(
    state: tauri::State<store_cmd::AppState>,
    input: store_cmd::ArmInput,
) -> Result<store_cmd::ArmRow, String> {
    store_cmd::save_arm(&state, input)
}

#[tauri::command]
fn store_data_path(state: tauri::State<store_cmd::AppState>) -> String {
    store_cmd::data_path(&state)
}

#[tauri::command]
fn export_store(
    state: tauri::State<store_cmd::AppState>,
) -> Result<store_cmd::ExportReport, String> {
    store_cmd::export_store(&state)
}

#[tauri::command]
fn delete_all(state: tauri::State<store_cmd::AppState>) -> Result<(), String> {
    store_cmd::delete_all(&state)
}

fn main() {
    let result = tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let state = store_cmd::AppState::open(&dir).map_err(std::io::Error::other)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_version,
            demo_example6_layer,
            list_arms,
            upsert_arm,
            store_data_path,
            export_store,
            delete_all
        ])
        .run(tauri::generate_context!());
    if let Err(err) = result {
        eprintln!("agent-value failed to start: {err}");
        std::process::exit(1);
    }
}
