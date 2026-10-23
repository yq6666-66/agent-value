//! Local store held by the shell. Scoring stays in agent-value-core; this
//! module only opens the database and passes arm, export, and wipe calls
//! through to agent-value-store.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_value_core::arm::{Arm, ModelId, Protocol, ProviderKind, Route};
use agent_value_core::scope::ScopeId;
use agent_value_core::weights::WeightLedger;
use agent_value_probe_run::Flight;
use agent_value_store::{DigestKey, ExportDump, Store};
use serde::Deserialize;
use serde::Serialize;

const DB_FILE: &str = "agent-value.sqlite";
const KEY_FILE: &str = "digest.key";
const EXPORT_FILE: &str = "agent-value-export.json";

/// One arm as stored. These are the only fields the webview receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArmRow {
    pub id: i64,
    pub model: String,
    pub provider_kind: String,
    pub host: String,
    pub protocol: String,
    pub client: String,
}

/// Fields the webview may send when adding an arm. There is no key field.
#[derive(Debug, Deserialize)]
pub struct ArmInput {
    pub model: String,
    pub provider_kind: String,
    pub host: String,
    pub protocol: String,
    pub client: String,
}

/// Result of an export. The webview gets counts and the file path, not row bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    pub schema_version: i64,
    pub table_count: usize,
    pub row_count: usize,
    pub path: Option<String>,
}

pub struct AppState {
    store: Mutex<Store>,
    data_path: PathBuf,
    pub(crate) flight: Flight,
}

impl AppState {
    /// Opens (creating if needed) the database and digest key in `dir`.
    pub fn open(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        let data_path = dir.join(DB_FILE);
        let key = load_or_create_digest_key(&dir.join(KEY_FILE))?;
        let store = Store::open(&data_path, key).map_err(|err| err.to_string())?;
        Ok(Self {
            store: Mutex::new(store),
            data_path,
            flight: Flight::new(),
        })
    }

    /// In-memory database for tests. Nothing is written to disk.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, String> {
        let key = DigestKey::from_bytes(&[7u8; 32]).map_err(|err| err.to_string())?;
        let store = Store::open_in_memory(key).map_err(|err| err.to_string())?;
        Ok(Self {
            store: Mutex::new(store),
            data_path: PathBuf::from(":memory:"),
            flight: Flight::new(),
        })
    }

    /// Store mutex for [`crate::probe_run::run_once`]. The panel does not
    /// send, so a build that never calls the run may not use this.
    #[allow(dead_code)]
    pub(crate) fn store(&self) -> &Mutex<Store> {
        &self.store
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        self.store
            .lock()
            .map_err(|_| "store lock poisoned".to_string())
    }
}

pub fn data_path(state: &AppState) -> String {
    state.data_path.display().to_string()
}

pub fn read_arms(state: &AppState) -> Result<Vec<ArmRow>, String> {
    let store = state.lock()?;
    let arms = store.list_arms().map_err(|err| err.to_string())?;
    Ok(arms.iter().map(|(id, arm)| arm_row(id.0, arm)).collect())
}

pub fn save_arm(state: &AppState, input: ArmInput) -> Result<ArmRow, String> {
    let arm = arm_from_input(&input)?;
    let store = state.lock()?;
    let id = store.upsert_arm(&arm).map_err(|err| err.to_string())?;
    let stored = store.arm(id).map_err(|err| err.to_string())?;
    Ok(arm_row(id.0, &stored))
}

pub fn export_store(state: &AppState) -> Result<ExportReport, String> {
    let store = state.lock()?;
    let dump = store.export().map_err(|err| err.to_string())?;
    let report = report_for(&dump, None);
    if state.data_path == Path::new(":memory:") {
        return Ok(report);
    }
    let path = state.data_path.with_file_name(EXPORT_FILE);
    let body = serde_json::to_string_pretty(&dump_json(&dump)).map_err(|err| err.to_string())?;
    fs::write(&path, body).map_err(|err| err.to_string())?;
    Ok(report_for(&dump, Some(path.display().to_string())))
}

pub fn delete_all(state: &AppState) -> Result<(), String> {
    let mut store = state.lock()?;
    store.delete_all().map_err(|err| err.to_string())
}

pub(crate) fn with_store<T>(
    state: &AppState,
    body: impl FnOnce(&agent_value_store::Store) -> Result<T, String>,
) -> Result<T, String> {
    let store = state.lock()?;
    body(&store)
}

pub(crate) fn with_store_mut<T>(
    state: &AppState,
    body: impl FnOnce(&mut agent_value_store::Store) -> Result<T, String>,
) -> Result<T, String> {
    let mut store = state.lock()?;
    body(&mut store)
}

/// The stored ledger for `scope`, or a new default ledger when none was saved.
pub fn read_weight_ledger(state: &AppState, scope: &ScopeId) -> Result<WeightLedger, String> {
    let store = state.lock()?;
    match store
        .load_weight_ledger(scope)
        .map_err(|err| err.to_string())?
    {
        Some(ledger) => Ok(ledger),
        None => Ok(WeightLedger::new(scope.clone())),
    }
}

