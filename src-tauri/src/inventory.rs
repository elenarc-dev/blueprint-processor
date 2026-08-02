//! Persistent inventory of saved blueprint files.
//!
//! Stored as a single JSON file at `${app_data_dir}/inventory.json`.
//! Each entry remembers the path the user saved to and a cached copy
//! of `name` so the dashboard can render a row without reading every
//! file. `last_seen_at` / `added_at` are unix-seconds stamps.
//!
//! Missing files are NOT removed on detect; `list_with_check` filters
//! them out at read time so a blueprint that was temporarily moved
//! comes back automatically when the drive is re-mounted. Use
//! `inventory::remove` to delete from the inventory permanently.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A single inventory entry. `path` is the absolute filesystem path
/// the user saved to. `name` is the cached `bp.name` so the dashboard
/// can render the row without re-parsing every file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InventoryEntry {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub added_at: u64,
    #[serde(default)]
    pub last_seen_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct InventoryFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    entries: Vec<InventoryEntry>,
}

fn default_version() -> u32 {
    INVENTORY_VERSION
}

const INVENTORY_VERSION: u32 = 1;
const INVENTORY_FILENAME: &str = "inventory.json";

#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid inventory JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Path is empty")]
    EmptyPath,
}

/// Resolve the inventory file path given an app-data root. Trivial but
/// centralised so tests and the Tauri command stay aligned.
pub fn inventory_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(INVENTORY_FILENAME)
}

/// Load the inventory from disk. Missing file returns an empty
/// inventory (first launch). A corrupt file is also treated as empty
/// to keep the app starting; the next `add` will overwrite with a
/// valid envelope and the prior content is dropped silently. We log
/// the parse error so the dev tools surface it.
pub fn load(app_data_dir: &Path) -> Result<Vec<InventoryEntry>, InventoryError> {
    let path = inventory_path(app_data_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)?;
    match serde_json::from_str::<InventoryFile>(&raw) {
        Ok(parsed) => Ok(parsed.entries),
        Err(e) => {
            // Don't bubble up corruption — the next add() rewrites the
            // file and we don't want a benign first-launch-with-typo'd-
            // file to brick the app. Log so devtools surfaces it and a
            // user opening the file by hand can see the parse error.
            eprintln!("inventory.json parse failed; recovering as empty: {e}");
            Ok(Vec::new())
        }
    }
}

/// Persist the inventory atomically: write to a temp sibling then
/// rename. Same protection as `write_atomically` in lib.rs.
pub fn save(app_data_dir: &Path, entries: &[InventoryEntry]) -> Result<(), InventoryError> {
    fs::create_dir_all(app_data_dir)?;
    let envelope = InventoryFile {
        version: INVENTORY_VERSION,
        entries: entries.to_vec(),
    };
    let pretty = serde_json::to_string_pretty(&envelope)?;
    let target = inventory_path(app_data_dir);
    atomic_write(&target, pretty.as_bytes());
    Ok(())
}

/// Add or update an entry by path. Idempotent on path: re-adding the
/// same path only updates `name` and `last_seen_at`. Returns the
/// resulting entry.
///
/// **Concurrent safety:** the entire `load → modify → save` cycle is
/// held under a process-wide mutex (`inv_lock`). Tauri `#[command]`
/// handlers run on a tokio thread pool, so multiple parallel
/// `inventory_add` invocations (e.g. the dashboard's bulk-add UX
/// with `Promise.allSettled`) would otherwise race on the file I/O
/// and lose entries. With the lock, every caller observes a
/// serialized read-modify-write window.
pub fn add(
    app_data_dir: &Path,
    path: &str,
    name: &str,
) -> Result<InventoryEntry, InventoryError> {
    if path.trim().is_empty() {
        return Err(InventoryError::EmptyPath);
    }
    let _guard = inv_lock()
        .lock()
        .expect("inventory mutex poisoned — unwrap is safe because the guard never panics");
    let mut entries = load(app_data_dir)?;
    let now = unix_seconds();
    let entry = match entries.iter_mut().find(|e| e.path == path) {
        Some(existing) => {
            existing.name = name.to_string();
            existing.last_seen_at = now;
            existing.clone()
        }
        None => {
            let entry = InventoryEntry {
                path: path.to_string(),
                name: name.to_string(),
                added_at: now,
                last_seen_at: now,
            };
            entries.push(entry.clone());
            entry
        }
    };
    save(app_data_dir, &entries)?;
    Ok(entry)
}

