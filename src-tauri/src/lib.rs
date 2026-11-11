// Blueprint Processor - Tauri backend.
//
// Responsibilities:
//   * Persist user-authored blueprints as JSON files on disk.
//   * Validate the JSON against the Blueprint struct on save/load.
//   * Run nodes (executor) and return the results.
//   * Track an inventory of saved blueprints for the Dashboard.

mod ai;
mod blueprint;
mod executor;
mod inventory;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use ai::{AiGateway, RigGateway, SecretStatus, SecretStore};
use blueprint::{Blueprint, SCHEMA_VERSION};
use executor::{run_graph_with, ExecError, RunOutcome};
use inventory::InventoryEntry;
use serde::Serialize;
use serde_json::Value;
use tauri::Manager;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BlueprintError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported schema version: found {found}, expected <= {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("Path is empty")]
    EmptyPath,
}

// Serialize errors as a plain string so they can cross the Tauri bridge
// without needing a custom serializable error type on the TS side.
impl Serialize for BlueprintError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Serialize)]
pub struct SaveResult {
    pub path: String,
    pub bytes: usize,
    /// Inventory entry that was upserted as a side-effect of saving.
    /// Lets the frontend shortcut a follow-up `inventory_add` call.
    pub inventory: Option<InventoryEntry>,
}

