//! API-key storage (AI-SEC-002).
//!
//! Keys live in `<app_data_dir>/secrets.json`, **not** in a blueprint and
//! **not** in the source tree. On Unix the file is created with mode
//! `0600` so other local users cannot read it; on Windows it inherits the
//! per-user ACL of the app data directory.
//!
//! The store is intentionally tiny and swappable: [`SecretStore`] is the
//! only type the rest of the backend touches, so moving to an OS keychain
//! later (Keychain / Credential Manager / Secret Service) means
//! reimplementing this one struct.
//!
//! **The key is never returned to the frontend.** [`SecretStatus`] carries
//! only `configured` plus a last-4 hint, which is enough for the settings
//! dialog to show "configured" without putting the secret back on the wire.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::AiProviderKind;

const FILE_NAME: &str = "secrets.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct SecretsFile {
    #[serde(default)]
    providers: BTreeMap<String, String>,
}

/// Non-secret view of one provider's credential, for the settings dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretStatus {
    pub provider: String,
    pub configured: bool,
    /// Last four characters, e.g. `"…ab12"`. `None` when unconfigured or
    /// when the key is short enough that showing any of it would give it
    /// away entirely.
    pub hint: Option<String>,
}

/// On-disk credential store rooted at the app data directory.
#[derive(Debug, Clone)]
pub struct SecretStore {
    dir: PathBuf,
}

impl SecretStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
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

    fn read(&self) -> Result<SecretsFile, String> {
        let path = self.file_path();
        match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|e| format!("parse {}: {e}", path.display())),
            // A missing store simply means "nothing configured yet".
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SecretsFile::default()),
            Err(e) => Err(format!("read {}: {e}", path.display())),
        }
    }

    /// Write through a temp file + rename so a crash cannot leave a
    /// half-written store behind (same rule as `lib.rs::write_atomically`).
    /// The `0600` mode is applied to the temp file, so the final rename
    /// publishes a file that already has the right permissions.
    fn write(&self, file: &SecretsFile) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| format!("create {}: {e}", self.dir.display()))?;
        let raw = serde_json::to_string_pretty(file)
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

    /// Look up the API key for `provider` (canonical name).
    pub fn get(&self, provider: &str) -> Result<Option<String>, String> {
        let canonical = canonical_provider(provider)?;
        Ok(self.read()?.providers.get(&canonical).cloned())
    }

    /// Store (or replace) the key for `provider`. Empty keys are rejected
    /// rather than stored — an empty key would look "configured" in the UI
    /// and then fail at run time.
    pub fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        let canonical = canonical_provider(provider)?;
        if key.trim().is_empty() {
            return Err("API key is empty".into());
        }
        let mut file = self.read()?;
        file.providers
            .insert(canonical, key.trim().to_string());
        self.write(&file)
    }

    /// Remove the key for `provider`. Returns whether something was
    /// actually removed.
    pub fn clear(&self, provider: &str) -> Result<bool, String> {
        let canonical = canonical_provider(provider)?;
        let mut file = self.read()?;
        let removed = file.providers.remove(&canonical).is_some();
        if removed {
            self.write(&file)?;
        }
        Ok(removed)
    }

    /// Status for every known provider, in `AiProviderKind::ALL` order.
    pub fn status(&self) -> Result<Vec<SecretStatus>, String> {
        let file = self.read()?;
        Ok(AiProviderKind::ALL
            .iter()
            .map(|&provider| {
                let key = file.providers.get(provider);
                SecretStatus {
                    provider: provider.to_string(),
                    configured: key.is_some(),
                    hint: key.and_then(|k| hint(k)),
                }
            })
            .collect())
    }
}

/// Reject unknown provider names so a typo cannot silently create a
/// secret nothing ever reads.
fn canonical_provider(provider: &str) -> Result<String, String> {
    AiProviderKind::parse(provider)
        .map(|p| p.as_str().to_string())
        .ok_or_else(|| {
            format!(
                "unknown provider '{provider}' (expected one of: {})",
                AiProviderKind::ALL.join(", ")
            )
        })
}

/// Last-four hint. Deliberately refuses to hint on short keys: for a
/// 6-character key, showing 4 characters would leak most of it.
fn hint(key: &str) -> Option<String> {
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

    #[test]
    fn set_get_clear_roundtrip() {
        let store = temp_store("roundtrip");
        assert_eq!(store.get("openai").unwrap(), None);

        store.set("openai", "  sk-test-1234567890  ").unwrap();
        assert_eq!(
            store.get("openai").unwrap().as_deref(),
            Some("sk-test-1234567890")
        );

        // Provider names normalise, so a saved key is found again no
        // matter how the caller spells the provider.
        assert_eq!(
            store.get("OpenAI").unwrap().as_deref(),
            Some("sk-test-1234567890")
        );

        assert!(store.clear("openai").unwrap());
        assert!(!store.clear("openai").unwrap());
        assert_eq!(store.get("openai").unwrap(), None);

        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn status_reports_configured_without_leaking_the_key() {
        let store = temp_store("status");
        store.set("anthropic", "sk-ant-abcdefghijkl").unwrap();

        let status = store.status().unwrap();
        assert_eq!(status.len(), 2);
        let anthropic = status
            .iter()
            .find(|s| s.provider == "anthropic")
            .expect("anthropic entry");
        assert!(anthropic.configured);
        assert_eq!(anthropic.hint.as_deref(), Some("…ijkl"));
        // The full secret must never appear in the status payload.
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("sk-ant-abcdefghijkl"));

        let openai = status
            .iter()
            .find(|s| s.provider == "openai")
            .expect("openai entry");
        assert!(!openai.configured);
        assert_eq!(openai.hint, None);

        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[test]
    fn short_keys_get_no_hint() {
        assert_eq!(hint("short"), None);
        assert_eq!(hint("12345678"), Some("…5678".to_string()));
    }

    #[test]
    fn unknown_provider_and_empty_key_are_rejected() {
        let store = temp_store("reject");
        assert!(store.set("nope", "key-123456789").is_err());
        assert!(store.set("openai", "   ").is_err());
        let _ = std::fs::remove_dir_all(store.dir);
    }

    #[cfg(unix)]
    #[test]
    fn store_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let store = temp_store("perms");
        store.set("openai", "sk-test-1234567890").unwrap();
        let mode = std::fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "secrets.json must be 0600");
        let _ = std::fs::remove_dir_all(store.dir);
    }
}
