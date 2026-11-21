//! App-level AI tools.
//!
//! The first of the two tool flavours the `ai.completion` node supports:
//! tools defined **once per app** (toolbar → AI Tools) and merely selected
//! by id in a node's `app_tools` param. They live in
//! `<app_data_dir>/tools.json`, never in a blueprint, mirroring how AI
//! providers are configured globally.
//!
//! An app tool is a named HTTP endpoint: the model supplies arguments, the
//! backend POSTs them as JSON to `url` and hands the response back to the
//! model. Keeping the definition out of the blueprint means a shared graph
//! never carries an endpoint or an `Authorization` header.
//!
//! Tool names must be usable by OpenAI/Anthropic (letters, digits, `_`,
//! `-`), so [`normalize`] sanitises them on save and the store refuses a
//! tool with no usable name.
//!
//! The on-disk file is written atomically and, on Unix, with mode `0600`
//! because `headers` may contain a token.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const FILE_NAME: &str = "tools.json";
const STORE_VERSION: u32 = 1;

/// One app-level tool. `parameters` is a JSON Schema object the model sees
/// as the function signature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppTool {
    /// Stable handle a node stores in `params.app_tools`.
    #[serde(default)]
    pub id: String,
    /// Function name the model calls.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema object describing the arguments.
    #[serde(default = "default_parameters")]
    pub parameters: Value,
    /// HTTP method (`POST` by default — a tool call has a body).
    #[serde(default = "default_method")]
    pub method: String,
    /// Endpoint the arguments are sent to.
    #[serde(default)]
    pub url: String,
    /// Extra request headers, one `Name: Value` per line.
    #[serde(default)]
    pub headers: String,
}

fn default_parameters() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn default_method() -> String {
    "POST".to_string()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ToolsFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    tools: Vec<AppTool>,
}

fn default_version() -> u32 {
    STORE_VERSION
}

/// Path of the store inside an app-data directory.
pub fn store_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(FILE_NAME)
}

/// Read every configured app tool. A missing file means "none configured".
pub fn list(app_data_dir: &Path) -> Result<Vec<AppTool>, String> {
    let path = store_path(app_data_dir);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let file: ToolsFile =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok(file.tools)
}

fn write_all(app_data_dir: &Path, tools: &[AppTool]) -> Result<(), String> {
    fs::create_dir_all(app_data_dir)
        .map_err(|e| format!("create {}: {e}", app_data_dir.display()))?;
    let envelope = ToolsFile {
        version: STORE_VERSION,
        tools: tools.to_vec(),
    };
    let raw = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("serialize tools: {e}"))?;

    let path = store_path(app_data_dir);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = app_data_dir.join(format!(".tools.{}.{nanos}.tmp", std::process::id()));
    fs::write(&tmp, raw).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))?;

    // `headers` may carry a bearer token; keep the file owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Serialising every read-modify-write, like `inventory` does.
fn store_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Sanitise a name the provider will accept as a function name.
pub fn sanitize_tool_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Fill in what the caller may omit and reject what cannot work.
fn normalize(mut tool: AppTool) -> Result<AppTool, String> {
    tool.name = sanitize_tool_name(&tool.name);
    if tool.name.is_empty() {
        return Err("tool name is required (letters, digits, '_' or '-')".into());
    }
    tool.url = tool.url.trim().to_string();
    if tool.url.is_empty() {
        return Err(format!("tool '{}': url is required", tool.name));
    }
    if !tool.url.starts_with("http://") && !tool.url.starts_with("https://") {
        return Err(format!(
            "tool '{}': url must start with http:// or https://",
            tool.name
        ));
    }
    tool.method = tool.method.trim().to_ascii_uppercase();
    if tool.method.is_empty() {
        tool.method = default_method();
    }
    if !tool.parameters.is_object() {
        tool.parameters = default_parameters();
    }
    tool.id = tool.id.trim().to_string();
    Ok(tool)
}