/// Remove an entry by path. Returns `true` if the entry existed and
/// was removed; `false` if it wasn't there in the first place.
///
/// Held under the same process-wide mutex as `add` so a concurrent
/// `remove` + `add` on a fresh drive can't combine a stale load
/// with a fresh save.
pub fn remove(app_data_dir: &Path, path: &str) -> Result<bool, InventoryError> {
    let _guard = inv_lock()
        .lock()
        .expect("inventory mutex poisoned — unwrap is safe because the guard never panics");
    let mut entries = load(app_data_dir)?;
    let before = entries.len();
    entries.retain(|e| e.path != path);
    if entries.len() == before {
        return Ok(false);
    }
    save(app_data_dir, &entries)?;
    Ok(true)
}

/// List inventory entries whose file still exists on disk. Missing
/// ones are filtered out at read time; `load` keeps them so a
/// temporarily-unmounted drive doesn't lose the user's list.
pub fn list_with_check(app_data_dir: &Path) -> Result<Vec<InventoryEntry>, InventoryError> {
    let entries = load(app_data_dir)?;
    Ok(entries
        .into_iter()
        .filter(|e| Path::new(&e.path).exists())
        .collect())
}

fn atomic_write(target: &Path, bytes: &[u8]) {
    use std::process;

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let _ = fs::create_dir_all(parent);

    let base = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("inventory");
    let pid = process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(".{base}.{pid}.{nanos}.tmp"));
    let _ = fs::write(&tmp, bytes);
    let _ = fs::rename(&tmp, target);
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Process-wide mutex serialising every read-modify-write of
/// `inventory.json`. Held across `inventory::add` / `inventory::remove`
/// so concurrent Tauri command handlers — including the dashboard's
/// parallel bulk-add — cannot lose entries to a load-merge-write race.
///
/// `OnceLock` + `Mutex` is the standard-library equivalent of the
/// `lazy_static!` / `once_cell::sync::Lazy` pattern; no extra crate
/// is required. The mutex is process-wide (one per app instance) —
/// any IPC caller or `inventory::*` unit test goes through this same
/// lock when it touches the file.
///
/// Reads (e.g. `inventory::list_with_check`) deliberately do NOT
/// take the lock: `atomic_write` writes to a temp file and renames,
/// so any reader sees EITHER the pre-write or the post-write version
/// — never a partial file.
fn inv_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::nodes::test_helpers::unique_tmp;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let pid = std::process::id();
            let mut p = std::env::temp_dir();
            p.push(format!("bp-inv-{label}-{pid}-{nanos}"));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn empty_load_on_missing_file() {
        let dir = TempDir::new("empty");
        assert!(load(&dir.0).unwrap().is_empty());
    }

    #[test]
    fn add_returns_new_entry() {
        let dir = TempDir::new("add-new");
        let e = add(&dir.0, "/tmp/example.bp.json", "Example").unwrap();
        assert_eq!(e.path, "/tmp/example.bp.json");
        assert_eq!(e.name, "Example");
        assert!(e.added_at > 0);
        assert_eq!(e.added_at, e.last_seen_at);
    }

    #[test]
    fn add_is_idempotent_by_path() {
        let dir = TempDir::new("idem");
        let first = add(&dir.0, "/tmp/x.bp.json", "First").unwrap();
        // Sleep a whole second so the unix-seconds stamp bumps in
        // every CI clock granularity (Windows has 100-ns tick so a
        // single ms is plenty, but Linux sometimes has coarser
        // resolution on containers).
        std::thread::sleep(std::time::Duration::from_secs(1));
        let second = add(&dir.0, "/tmp/x.bp.json", "Second").unwrap();
        assert_eq!(second.path, "/tmp/x.bp.json");
        assert_eq!(second.name, "Second");
        let entries = load(&dir.0).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Second");
        // CRITICAL invariants this test is named for:
        //   * `added_at` is preserved across re-adds (not reset to now).
        //   * `last_seen_at` strictly increases on re-add.
        assert_eq!(entries[0].added_at, first.added_at);
        assert!(entries[0].last_seen_at > entries[0].added_at);
        assert_eq!(entries[0].last_seen_at, second.last_seen_at);
    }

    #[test]
    fn remove_returns_true_for_known_and_false_for_unknown() {
        let dir = TempDir::new("rem");
        add(&dir.0, "/tmp/y.bp.json", "Y").unwrap();
        assert!(remove(&dir.0, "/tmp/y.bp.json").unwrap());
        assert!(!remove(&dir.0, "/tmp/nope.bp.json").unwrap());
    }

    #[test]
    fn list_with_check_filters_missing() {
        let dir = TempDir::new("check");
        let real = unique_tmp("real");
        fs::write(&real, b"{}").unwrap();
        let p_real = real.to_string_lossy().to_string();
        let p_ghost = "/tmp/__bp_does_not_exist__.bp.json";
        add(&dir.0, &p_real, "Real").unwrap();
        add(&dir.0, p_ghost, "Ghost").unwrap();
        let visible = list_with_check(&dir.0).unwrap();
        // Only the on-disk one remains.
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].path, p_real);
        // The ghost entry is still in the raw inventory so a
        // re-appearance is recovered automatically.
        let raw = load(&dir.0).unwrap();
        assert_eq!(raw.len(), 2);
        fs::remove_file(&real).ok();
    }

    #[test]
    fn empty_path_rejected() {
        let dir = TempDir::new("empty-path");
        let err = add(&dir.0, "   ", "x").unwrap_err();
        assert!(matches!(err, InventoryError::EmptyPath));
    }

    #[test]
    fn corrupt_file_treated_as_empty() {
        let dir = TempDir::new("corrupt");
        fs::create_dir_all(&dir.0).unwrap();
        let p = inventory_path(&dir.0);
        fs::write(&p, b"not json {{{").unwrap();
        // Loads as empty; subsequent add rewrites cleanly.
        assert!(load(&dir.0).unwrap().is_empty());
        add(&dir.0, "/tmp/z.bp.json", "Z").unwrap();
        // Now reads back without crashing.
        let entries = load(&dir.0).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn parallel_adds_dont_lose_entries() {
        // Spawn N threads that each call `add` with a unique path.
        // Without the inventory mutex, two parallel invocations would
        // load the same baseline, both append/mutate their private
        // copy, and the last-write-wins would drop most entries.
        // With the mutex, all entries are preserved.
        let dir = TempDir::new("parallel");
        let n_threads = 32;
        let threads: Vec<std::thread::JoinHandle<()>> = (0..n_threads)
            .map(|i| {
                let dir = dir.0.clone();
                let path = format!("/tmp/p-{i}.bp.json");
                std::thread::spawn(move || {
                    add(&dir, &path, &path).expect("add should succeed");
                })
            })
            .collect();
        for h in threads {
            h.join().expect("worker thread panicked");
        }
        let entries = load(&dir.0).unwrap();
        assert_eq!(
            entries.len(),
            n_threads,
            "every parallel-add path should be present in the final inventory (lost-entry race)"
        );
        let mut seen: std::collections::HashSet<String> = entries.iter().map(|e| e.path.clone()).collect();
        for i in 0..n_threads {
            assert!(seen.remove(&format!("/tmp/p-{i}.bp.json")));
        }
        assert!(seen.is_empty());
    }
}
