//! Provider + API-key storage (AI-SEC-002).
//!
//! Providers are configured **globally**, in the toolbar's AI Providers
//! dialog, and live in `<app_data_dir>/secrets.json` — **not** in a
//! blueprint and **not** in the source tree. On Unix the file is created
//! with mode `0600` so other local users cannot read it; on Windows it
//! inherits the per-user ACL of the app data directory.
//!
//! On-disk shape:
//!
//! ```json
//! { "providers": [ { "id": "ollama", "kind": "openai", "label": "Local",
//!                    "base_url": "http://127.0.0.1:11434/v1",
//!                    "models": ["llama3.1:70b"], "api_key": "" } ] }
//! ```
//!
//! `id` is the stable handle a node refers to (`"ollama::llama3.1:70b"`)
//! and never changes: renaming a provider only changes `label`. Several
//! providers of the same `kind` are legal (OpenAI *and* a local Ollama,
//! each with its own `base_url` and key).
//!
//! Older files stored `providers` as a flat `kind -> key` map. `read()`
//! accepts both shapes and normalises the legacy one into records whose
//! `id` is the kind name, so existing installs keep working and the node
//! resolution only ever has to deal with records.
//!
//! The store is intentionally tiny and swappable: [`SecretStore`] is the
//! only type the rest of the backend touches, so moving to an OS keychain
//! later (Keychain / Credential Manager / Secret Service) means
//! reimplementing this one struct.
//!
//! **A key is never returned to the frontend.** [`ProviderStatus`] carries
//! `configured` plus a last-4 hint, which is enough for the dialog to show
//! state without putting the secret back on the wire.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiProviderKind, ProviderInfo};

const FILE_NAME: &str = "secrets.json";

/// One configured provider, as stored on disk.
///
/// Every field has a `default` so a hand-edited (or partially written)
/// entry still parses; validation happens in [`SecretStore::save`], which
/// is the only place that *creates* records.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderRecord {
    /// Stable handle nodes use: `openai`, `ollama`, `work-anthropic`…
    #[serde(default)]
    pub id: String,
    /// `AiProviderKind` canonical name (`openai` / `anthropic`).
    #[serde(default)]
    pub kind: String,
    /// Display name. Empty means "show the id".
    #[serde(default)]
    pub label: String,
    /// Provider endpoint. Empty means "the wire's own default".
    #[serde(default)]
    pub base_url: String,
    /// Models offered in the node's Model dropdown. Advisory: a model
    /// typed by hand is still sent as-is.
    #[serde(default)]
    pub models: Vec<String>,
    /// The secret. Empty for a keyless local server (`base_url` set).
    #[serde(default)]
    pub api_key: String,
}

impl ProviderRecord {
    fn kind_parsed(&self) -> AiProviderKind {
        // `normalize` rejects unknown kinds, so this is a defensive fallback.
        AiProviderKind::parse(&self.kind).unwrap_or_default()
    }

    /// View for the executor / gateway: no key, but `has_key`.
    pub fn to_info(&self) -> ProviderInfo {
        ProviderInfo {
            id: self.id.clone(),
            kind: self.kind_parsed(),
            label: self.label.clone(),
            base_url: non_empty(&self.base_url),
            models: self.models.clone(),
            has_key: !self.api_key.trim().is_empty(),
        }
    }

    /// View for the dialog: no key, but `configured` plus a hint.
    pub fn to_status(&self) -> ProviderStatus {
        ProviderStatus {
            id: self.id.clone(),
            kind: self.kind.clone(),
            label: self.label.clone(),
            base_url: self.base_url.clone(),
            models: self.models.clone(),
            configured: !self.api_key.trim().is_empty(),
            hint: hint(&self.api_key),
        }
    }
}

/// One provider as the dialog submits it (IPC DTO).
///
/// `#[serde(rename_all = "camelCase")]` because Tauri v2 hands command
/// arguments over in camelCase. An empty `apiKey` means "keep whatever key
/// is already stored" — the frontend never receives the key back, so it
/// has nothing to prefill.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDraft {
    /// Empty asks the store to generate one (`ollama`, `ollama-2`, …).
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub api_key: String,
}