fn unique_id(tools: &[AppTool], seed: &str) -> String {
    let base = if seed.trim().is_empty() {
        "tool".to_string()
    } else {
        sanitize_tool_name(seed).to_ascii_lowercase()
    };
    if !tools.iter().any(|t| t.id == base) {
        return base;
    }
    let mut i = 2;
    loop {
        let candidate = format!("{base}-{i}");
        if !tools.iter().any(|t| t.id == candidate) {
            return candidate;
        }
        i += 1;
    }
}

/// Create or update a tool by id. Returns the full resulting list so the
/// dialog refreshes in one round-trip.
pub fn save(app_data_dir: &Path, tool: AppTool) -> Result<Vec<AppTool>, String> {
    let _guard = store_lock()
        .lock()
        .expect("app tools mutex poisoned — the guard never panics");
    let mut tools = list(app_data_dir)?;
    let mut tool = normalize(tool)?;

    if tool.id.is_empty() {
        tool.id = unique_id(&tools, &tool.name);
    }
    if let Some(existing) = tools.iter().position(|t| t.id == tool.id) {
        tools[existing] = tool;
    } else {
        tools.push(tool);
    }
    write_all(app_data_dir, &tools)?;
    Ok(tools)
}

/// Drop a tool by id. Returns the full resulting list.
pub fn remove(app_data_dir: &Path, id: &str) -> Result<Vec<AppTool>, String> {
    let _guard = store_lock()
        .lock()
        .expect("app tools mutex poisoned — the guard never panics");
    let mut tools = list(app_data_dir)?;
    tools.retain(|t| t.id != id.trim());
    write_all(app_data_dir, &tools)?;
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let mut p = std::env::temp_dir();
            p.push(format!(
                "bp-tools-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tool(name: &str, url: &str) -> AppTool {
        AppTool {
            id: String::new(),
            name: name.into(),
            description: "d".into(),
            parameters: default_parameters(),
            method: "post".into(),
            url: url.into(),
            headers: String::new(),
        }
    }

    #[test]
    fn empty_store_reads_as_empty() {
        let dir = TempDir::new("empty");
        assert!(list(&dir.0).unwrap().is_empty());
    }

    #[test]
    fn save_generates_ids_and_normalizes() {
        let dir = TempDir::new("save");
        let saved = save(&dir.0, tool("my tool", "https://example.com/x")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "my_tool");
        assert_eq!(saved[0].id, "my_tool");
        assert_eq!(saved[0].method, "POST");

        let again = save(&dir.0, tool("my tool", "https://example.com/y")).unwrap();
        assert_eq!(again.len(), 2);
        assert_eq!(again[1].id, "my_tool-2");
    }

    #[test]
    fn update_keeps_one_row() {
        let dir = TempDir::new("update");
        let first = save(&dir.0, tool("lookup", "https://example.com/a")).unwrap();
        let mut edit = first[0].clone();
        edit.description = "changed".into();
        let after = save(&dir.0, edit).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].description, "changed");
    }

    #[test]
    fn remove_returns_the_remaining_list() {
        let dir = TempDir::new("remove");
        let saved = save(&dir.0, tool("a", "https://example.com/a")).unwrap();
        let after = remove(&dir.0, &saved[0].id).unwrap();
        assert!(after.is_empty());
    }

    #[test]
    fn invalid_tools_are_rejected() {
        let dir = TempDir::new("invalid");
        assert!(save(&dir.0, tool("", "https://example.com")).is_err());
        assert!(save(&dir.0, tool("ok", "")).is_err());
        assert!(save(&dir.0, tool("ok", "ftp://example.com")).is_err());
    }

    #[test]
    fn parameters_fall_back_to_an_empty_object_schema() {
        let dir = TempDir::new("schema");
        let mut t = tool("x", "https://example.com");
        t.parameters = Value::Null;
        let saved = save(&dir.0, t).unwrap();
        assert_eq!(saved[0].parameters["type"], "object");
    }
}
