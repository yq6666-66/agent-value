// Minimal Tauri 2 shell. All scoring lives in agent-value-core; the shell only
// exposes core results to the display-only web UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod demo_example6;

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

fn main() {
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_version, demo_example6_layer])
        .run(tauri::generate_context!());
    if let Err(err) = result {
        eprintln!("agent-value failed to start: {err}");
        std::process::exit(1);
    }
}
