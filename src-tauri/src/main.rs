// Minimal Tauri 2 shell. All scoring lives in agent-value-core; the shell only
// exposes core results to the display-only web UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod demo_example6;
mod elicit_cmd;
mod store_cmd;
mod window_chrome;

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
fn delete_all(
    state: tauri::State<store_cmd::AppState>,
    elicit: tauri::State<elicit_cmd::ElicitState>,
) -> Result<(), String> {
    elicit_cmd::wipe_with(&state, &elicit)
}

#[tauri::command]
fn elicit_start(
    elicit: tauri::State<elicit_cmd::ElicitState>,
) -> Result<elicit_cmd::StepView, String> {
    elicit_cmd::start(&elicit)
}

#[tauri::command]
fn elicit_next(
    elicit: tauri::State<elicit_cmd::ElicitState>,
) -> Result<elicit_cmd::StepView, String> {
    elicit_cmd::next_step(&elicit)
}

#[tauri::command]
fn elicit_answer(
    elicit: tauri::State<elicit_cmd::ElicitState>,
    answer: elicit_cmd::AnswerInput,
) -> Result<elicit_cmd::StepView, String> {
    elicit_cmd::answer(&elicit, answer)
}

#[tauri::command]
fn elicit_skip(
    elicit: tauri::State<elicit_cmd::ElicitState>,
    question: usize,
) -> Result<elicit_cmd::StepView, String> {
    elicit_cmd::skip(&elicit, question)
}

#[tauri::command]
fn elicit_regions(
    elicit: tauri::State<elicit_cmd::ElicitState>,
) -> Result<elicit_cmd::RegionsView, String> {
    elicit_cmd::regions_of(&elicit)
}

#[tauri::command]
fn elicit_finish(
    elicit: tauri::State<elicit_cmd::ElicitState>,
) -> Result<elicit_cmd::ReportView, String> {
    elicit_cmd::finish(&elicit)
}

#[tauri::command]
fn elicit_save(
    state: tauri::State<store_cmd::AppState>,
    elicit: tauri::State<elicit_cmd::ElicitState>,
) -> Result<elicit_cmd::SaveView, String> {
    elicit_cmd::save(&state, &elicit)
}

#[tauri::command]
fn weight_context(
    state: tauri::State<store_cmd::AppState>,
) -> Result<elicit_cmd::WeightContext, String> {
    elicit_cmd::context(&state)
}

#[tauri::command]
fn begin_window(
    state: tauri::State<store_cmd::AppState>,
) -> Result<elicit_cmd::WeightContext, String> {
    elicit_cmd::begin_window(&state)
}

#[tauri::command]
fn edit_weights(
    state: tauri::State<store_cmd::AppState>,
    weights: elicit_cmd::WeightInput,
) -> Result<elicit_cmd::TrialView, String> {
    elicit_cmd::edit_weights(&state, weights)
}

/// Sample first or later window. The label is the core's, when one applies.
#[tauri::command]
fn window_chrome(sample: String) -> Result<window_chrome::WindowChrome, String> {
    window_chrome::sample_window_chrome(&sample)
}

fn main() {
    let result = tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let state = store_cmd::AppState::open(&dir).map_err(std::io::Error::other)?;
            app.manage(state);
            app.manage(elicit_cmd::ElicitState::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_version,
            demo_example6_layer,
            list_arms,
            upsert_arm,
            store_data_path,
            export_store,
            delete_all,
            window_chrome,
            elicit_start,
            elicit_next,
            elicit_answer,
            elicit_skip,
            elicit_regions,
            elicit_finish,
            elicit_save,
            weight_context,
            begin_window,
            edit_weights
        ])
        .run(tauri::generate_context!());
    if let Err(err) = result {
        eprintln!("agent-value failed to start: {err}");
        std::process::exit(1);
    }
}