/// Save a blueprint and auto-upsert the (path, name) pair into the
/// inventory so the Dashboard picks it up on the next read. The
/// `inventory` field on the response carries the upserted entry, so
/// the frontend doesn't have to round-trip a second command.
///
/// **Inventory side-effects are best-effort.** A failure to update the
/// inventory does NOT fail the save — the file is the source of truth
/// and the user can always re-add manually from the Dashboard.
#[tauri::command]
fn save_blueprint(
    app: tauri::AppHandle,
    json: String,
    path: String,
) -> Result<SaveResult, BlueprintError> {
    if path.trim().is_empty() {
        return Err(BlueprintError::EmptyPath);
    }

    // Validate: parse into Blueprint (also catches malformed JSON).
    let mut bp: Blueprint = serde_json::from_str(&json)?;

    // Migrate legacy saves without a version stamp by raising to the
    // current SCHEMA_VERSION in-memory; the on-disk payload is then
    // re-serialised so v0 → v1 happens transparently.
    if bp.schema_version == 0 {
        bp.schema_version = SCHEMA_VERSION;
    }
    if bp.schema_version > SCHEMA_VERSION {
        return Err(BlueprintError::UnsupportedVersion {
            found: bp.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    // Pretty-print on save so the files are diff-friendly in git.
    let pretty = serde_json::to_string_pretty(&bp)?;
    write_atomically(Path::new(&path), pretty.as_bytes())?;

    // Compute the inventory upsert BEFORE moving `path` into the
    // SaveResult struct, so the inventory helper can still borrow it.
    let inventory = best_effort_inventory_add(&app, &path, &bp.name);

    Ok(SaveResult {
        path,
        bytes: pretty.len(),
        inventory,
    })
}

#[tauri::command]
fn load_blueprint(path: String) -> Result<String, BlueprintError> {
    if path.trim().is_empty() {
        return Err(BlueprintError::EmptyPath);
    }
    let raw = fs::read_to_string(&path)?;
    let bp: Blueprint = serde_json::from_str(&raw)?;

    if bp.schema_version > SCHEMA_VERSION {
        return Err(BlueprintError::UnsupportedVersion {
            found: bp.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    // Normalize on the way out so the frontend always sees current schema.
    serde_json::to_string(&bp).map_err(BlueprintError::from)
}

/// Run a blueprint. `inputs` provides runtime values for any
/// `blueprint.input` node whose `metadata.inputs[i].id` matches a
/// key. Required inputs without a `default` and no provided runtime
/// value collapse downstream to `Value::Null`. The fully-qualified
/// Blueprint JSON is the same shape we accept on save.
///
/// **Async on purpose:** `ai.*` nodes await provider calls (AI-BACK-004),
/// and the graph order is still fully deterministic — the topological
/// order is computed up front and nodes are awaited one at a time, so a
/// slow completion delays later nodes instead of racing them.
///
/// AI credentials are resolved here, from the app's secret store, and
/// handed to the runner as a gateway. They never travel in the JSON.
#[tauri::command]
async fn run_blueprint(
    app: tauri::AppHandle,
    json: String,
    inputs: HashMap<String, Value>,
) -> Result<RunOutcome, ExecError> {
    let bp: Blueprint = serde_json::from_str(&json).map_err(|e| ExecError::Node {
        node: "_root".into(),
        message: format!("invalid blueprint JSON: {e}"),
    })?;
    let gateway: Arc<dyn AiGateway> = Arc::new(RigGateway::new(secret_store(&app)?));
    Ok(RunOutcome::from_run_result(
        run_graph_with(&bp, &inputs, gateway).await,
    ))
}

// ---------------------------------------------------------------------
// AI credential commands — back the toolbar's "AI Keys" dialog.
//
// None of these return a key. `ai_key_status` answers `configured` plus a
// last-4 hint, which is all the UI needs (AI-SEC-002).
// ---------------------------------------------------------------------

#[tauri::command]
fn ai_key_status(app: tauri::AppHandle) -> Result<Vec<SecretStatus>, String> {
    secret_store(&app)
        .map_err(|e| e.to_string())?
        .status()
}

#[tauri::command]
fn ai_set_key(
    app: tauri::AppHandle,
    provider: String,
    key: String,
) -> Result<Vec<SecretStatus>, String> {
    let store = secret_store(&app).map_err(|e| e.to_string())?;
    store.set(&provider, &key)?;
    store.status()
}

#[tauri::command]
fn ai_clear_key(app: tauri::AppHandle, provider: String) -> Result<Vec<SecretStatus>, String> {
    let store = secret_store(&app).map_err(|e| e.to_string())?;
    store.clear(&provider)?;
    store.status()
}

// ---------------------------------------------------------------------
// Inventory commands — back the Dashboard route at `/dashboard`.
// ---------------------------------------------------------------------

/// List inventory entries, filtering out paths that are missing from
/// disk. This is what the Dashboard renders on mount (and after every
/// user-triggered refresh). Missing entries are NOT removed from the
/// inventory file, so a temporarily-unmounted drive comes back online
/// without losing the user's list.
#[tauri::command]
fn inventory_list(app: tauri::AppHandle) -> Result<Vec<InventoryEntry>, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app_data_dir: {e}"))?;
    inventory::list_with_check(&dir).map_err(|e| e.to_string())
}

/// Manual upsert the dashboard uses when adding a pre-existing file
/// to the inventory (without going through the editor's Save flow).
#[tauri::command]
fn inventory_add(
    app: tauri::AppHandle,
    path: String,
    name: String,
) -> Result<InventoryEntry, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app_data_dir: {e}"))?;
    inventory::add(&dir, &path, &name).map_err(|e| e.to_string())
}

/// Drop a path from the inventory without touching the file on disk.
/// Used by the Dashboard's "Remove from library" button.
#[tauri::command]
fn inventory_remove(app: tauri::AppHandle, path: String) -> Result<bool, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app_data_dir: {e}"))?;
    inventory::remove(&dir, &path).map_err(|e| e.to_string())
}

/// Return the absolute path of the inventory file. Useful for tests
/// and debugging; also helps the frontend render the data dir path
/// in the Dashboard header.
#[tauri::command]
fn inventory_path(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app_data_dir: {e}"))?;
    Ok(inventory::inventory_path(&dir).to_string_lossy().to_string())
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// The app-data-dir backed credential store. Resolved lazily per command
/// so tests never need an `AppHandle` and the path always matches the
/// platform's app data directory.
fn secret_store(app: &tauri::AppHandle) -> Result<SecretStore, ExecError> {
    let dir = app.path().app_data_dir().map_err(|e| ExecError::Node {
        node: "_root".into(),
        message: format!("resolve app_data_dir: {e}"),
    })?;
    Ok(SecretStore::new(dir))
}

/// Upsert (path, name) into the inventory, tolerating any failure
/// (returns `None`). Used as a best-effort side-effect of `save_blueprint`.
/// Logs the failure so devtools surfaces 'why doesn't my dashboard show
/// this blueprint?' without the user having to guess.
fn best_effort_inventory_add(
    app: &tauri::AppHandle,
    path: &str,
    name: &str,
) -> Option<InventoryEntry> {
    let dir = match app.path().app_data_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("save_blueprint: inventory unavailable (app_data_dir): {e}");
            return None;
        }
    };
    match inventory::add(&dir, path, name) {
        Ok(e) => Some(e),
        Err(e) => {
            eprintln!("save_blueprint: inventory upsert for {path:?} failed: {e}");
            None
        }
    }
}

/// Write to a temp file next to the target and then atomically rename it.
/// Protects against corrupted files if the app crashes mid-write and avoids
/// collisions when several saves race into the same directory (the temp
/// suffix encodes the PID and nanoseconds to keep it unique across saves).
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let base = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("blueprint");
    let pid = process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(".{base}.{pid}.{nanos}.tmp"));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            save_blueprint,
            load_blueprint,
            run_blueprint,
            ai_key_status,
            ai_set_key,
            ai_clear_key,
            inventory_list,
            inventory_add,
            inventory_remove,
            inventory_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