/// Non-secret view of one configured provider, for the settings dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub base_url: String,
    pub models: Vec<String>,
    pub configured: bool,
    /// Last four characters, e.g. `"…ab12"`. `None` when unconfigured or
    /// when the key is short enough that showing any of it would give it
    /// away entirely.
    pub hint: Option<String>,
}

/// Serialized form of the file. Always an array, whatever the input shape
/// was, so a migrated file is rewritten in the current shape.
#[derive(Serialize)]
struct SecretsFile<'a> {
    providers: &'a [ProviderRecord],
}

/// On-disk provider/credential store rooted at the app data directory.
#[derive(Debug, Clone)]
pub struct SecretStore {
    dir: PathBuf,
}

impl SecretStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Directory holding `secrets.json`. The app-tool store lives in the
    /// same directory, which is how the gateway reaches it without a
    /// second path being threaded through every call.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Absolute path of the store file (used by the permission test and
    /// handy when a future diagnostics panel needs to print it).
    #[allow(dead_code)]
    pub fn path(&self) -> PathBuf {
        self.file_path()
    }

    fn file_path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Records in file order, migrated and validated. Unreadable entries
    /// are skipped with a log line rather than failing the whole store:
    /// one broken entry must not hide every other provider.
    fn read(&self) -> Result<Vec<ProviderRecord>, String> {
        let path = self.file_path();
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            // A missing store simply means "nothing configured yet".
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        let value: Value = serde_json::from_str(&raw)
            .map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(normalize(&value))
    }

    /// Write through a temp file + rename so a crash cannot leave a
    /// half-written store behind (same rule as `lib.rs::write_atomically`).
    /// The `0600` mode is applied to the temp file, so the final rename
    /// publishes a file that already has the right permissions.
    fn write(&self, records: &[ProviderRecord]) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| format!("create {}: {e}", self.dir.display()))?;
        let file = SecretsFile { providers: records };
        let raw = serde_json::to_string_pretty(&file)
            .map_err(|e| format!("serialize secret store: {e}"))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = self
            .dir
            .join(format!(".{FILE_NAME}.{}.{nanos}.tmp", std::process::id()));
        fs::write(&tmp, raw.as_bytes())
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        restrict_permissions(&tmp)?;
        fs::rename(&tmp, self.file_path())
            .map_err(|e| format!("rename {}: {e}", self.file_path().display()))?;
        Ok(())
    }

    /// Every configured provider (full records, key included). Gateway
    /// code only; never handed to the frontend.
    pub fn records(&self) -> Result<Vec<ProviderRecord>, String> {
        self.read()
    }

    /// Provider list for the executor (`AiGateway::providers`).
    pub fn provider_infos(&self) -> Result<Vec<ProviderInfo>, String> {
        Ok(self.read()?.iter().map(ProviderRecord::to_info).collect())
    }

    /// Status for every configured provider, in file order.
    pub fn statuses(&self) -> Result<Vec<ProviderStatus>, String> {
        Ok(self.read()?.iter().map(ProviderRecord::to_status).collect())
    }

    /// Look one provider up by id (case-insensitive: ids are stored
    /// lowercase, a hand-edited blueprint may not be).
    pub fn record(&self, id: &str) -> Result<Option<ProviderRecord>, String> {
        let needle = lookup_id(id);
        Ok(self.read()?.into_iter().find(|r| r.id == needle))
    }

    /// Create or update one provider. Returns the id it was stored under
    /// (a generated one when the draft had no id).
    pub fn save(&self, draft: ProviderDraft) -> Result<String, String> {
        let kind = AiProviderKind::parse(&draft.kind).ok_or_else(|| {
            format!(
                "unknown provider kind '{}' (expected one of: {})",
                draft.kind.trim(),
                AiProviderKind::ALL.join(", ")
            )
        })?;

        let mut records = self.read()?;
        let id = if draft.id.trim().is_empty() {
            generate_id(kind.as_str(), &records)
        } else {
            canonical_id(&draft.id).ok_or_else(|| {
                format!(
                    "provider id {:?} is not valid: use letters, digits, '.' '-' or '_' (no spaces, no ':')",
                    draft.id
                )
            })?
        };

        let base_url = draft.base_url.trim().to_string();
        let existing = records.iter().find(|r| r.id == id);
        // An empty key field means "leave the stored key alone", because
        // the dialog can never show the key it would have to resend.
        let api_key = match draft.api_key.trim() {
            "" => existing.map(|r| r.api_key.clone()).unwrap_or_default(),
            key => key.to_string(),
        };
        if api_key.is_empty() && base_url.is_empty() {
            return Err(format!(
                "'{id}' needs an API key, or a Base URL for a local server (Ollama, LM Studio…)"
            ));
        }

        let record = ProviderRecord {
            id: id.clone(),
            kind: kind.as_str().to_string(),
            label: draft.label.trim().to_string(),
            base_url,
            models: clean_models(&draft.models),
            api_key,
        };
        match records.iter_mut().find(|r| r.id == id) {
            Some(slot) => *slot = record,
            None => records.push(record),
        }
        self.write(&records)?;
        Ok(id)
    }

    /// Drop a whole provider (key included).
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let needle = lookup_id(id);
        let mut records = self.read()?;
        let before = records.len();
        records.retain(|r| r.id != needle);
        if records.len() == before {
            return Ok(false);
        }
        self.write(&records)?;
        Ok(true)
    }

    /// Keep the provider, drop only its key. Legal even when that leaves
    /// the provider unusable: the run then reports a missing key instead
    /// of the dialog refusing to show state the user can fix later.
    pub fn clear_key(&self, id: &str) -> Result<bool, String> {
        let needle = lookup_id(id);
        let mut records = self.read()?;
        match records.iter_mut().find(|r| r.id == needle) {
            Some(record) => {
                record.api_key.clear();
                self.write(&records)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

/// Legacy + current shapes -> records.
fn normalize(value: &Value) -> Vec<ProviderRecord> {
    let Some(providers) = value.get("providers") else {
        return Vec::new();
    };
    match providers {
        Value::Array(items) => items.iter().filter_map(parse_record).collect(),
        // `providers` used to be `{ "openai": "sk-…", "anthropic": "sk-…" }`.
        // The migrated id is the kind name, so a blueprint that still says
        // `provider: "openai"` resolves to that record.
        Value::Object(map) => map
            .iter()
            .filter_map(|(kind, key)| {
                let Some(kind) = AiProviderKind::parse(kind) else {
                    eprintln!("secrets.json: ignoring unknown provider kind {kind:?}");
                    return None;
                };
                let api_key = key.as_str().unwrap_or_default().trim().to_string();
                if api_key.is_empty() {
                    eprintln!("secrets.json: ignoring empty key for {kind:?}");
                    return None;
                }
                Some(ProviderRecord {
                    id: kind.as_str().to_string(),
                    kind: kind.as_str().to_string(),
                    label: String::new(),
                    base_url: String::new(),
                    models: Vec::new(),
                    api_key,
                })
            })
            .collect(),
        other => {
            eprintln!("secrets.json: ignoring unexpected `providers` value: {other}");
            Vec::new()
        }
    }
}

fn parse_record(value: &Value) -> Option<ProviderRecord> {
    let raw: ProviderRecord = match serde_json::from_value(value.clone()) {
        Ok(record) => record,
        Err(e) => {
            eprintln!("secrets.json: ignoring unreadable provider entry: {e}");
            return None;
        }
    };
    let Some(id) = canonical_id(&raw.id) else {
        eprintln!("secrets.json: ignoring provider with invalid id {:?}", raw.id);
        return None;
    };
    let Some(kind) = AiProviderKind::parse(&raw.kind) else {
        eprintln!(
            "secrets.json: ignoring provider {id:?} with unknown kind {:?}",
            raw.kind
        );
        return None;
    };
    Some(ProviderRecord {
        id,
        kind: kind.as_str().to_string(),
        label: raw.label.trim().to_string(),
        base_url: raw.base_url.trim().to_string(),
        models: clean_models(&raw.models),
        api_key: raw.api_key.trim().to_string(),
    })
}

/// Ids are lowercase and carry no `:` — the node's `providerId::model`
/// separator has to stay unambiguous.
fn canonical_id(raw: &str) -> Option<String> {
    let id = raw.trim().to_ascii_lowercase();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
        return None;
    }
    Some(id)
}

fn lookup_id(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// Free id for a new provider of this kind: `openai`, then `openai-2`,
/// `openai-3`, … skipping ids already in use. A hand-written id (e.g.
/// `ollama`) is allowed as long as `canonical_id` accepts it; only the
/// generated ones follow the kind.
fn generate_id(kind: &str, records: &[ProviderRecord]) -> String {
    if !records.iter().any(|r| r.id == kind) {
        return kind.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{kind}-{n}");
        if !records.iter().any(|r| r.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn clean_models(models: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() || out.iter().any(|kept| kept == model) {
            continue;
        }
        out.push(model.to_string());
    }
    out
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Last-four hint. Deliberately refuses to hint on short keys: for a
/// 6-character key, showing 4 characters would leak most of it.
fn hint(key: &str) -> Option<String> {
    let key = key.trim();
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 8 {
        return None;
    }
    Some(format!(
        "…{}",
        chars[chars.len() - 4..].iter().collect::<String>()
    ))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), String> {
    // Windows inherits the per-user ACL of the app data directory; there
    // is no portable mode bit to set here.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh directory under the OS temp dir, unique per test thread.
    fn temp_store(label: &str) -> SecretStore {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "bp-secrets-{label}-{}-{nanos}",
            std::process::id()
        ));
        SecretStore::new(p)
    }

    fn draft(kind: &str, api_key: &str) -> ProviderDraft {
        ProviderDraft {
            kind: kind.to_string(),
            api_key: api_key.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn save_record_status_roundtrip() {
        let store = temp_store("roundtrip");
        assert!(store.records().unwrap().is_empty());

        let id = store
            .save(ProviderDraft {
                id: "openai".into(),
                kind: "openai".into(),
                label: "OpenAI".into(),
                base_url: String::new(),
                models: vec!["gpt-4o-mini".into(), " gpt-4o ".into(), "gpt-4o-mini".into()],
                api_key: "  sk-test-1234567890  ".into(),
            })
            .unwrap();
        assert_eq!(id, "openai");

        let record = store.record("openai").unwrap().expect("saved record");
        assert_eq!(record.kind, "openai");
        assert_eq!(record.label, "OpenAI");
        assert_eq!(record.api_key, "sk-test-1234567890");
        // Models are trimmed and de-duplicated on the way in.
        assert_eq!(record.models, vec!["gpt-4o-mini", "gpt-4o"]);

        // Ids are stored lowercase, so any spelling finds them.
        assert!(store.record("OpenAI").unwrap().is_some());

        // Saving again with the same id updates in place, in file order.
        store
            .save(ProviderDraft {
                id: "openai".into(),
                kind: "openai".into(),
                label: "OpenAI prod".into(),
                models: vec!["gpt-4o".into()],
                api_key: String::new(),
                ..Default::default()
            })
            .unwrap();
        let records = store.records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].label, "OpenAI prod");
        assert_eq!(records[0].api_key, "sk-test-1234567890", "empty key keeps the old one");

        assert!(store.remove("openai").unwrap());
        assert!(!store.remove("openai").unwrap());
        assert!(store.records().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn generated_ids_do_not_collide() {
        let store = temp_store("ids");
        assert_eq!(store.save(draft("openai", "sk-one-12345678")).unwrap(), "openai");
        assert_eq!(store.save(draft("openai", "sk-two-12345678")).unwrap(), "openai-2");
        assert_eq!(store.save(draft("anthropic", "sk-ant-12345678")).unwrap(), "anthropic");
        assert_eq!(
            store.records().unwrap().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["openai", "openai-2", "anthropic"]
        );
        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn keyless_provider_needs_a_base_url() {
        let store = temp_store("keyless");
        // Local server (Ollama, LM Studio): no key is fine.
        let id = store
            .save(ProviderDraft {
                id: "ollama".into(),
                kind: "openai".into(),
                base_url: "http://127.0.0.1:11434/v1".into(),
                models: vec!["llama3.1:70b".into()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(id, "ollama");
        assert!(!store.statuses().unwrap()[0].configured);

        // Same kind without a base_url would talk to api.openai.com with
        // no credential, which is a confusing failure — reject it here.
        let err = store
            .save(ProviderDraft {
                id: "hosted".into(),
                kind: "openai".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert!(err.contains("API key"), "{err}");

        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn invalid_id_and_kind_are_rejected() {
        let store = temp_store("reject");
        // `:` would break `providerId::model` splitting.
        assert!(store
            .save(ProviderDraft {
                id: "bad id".into(),
                kind: "openai".into(),
                api_key: "sk-x-12345678".into(),
                ..Default::default()
            })
            .is_err());
        assert!(store.save(draft("nope", "sk-x-12345678")).is_err());
        assert!(store.records().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn legacy_kind_map_is_migrated() {
        let store = temp_store("legacy");
        fs::create_dir_all(&store.dir).unwrap();
        fs::write(
            store.file_path(),
            r#"{ "providers": { "openai": "sk-old-12345678", "bogus": "sk-x", "": "sk-y" } }"#,
        )
        .unwrap();

        let records = store.records().unwrap();
        assert_eq!(records.len(), 1, "unknown kinds are dropped, not fatal");
        // The migrated id is the kind name, which is what a legacy
        // `provider: "openai"` param resolves to.
        assert_eq!(records[0].id, "openai");
        assert_eq!(records[0].api_key, "sk-old-12345678");

        // The next write publishes the current shape, not the map.
        store.save(draft("anthropic", "sk-ant-12345678")).unwrap();
        let raw: Value = serde_json::from_str(&fs::read_to_string(store.file_path()).unwrap()).unwrap();
        assert!(raw["providers"].is_array(), "{raw}");

        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn statuses_report_configured_without_leaking_the_key() {
        let store = temp_store("status");
        store.save(draft("anthropic", "sk-ant-abcdefghijkl")).unwrap();
        store
            .save(ProviderDraft {
                id: "ollama".into(),
                kind: "openai".into(),
                base_url: "http://127.0.0.1:11434/v1".into(),
                models: vec!["llama3.1:70b".into()],
                ..Default::default()
            })
            .unwrap();

        let status = store.statuses().unwrap();
        assert_eq!(status.len(), 2);
        let anthropic = status.iter().find(|s| s.id == "anthropic").expect("anthropic");
        assert!(anthropic.configured);
        assert_eq!(anthropic.hint.as_deref(), Some("…ijkl"));
        // The full secret must never appear in the status payload.
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("sk-ant-abcdefghijkl"));

        let ollama = status.iter().find(|s| s.id == "ollama").expect("ollama");
        assert!(!ollama.configured);
        assert_eq!(ollama.hint, None);
        assert_eq!(ollama.base_url, "http://127.0.0.1:11434/v1");

        // `clear_key` keeps the provider but drops the secret.
        store.clear_key("anthropic").unwrap();
        let status = store.statuses().unwrap();
        let anthropic = status.iter().find(|s| s.id == "anthropic").expect("anthropic");
        assert!(!anthropic.configured);
        assert_eq!(anthropic.models, Vec::<String>::new());

        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn provider_infos_hide_the_key_but_show_has_key() {
        let store = temp_store("infos");
        store.save(draft("openai", "sk-test-1234567890")).unwrap();
        store
            .save(ProviderDraft {
                id: "ollama".into(),
                kind: "openai".into(),
                base_url: "http://127.0.0.1:11434/v1".into(),
                ..Default::default()
            })
            .unwrap();

        let infos = store.provider_infos().unwrap();
        assert_eq!(infos.len(), 2);
        assert!(infos[0].has_key);
        assert_eq!(infos[0].base_url, None);
        assert_eq!(infos[0].kind, AiProviderKind::OpenAi);
        assert!(!infos[1].has_key);
        assert_eq!(infos[1].base_url.as_deref(), Some("http://127.0.0.1:11434/v1"));

        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn short_keys_get_no_hint() {
        assert_eq!(hint("short"), None);
        assert_eq!(hint("12345678"), Some("…5678".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn store_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let store = temp_store("perms");
        store.save(draft("openai", "sk-test-1234567890")).unwrap();
        let mode = std::fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "secrets.json must be 0600");
        let _ = std::fs::remove_dir_all(store.dir);
    }
}
