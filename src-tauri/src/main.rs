// Minimal Tauri 2 shell. All scoring lives in agent-value-core; the shell only
// exposes core results to the display-only web UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod demo_example6;
mod elicit_cmd;
// Re-exports the sender. Orchestration calls the crate directly.
#[allow(dead_code)]
mod probe_http;
mod probe_panel;
mod probe_run;
mod sensitivity_panel;
mod store_cmd;
mod window_chrome;
mod window_result;

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

/// Coding-scope window result, or one named example fixture.
///
/// `source` is `store`, `example6`, `example7`, `example8`, or `tie`.
#[tauri::command]
fn window_result(
    state: tauri::State<store_cmd::AppState>,
    source: String,
) -> Result<window_result::WindowResult, String> {
    window_result::result(&state, &source)
}

/// Winner map, Ω copy, and flip sentences for one coding layer.
///
/// `source` is `store`, `example6`, `example7`, `example8`, `tie`, `first`, or `grid`.
/// The chart is omitted when the core withholds it.
#[tauri::command]
fn sensitivity(
    state: tauri::State<store_cmd::AppState>,
    source: String,
) -> Result<sensitivity_panel::SensitivityView, String> {
    sensitivity_panel::view(&state, &source)
}

/// Probe consent, budget, and recorded or fixture results.
///
/// `source` is `store` or `fixture`. This command does not send HTTP.
#[tauri::command]
fn probe_panel(
    state: tauri::State<store_cmd::AppState>,
    source: String,
) -> Result<probe_panel::ProbePanel, String> {
    probe_panel::view(&state, &source)
}

/// Stores the first probe-consent decision. A later call keeps that decision.
#[tauri::command]
fn set_probe_consent(
    state: tauri::State<store_cmd::AppState>,
    accepted: bool,
    source: String,
) -> Result<probe_panel::ProbePanel, String> {
    probe_panel::set_consent(&state, accepted, &source)
}

/// Runs one probe for an arm already stored or already shown.
///
/// The webview sends the arm id, the key's environment-variable name, and
/// the base URL. The seed is [`probe_run::IN_APP_RUN_SEED`] and the claims
/// are the H3 typical plan (tool call and JSON). The page does not choose
/// them, and it does not send a token. HTTP is blocking, so the call runs
/// on `spawn_blocking` and does not hold the async runtime. Opening the
/// page does not invoke this command.
#[tauri::command]
async fn run_probe_once(
    app: tauri::AppHandle,
    input: probe_run::RunOnceInput,
) -> Result<probe_run::RunOnceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<store_cmd::AppState>();
        probe_run::run_from_webview(state.inner(), &input)
    })
    .await
    .map_err(|_| "这次探测没有完成。".to_string())?
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
            edit_weights,
            window_result,
            sensitivity,
            probe_panel,
            set_probe_consent,
            run_probe_once
        ])
        .run(tauri::generate_context!());
    if let Err(err) = result {
        eprintln!("agent-value failed to start: {err}");
        std::process::exit(1);
    }
}