/// Loads the scope's ledger, applies `update`, and saves it.
///
/// A new scope has no rows yet; the default version is written with the update.
/// If `update` returns an error, nothing is written.
pub fn update_weight_ledger<T>(
    state: &AppState,
    scope: &ScopeId,
    update: impl FnOnce(&mut WeightLedger) -> Result<T, String>,
) -> Result<T, String> {
    let mut store = state.lock()?;
    let mut ledger = match store
        .load_weight_ledger(scope)
        .map_err(|err| err.to_string())?
    {
        Some(ledger) => ledger,
        None => WeightLedger::new(scope.clone()),
    };
    let value = update(&mut ledger)?;
    store
        .save_weight_ledger(&ledger)
        .map_err(|err| err.to_string())?;
    Ok(value)
}

fn report_for(dump: &ExportDump, path: Option<String>) -> ExportReport {
    ExportReport {
        schema_version: dump.schema_version,
        table_count: dump.tables.len(),
        row_count: dump.tables.iter().map(|table| table.rows.len()).sum(),
        path,
    }
}

fn dump_json(dump: &ExportDump) -> serde_json::Value {
    serde_json::json!({
        "schema_version": dump.schema_version,
        "tables": dump.tables.iter().map(|table| {
            serde_json::json!({
                "name": table.name,
                "columns": table.columns,
                "rows": table.rows,
            })
        }).collect::<Vec<_>>(),
    })
}

fn arm_row(id: i64, arm: &Arm) -> ArmRow {
    let route = arm.route();
    ArmRow {
        id,
        model: arm.model().as_str().to_string(),
        provider_kind: route.kind().as_str().to_string(),
        host: route.host().to_string(),
        protocol: route.protocol().as_str().to_string(),
        client: route.client().to_string(),
    }
}

fn arm_from_input(input: &ArmInput) -> Result<Arm, String> {
    let model = ModelId::new(&input.model).map_err(|err| err.to_string())?;
    let kind = ProviderKind::parse(&input.provider_kind).map_err(|err| err.to_string())?;
    let protocol = Protocol::parse(&input.protocol).map_err(|err| err.to_string())?;
    let route =
        Route::new(kind, &input.host, protocol, &input.client).map_err(|err| err.to_string())?;
    Ok(Arm::new(model, route))
}

fn load_or_create_digest_key(path: &Path) -> Result<DigestKey, String> {
    if path.exists() {
        let bytes = fs::read(path).map_err(|err| err.to_string())?;
        return DigestKey::from_bytes(&bytes).map_err(|err| err.to_string());
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|err| err.to_string())?;
    write_new_file(path, &bytes)?;
    DigestKey::from_bytes(&bytes).map_err(|err| err.to_string())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| err.to_string())?;
    file.write_all(bytes).map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use agent_value_core::cells::CellId;
    use agent_value_core::records::TaskRecord;
    use agent_value_core::scope::{ScopeId, TaskKind};

    use super::*;

    const PROMPT_MARKER: &str = "QX7-PROMPT-BODY";

    fn input(model: &str) -> ArmInput {
        ArmInput {
            model: model.to_string(),
            provider_kind: "official".to_string(),
            host: "api.example.test".to_string(),
            protocol: "anthropic-messages".to_string(),
            client: "desktop".to_string(),
        }
    }

    #[test]
    fn upsert_then_list_shows_the_arm_without_a_secret_column() {
        let state = AppState::open_in_memory().unwrap();
        assert!(read_arms(&state).unwrap().is_empty());
        let saved = save_arm(&state, input("Example-Model")).unwrap();
        assert_eq!(saved.model, "example-model");
        let listed = read_arms(&state).unwrap();
        assert_eq!(listed, vec![saved.clone()]);

        let value = serde_json::to_value(&saved).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["client", "host", "id", "model", "protocol", "provider_kind",]
        );
    }

    #[test]
    fn delete_all_clears_the_arm_list() {
        let state = AppState::open_in_memory().unwrap();
        save_arm(&state, input("example-model")).unwrap();
        delete_all(&state).unwrap();
        assert!(read_arms(&state).unwrap().is_empty());
    }

    #[test]
    fn export_file_excludes_prompt_text() {
        let dir =
            std::env::temp_dir().join(format!("agent-value-shell-export-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let state = AppState::open(&dir).unwrap();
        let prompt = format!("Please refactor the module; secret marker {PROMPT_MARKER}");
        {
            let store = state.lock().unwrap();
            let mut task = TaskRecord::new(
                "t1",
                ScopeId::new(TaskKind::Coding),
                CellId::user("refactor").unwrap(),
                arm_from_input(&input("example-model")).unwrap(),
                1_700_000_000_000,
            );
            task.first_message = Some(store.text_facts("first-message", &prompt).unwrap());
            store.insert_task(&task).unwrap();
        }

        let report = export_store(&state).unwrap();
        let path = report.path.expect("export path");
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(PROMPT_MARKER));
        assert!(!text.contains(&prompt));
        assert!(text.contains("first_msg_digest"));
        assert!(text.contains("example-model"));

        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        for table in json["tables"].as_array().unwrap() {
            for column in table["columns"].as_array().unwrap() {
                let name = column.as_str().unwrap().to_lowercase();
                let allowed = name == "key_env_var" || name == "content_sha256";
                let forbidden = ["secret", "api_key", "password", "prompt", "body"]
                    .iter()
                    .any(|needle| name.contains(needle));
                assert!(!forbidden || allowed, "{name}");
            }
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
