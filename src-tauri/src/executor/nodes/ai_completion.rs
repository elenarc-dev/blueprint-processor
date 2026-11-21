//! `ai.completion` — sends a prompt (optionally combined with data) to an
//! external model and emits the reply as text.
//!
//! Two data inputs:
//!   * `prompt` — the instruction. A wired value wins; otherwise the
//!     `Prompt (default)` param is used (the runner already resolves an
//!     unwired port to `params[<port_id>]`).
//!   * `data` — optional payload from an upstream node.
//!
//! ## How prompt and data are combined
//!
//! * Prompt contains `{{data}}` → every occurrence is replaced with the
//!   serialized data value (objects and arrays become JSON text, so an
//!   `array.getField` → `array.create` pair can feed a structured row).
//! * Prompt has no placeholder → the data is appended after a blank line,
//!   which is the least surprising default for "here is the instruction,
//!   here is the material".
//! * No data connected → the prompt is sent unchanged. A prompt that
//!   still contains `{{data}}` is an **error**: sending a template with a
//!   hole in it produces a confidently wrong answer, which is worse than
//!   failing.
//!
//! ## Shape mirroring (broadcast rules)
//!
//! The same rules the rest of the catalog uses, applied to the pair
//! (prompt, data):
//!
//! | prompt     | data       | calls              |
//! |------------|------------|--------------------|
//! | scalar     | scalar     | 1                  |
//! | array(N)   | scalar     | N                  |
//! | scalar     | array(N)   | N                  |
//! | array(N)   | array(N)   | N (zipped)         |
//! | array(N)   | array(M≠N) | error              |
//!
//! The output is an array whenever either input was an array — a
//! 1-element array stays a 1-element array — and order is preserved
//! (`buffered`, not `buffer_unordered`).
//!
//! **An array data value means "one call per element".** To send a whole
//! array as a single JSON payload, route it through `json.stringify`
//! first; that turns it into a scalar string and keeps it as one value.
//!
//! This is the only node whose executor is async. It is registered in
//! `setup_async_registry()` (not `setup_registry()`) and therefore takes
//! **owned** inputs: the runner clones them so the boxed future never
//! borrows the per-iteration locals.
//!
//! Credentials are never read from `params`. Providers — their kind,
//! endpoint and API key — are configured **globally** (toolbar → AI
//! Providers) and reach the node through `AiGateway::providers`; the node
//! only names a model. See [`resolve_target`] for the exact cascade.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, StreamExt, TryStreamExt};
use serde_json::{json, Map, Value};

use crate::ai::{
    AiEnv, AiProviderKind, AiRequest, ChatMessage, ChatTurn, ProviderInfo, ToolCall, ToolSpec,
};
use crate::app_tools::AppTool;
use crate::executor::nodes::http_request;
use crate::executor::runner::run_blueprint_path;
use crate::executor::{single, value_to_string, ExecError, NodeOutputs};

/// Placeholder for the `data` input inside the prompt text.
const DATA_PLACEHOLDER: &str = "{{data}}";

pub fn exec<'a>(
    inputs: &'a Map<String, Value>,
    params: &'a Map<String, Value>,
    env: &'a AiEnv,
) -> Pin<Box<dyn Future<Output = Result<NodeOutputs, ExecError>> + Send + 'a>> {
    Box::pin(run(inputs, params, env))
}

async fn run(
    inputs: &Map<String, Value>,
    params: &Map<String, Value>,
    env: &AiEnv,
) -> Result<NodeOutputs, ExecError> {
    let providers = env
        .gateway
        .providers()
        .map_err(|e| ExecError::Node {
            node: env.node_id.clone(),
            message: e.to_string(),
        })?;
    let target = resolve_target(params, &providers, &env.node_id)?;

    let system = params
        .get("system")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let temperature = params.get("temperature").and_then(Value::as_f64);
    let max_tokens = params.get("max_tokens").and_then(Value::as_u64);
    let concurrency = params
        .get("concurrency")
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 16) as usize;

    // The runner already resolves the unwired-port fallback to
    // `params["prompt"]`, so one lookup covers both "wired" and "typed in
    // the node body".
    let prompt_value = inputs.get("prompt").cloned().unwrap_or(Value::Null);
    // `data` has no param fallback on purpose: it is either wired to an
    // upstream node or absent, never typed by hand.
    let data_value = inputs.get("data").cloned().unwrap_or(Value::Null);

    let plan = pair_prompt_and_data(&prompt_value, &data_value, &env.node_id)?;

    // Tools come from two places: app-level tools selected by id in
    // `params.app_tools`, and `ai.tool` descriptors wired into the `tools`
    // port. Both end up as one list of (schema, runner) pairs.
    let bound_tools = collect_tools(params, inputs, env)?;
    let tool_specs: Vec<ToolSpec> = bound_tools.iter().map(|t| t.spec.clone()).collect();
    let max_rounds = params
        .get("max_tool_rounds")
        .and_then(Value::as_u64)
        .unwrap_or(6)
        .clamp(1, 16) as usize;

    let requests: Vec<AiRequest> = plan
        .items
        .iter()
        .map(|(prompt, data)| {
            Ok(AiRequest {
                provider_id: target.provider_id.clone(),
                provider: target.provider,
                model: target.model.clone(),
                base_url: target.base_url.clone(),
                prompt: compose_prompt(prompt, data.as_ref(), &env.node_id)?,
                system: system.clone(),
                temperature,
                max_tokens,
                images: Vec::new(),
            })
        })
        .collect::<Result<_, ExecError>>()?;

    // `buffered` (not `buffer_unordered`) keeps the output order aligned
    // with the input order while still allowing `concurrency` in flight.
    // Without tools this is one plain completion per item; with tools each
    // item runs its own bounded tool-calling conversation.
    let answers: Vec<String> = if bound_tools.is_empty() {
        stream::iter(requests.into_iter().map(|req| {
            let gateway = Arc::clone(&env.gateway);
            async move { gateway.complete(&req).await }
        }))
        .buffered(concurrency)
        .try_collect()
        .await
        .map_err(|e| ExecError::Node {
            node: env.node_id.clone(),
            message: e.to_string(),
        })?
    } else {
        let depth = env.depth;
        let specs = &tool_specs;
        let tools = &bound_tools;
        stream::iter(requests.into_iter().map(|req| {
            let gateway = Arc::clone(&env.gateway);
            let node_id = env.node_id.clone();
            async move {
                run_agentic(
                    &gateway,
                    &req,
                    specs,
                    tools,
                    &node_id,
                    depth,
                    max_rounds,
                )
                .await
            }
        }))
        .buffered(concurrency)
        .try_collect()
        .await?
    };

    if plan.mirror {
        Ok(single(
            "out",
            Value::Array(answers.into_iter().map(Value::String).collect()),
        ))
    } else {
        Ok(single(
            "out",
            Value::String(answers.into_iter().next().unwrap_or_default()),
        ))
    }
}

/// One tool the node can offer the model: the schema the provider sees
/// plus how to actually run it.
struct BoundTool {
    spec: ToolSpec,
    runner: ToolRunner,
}

enum ToolRunner {
    /// App-level tool: a named HTTP endpoint.
    Http {
        method: String,
        url: String,
        headers: String,
    },
    /// Node tool: a saved blueprint run with the call arguments as inputs.
    Blueprint { path: String },
}

fn tool_error(node: &str, message: String) -> ExecError {
    ExecError::Node {
        node: node.to_string(),
        message,
    }
}

/// Merge app-level tools (by id) with wired `ai.tool` descriptors.
///
/// Duplicate names are an error: the model would not know which one to
/// call, and "last one wins" is a silent surprise.
fn collect_tools(
    params: &Map<String, Value>,
    inputs: &Map<String, Value>,
    env: &AiEnv,
) -> Result<Vec<BoundTool>, ExecError> {
    let mut out: Vec<BoundTool> = Vec::new();

    let selected: Vec<String> = params
        .get("app_tools")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if !selected.is_empty() {
        let catalog = env.gateway.tools().map_err(|e| {
            tool_error(&env.node_id, format!("app tools unavailable: {e}"))
        })?;
        for id in selected {
            let tool = catalog.iter().find(|t| t.id == id).ok_or_else(|| {
                tool_error(
                    &env.node_id,
                    format!("app tool '{id}' is not configured (Toolbar → AI Tools)"),
                )
            })?;
            push_tool(&mut out, bound_app_tool(tool), env)?;
        }
    }

    let wired = inputs.get("tools").cloned().unwrap_or(Value::Null);
    let descriptors = match wired {
        Value::Null => Vec::new(),
        Value::Array(items) => items,
        other => vec![other],
    };
    for descriptor in descriptors {
        if descriptor.is_null() {
            continue;
        }
        let obj = descriptor.as_object().ok_or_else(|| {
            tool_error(
                &env.node_id,
                "the Tools port expects ai.tool descriptors (objects) or an array of them".into(),
            )
        })?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(tool_error(
                &env.node_id,
                "a wired tool descriptor has no name".into(),
            ));
        }
        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let parameters = obj
            .get("parameters")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
        let path = obj
            .get("blueprint_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if path.is_empty() {
            return Err(tool_error(
                &env.node_id,
                format!("tool '{name}' has no blueprint_path (use an ai.tool node)"),
            ));
        }
        push_tool(
            &mut out,
            BoundTool {
                spec: ToolSpec {
                    name,
                    description,
                    parameters,
                },
                runner: ToolRunner::Blueprint { path },
            },
            env,
        )?;
    }

    Ok(out)
}

fn bound_app_tool(tool: &AppTool) -> BoundTool {
    BoundTool {
        spec: ToolSpec {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        },
        runner: ToolRunner::Http {
            method: tool.method.clone(),
            url: tool.url.clone(),
            headers: tool.headers.clone(),
        },
    }
}

fn push_tool(
    tools: &mut Vec<BoundTool>,
    tool: BoundTool,
    env: &AiEnv,
) -> Result<(), ExecError> {
    if tools.iter().any(|t| t.spec.name == tool.spec.name) {
        return Err(tool_error(
            &env.node_id,
            format!("duplicate tool name '{}'", tool.spec.name),
        ));
    }
    tools.push(tool);
    Ok(())
}

/// Run one prompt to completion, letting the model call tools in between.
///
/// A failing tool does not abort the run: its error is handed back to the
/// model as that tool's result, which lets it retry with better arguments
/// or explain the failure in the final answer.
async fn run_agentic(
    gateway: &Arc<dyn crate::ai::AiGateway>,
    req: &AiRequest,
    specs: &[ToolSpec],
    tools: &[BoundTool],
    node: &str,
    depth: usize,
    max_rounds: usize,
) -> Result<String, ExecError> {
    let mut messages = vec![ChatMessage::User(req.prompt.clone())];
    for _ in 0..max_rounds {
        let turn = gateway
            .complete_turn(req, &messages, specs)
            .await
            .map_err(|e| tool_error(node, e.to_string()))?;
        let ChatTurn { text, tool_calls } = turn;
        if tool_calls.is_empty() {
            return Ok(text);
        }
        messages.push(ChatMessage::Assistant {
            text,
            tool_calls: tool_calls.clone(),
        });
        for call in tool_calls {
            let content = execute_tool(tools, &call, gateway, depth).await;
            messages.push(ChatMessage::Tool {
                call_id: call.id,
                name: call.name,
                content,
            });
        }
    }
    Err(tool_error(
        node,
        format!(
            "the model kept requesting tools after {max_rounds} rounds; raise 'Max tool rounds' or simplify the task"
        ),
    ))
}

/// Execute one tool call and return the text the model will see. Errors
/// are serialized into the result instead of aborting the conversation.
async fn execute_tool(
    tools: &[BoundTool],
    call: &ToolCall,
    gateway: &Arc<dyn crate::ai::AiGateway>,
    depth: usize,
) -> String {
    let Some(bound) = tools.iter().find(|t| t.spec.name == call.name) else {
        return json!({ "error": format!("unknown tool '{}'", call.name) }).to_string();
    };
    match &bound.runner {
        ToolRunner::Http {
            method,
            url,
            headers,
        } => match http_request::perform(
            method,
            url,
            headers,
            Some(&call.arguments),
            60_000,
        )
        .await
        {
            Ok(outcome) => json!({ "status": outcome.status, "body": outcome.text }).to_string(),
            Err(message) => json!({ "error": message }).to_string(),
        },
        ToolRunner::Blueprint { path } => {
            let args = match &call.arguments {
                Value::Object(map) => map.clone(),
                Value::Null => Map::new(),
                other => {
                    return json!({ "error": format!(
                        "tool arguments must be a JSON object, got {}",
                        crate::executor::value_type_name(other)
                    ) })
                    .to_string()
                }
            };
            let external: HashMap<String, Value> = args.into_iter().collect();
            match run_blueprint_path(path, &external, Arc::clone(gateway), depth + 1).await {
                Ok(result) => serde_json::to_string(&result.outputs)
                    .unwrap_or_else(|_| "{}".to_string()),
                Err(message) => json!({ "error": message }).to_string(),
            }
        }
    }
}

/// One resolved `(prompt, data)` pair, plus whether the output must be an
/// array.
struct Plan {
    items: Vec<(String, Option<Value>)>,
    mirror: bool,
}

/// Resolve the cardinality of `(prompt, data)` into 1..N calls.
fn pair_prompt_and_data(
    prompt: &Value,
    data: &Value,
    node: &str,
) -> Result<Plan, ExecError> {
    let (prompt_mirror, prompts) = match prompt {
        Value::String(s) if !s.trim().is_empty() => (false, vec![s.clone()]),
        Value::Array(items) if !items.is_empty() => {
            (true, items.iter().map(value_to_string).collect())
        }
        _ => {
            return Err(ExecError::Node {
                node: node.to_string(),
                message:
                    "prompt is required: wire the 'Prompt' port or set the 'Prompt (default)' param"
                        .into(),
            })
        }
    };

    // `Null` / empty string / empty object-ish values count as "no data".
    let data_opt: Option<(bool, Vec<Value>)> = match data {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::Array(items) => Some((true, items.clone())),
        other => Some((false, vec![other.clone()])),
    };

    let Some((data_mirror, datas)) = data_opt else {
        return Ok(Plan {
            mirror: prompt_mirror,
            items: prompts.into_iter().map(|p| (p, None)).collect(),
        });
    };

    let mirror = prompt_mirror || data_mirror;

    // An empty data array means "zero rows in", so zero calls out. The
    // shape still mirrors (empty array), which keeps a dataset pipeline
    // honest instead of silently falling back to one untyped call.
    if datas.is_empty() {
        return Ok(Plan {
            mirror: true,
            items: Vec::new(),
        });
    }

    if prompts.len() == 1 {
        let prompt = prompts[0].clone();
        return Ok(Plan {
            mirror,
            items: datas.into_iter().map(|d| (prompt.clone(), Some(d))).collect(),
        });
    }
    if datas.len() == 1 {
        let data = datas[0].clone();
        return Ok(Plan {
            mirror,
            items: prompts
                .into_iter()
                .map(|p| (p, Some(data.clone())))
                .collect(),
        });
    }
    if prompts.len() == datas.len() {
        return Ok(Plan {
            mirror,
            items: prompts
                .into_iter()
                .zip(datas)
                .map(|(p, d)| (p, Some(d)))
                .collect(),
        });
    }

    Err(ExecError::Node {
        node: node.to_string(),
        message: format!(
            "prompt array ({}) and data array ({}) must have the same length",
            prompts.len(),
            datas.len()
        ),
    })
}

/// Merge the prompt text with one data value.
fn compose_prompt(
    prompt: &str,
    data: Option<&Value>,
    node: &str,
) -> Result<String, ExecError> {
    let Some(value) = data else {
        if prompt.contains(DATA_PLACEHOLDER) {
            return Err(ExecError::Node {
                node: node.to_string(),
                message: format!(
                    "the prompt uses {DATA_PLACEHOLDER} but the 'Data' port is empty: connect a value or remove the placeholder"
                ),
            });
        }
        return Ok(prompt.to_string());
    };

    // `value_to_string` renders objects/arrays as JSON, which is what a
    // model should see for structured input.
    let text = value_to_string(value);

    if prompt.contains(DATA_PLACEHOLDER) {
        return Ok(prompt.replace(DATA_PLACEHOLDER, &text));
    }
    if prompt.trim().is_empty() {
        return Ok(text);
    }
    Ok(format!("{prompt}\n\n{text}"))
}

/// Where a node's `model` param actually points: which configured provider
/// answers, under which model name, on which endpoint.
/// Resolved provider destination. Shared with `ai.vision` so both nodes
/// honour the same `model` / legacy `provider` cascade.
pub(crate) struct Target {
    pub(crate) provider_id: String,
    pub(crate) provider: AiProviderKind,
    pub(crate) model: String,
    pub(crate) base_url: Option<String>,
}

/// Separates `providerId::model` in the node's `model` param. `::` rather
/// than `:` because an Ollama tag is already `llama3.1:70b`: the split is
/// made on the **first** `::` and everything after it stays the model name.
const PROVIDER_SEPARATOR: &str = "::";

/// Turn `params.model` into a concrete provider + model.
///
/// Cascade, strongest signal first:
///
/// 1. `providerId::model` — the node names a configured provider, so that
///    provider's kind, endpoint and key are used verbatim. A model written
///    after `::` is sent as-is even if the provider does not list it
///    (advisory list, not a whitelist).
/// 2. No providers configured at all — the pre-catalog behaviour: `kind`
///    from the legacy `provider` param, endpoint from the legacy `base_url`
///    param. Existing blueprints keep running unchanged, and the gateway
///    reports a missing key exactly as before.
/// 3. A model name one of the configured providers offers (exact match,
///    then case-insensitive) — the provider that offers it wins, even over
///    a stale `provider` param.
/// 4. The `provider` param selects a *kind*, then the first configured
///    provider of that kind.
/// 5. One provider configured → it. Several → the first that has a key.
///
/// `base_url` precedence: a `base_url` left in an old blueprint means
/// "talk to this endpoint" and wins over the provider's own, but only when
/// the node does not name the provider explicitly with `::` — an explicit
/// provider reference is a deliberate choice of endpoint.
pub(crate) fn resolve_target(
    params: &Map<String, Value>,
    providers: &[ProviderInfo],
    node: &str,
) -> Result<Target, ExecError> {
    let model = trimmed_param(params, "model");
    let provider_raw = trimmed_param(params, "provider");
    let legacy_url = non_empty(&trimmed_param(params, "base_url"));

    // 1. `providerId::model`. Only a provider reference when there is a
    //    catalog to point at: with nothing configured the whole string is
    //    the model name, which is how every pre-catalog blueprint read it.
    if !providers.is_empty() {
        if let Some((id, rest)) = model.split_once(PROVIDER_SEPARATOR) {
            let provider = find_by_id(providers, id).ok_or_else(|| {
                node_error(
                    node,
                    format!(
                        "model {model:?} selects provider '{}', which is not configured. Configured providers: {}",
                        id.trim(),
                        catalog_list(providers)
                    ),
                )
            })?;
            return Ok(target_of(provider, rest, None));
        }
    }

    // 2. Nothing configured: the legacy params path, unchanged.
    if providers.is_empty() {
        let provider = parse_kind(&provider_raw, node)?;
        let model = non_empty(&model)
            .unwrap_or_else(|| provider.default_model().to_string());
        return Ok(Target {
            provider_id: String::new(),
            provider,
            model,
            base_url: legacy_url,
        });
    }

    // 3-5. Let the catalog decide.
    let provider = pick_provider(providers, &model, &provider_raw, node)?;
    Ok(target_of(provider, &model, legacy_url))
}

fn pick_provider<'p>(
    providers: &'p [ProviderInfo],
    model: &str,
    provider_raw: &str,
    node: &str,
) -> Result<&'p ProviderInfo, ExecError> {
    if let Some(provider) = providers.iter().find(|p| p.models.iter().any(|m| m == model)) {
        return Ok(provider);
    }
    if let Some(provider) = providers
        .iter()
        .find(|p| p.models.iter().any(|m| m.eq_ignore_ascii_case(model)))
    {
        return Ok(provider);
    }
    if !provider_raw.is_empty() {
        let kind = parse_kind(provider_raw, node)?;
        return providers.iter().find(|p| p.kind == kind).ok_or_else(|| {
            node_error(
                node,
                format!(
                    "no configured provider of kind '{}' ({}). Configured providers: {}",
                    kind.as_str(),
                    provider_raw,
                    catalog_list(providers)
                ),
            )
        });
    }
    if providers.len() == 1 {
        return Ok(&providers[0]);
    }
    Ok(providers.iter().find(|p| p.has_key).unwrap_or(&providers[0]))
}

fn target_of(provider: &ProviderInfo, model: &str, legacy_url: Option<String>) -> Target {
    let model = non_empty(model).unwrap_or_else(|| {
        // No model chosen: the provider's own first model, or the cheap
        // default of its kind when the provider lists none.
        provider
            .models
            .first()
            .cloned()
            .unwrap_or_else(|| provider.kind.default_model().to_string())
    });
    Target {
        provider_id: provider.id.clone(),
        provider: provider.kind,
        model,
        base_url: legacy_url.or_else(|| provider.base_url.clone()),
    }
}

fn find_by_id<'p>(providers: &'p [ProviderInfo], id: &str) -> Option<&'p ProviderInfo> {
    let needle = id.trim().to_ascii_lowercase();
    providers.iter().find(|p| p.id == needle)
}

/// An empty `provider` param means "whatever the default kind is", matching
/// the old `unwrap_or("openai")`.
fn parse_kind(raw: &str, node: &str) -> Result<AiProviderKind, ExecError> {
    if raw.is_empty() {
        return Ok(AiProviderKind::default());
    }
    AiProviderKind::parse(raw).ok_or_else(|| {
        node_error(
            node,
            format!(
                "unknown AI provider '{raw}' (expected one of: {})",
                AiProviderKind::ALL.join(", ")
            ),
        )
    })
}

fn trimmed_param(params: &Map<String, Value>, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn catalog_list(providers: &[ProviderInfo]) -> String {
    providers
        .iter()
        .map(|p| p.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn node_error(node: &str, message: String) -> ExecError {
    ExecError::Node {
        node: node.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::{json, Map, Value};

    use super::exec;
    use crate::ai::mock::MockGateway;
    use crate::ai::{AiEnv, AiProviderKind, ProviderInfo};

    fn upper(prompt: &str) -> String {
        prompt.to_uppercase()
    }

    fn echo(prompt: &str) -> String {
        prompt.to_string()
    }

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn env_of(gateway: Arc<MockGateway>) -> AiEnv {
        AiEnv {
            node_id: "ai1".into(),
            gateway,
            depth: 0,
        }
    }

    fn call(
        inputs: Map<String, Value>,
        params: Map<String, Value>,
        env: &AiEnv,
    ) -> Result<HashMap<String, Value>, crate::executor::ExecError> {
        tauri::async_runtime::block_on(exec(&inputs, &params, env))
    }

    fn inputs(prompt: Value, data: Value) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("prompt".into(), prompt);
        m.insert("data".into(), data);
        m
    }

    /// A blueprint that copies its `x` input to output `value`.
    fn write_tool_blueprint() -> std::path::PathBuf {
        let mut path = crate::executor::nodes::test_helpers::unique_tmp("ai-tool");
        path.set_extension("json");
        let bp = json!({
            "schema_version": 1,
            "id": "tool",
            "name": "Tool",
            "description": "",
            "nodes": [
                {
                    "id": "in",
                    "kind": "blueprint.input",
                    "position": {"x": 0.0, "y": 0.0},
                    "params": {},
                    "inputs": [],
                    "outputs": []
                },
                {
                    "id": "out",
                    "kind": "io.output",
                    "position": {"x": 0.0, "y": 0.0},
                    "params": {"name": "value"},
                    "inputs": [
                        {"id": "then_in", "name": "Then", "type": "exec"},
                        {"id": "in", "name": "In", "type": "any"}
                    ],
                    "outputs": []
                }
            ],
            "connections": [
                {"id": "e1", "sourceNode": "in", "sourcePort": "x", "targetNode": "out", "targetPort": "in"}
            ],
            "metadata": {
                "tags": [],
                "inputs": [{"id": "x", "name": "X", "type": "string", "description": ""}]
            }
        });
        std::fs::write(&path, serde_json::to_string(&bp).unwrap()).unwrap();
        path
    }

    /// A wired `ai.tool` descriptor runs its blueprint and the model's
    /// next turn becomes the node's output.
    #[test]
    fn wired_tool_runs_the_subgraph_and_returns_the_final_answer() {
        let path = write_tool_blueprint();
        let gateway = Arc::new(MockGateway::new(echo).with_turns(vec![
            crate::ai::ChatTurn {
                text: String::new(),
                tool_calls: vec![crate::ai::ToolCall {
                    id: "c1".into(),
                    name: "lookup".into(),
                    arguments: json!({ "x": "Ada" }),
                }],
            },
            crate::ai::ChatTurn {
                text: "FINAL".into(),
                tool_calls: Vec::new(),
            },
        ]));
        let env = env_of(gateway.clone());

        let mut call_inputs = Map::new();
        call_inputs.insert("prompt".into(), json!("who?"));
        call_inputs.insert(
            "tools".into(),
            json!({
                "name": "lookup",
                "description": "look up a row",
                "parameters": {"type": "object", "properties": {"x": {"type": "string"}}},
                "blueprint_path": path.to_string_lossy().to_string(),
            }),
        );

        let outs = call(call_inputs, params(json!({})), &env).unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("FINAL"));
        assert_eq!(gateway.calls().len(), 2, "one turn per model round");
        std::fs::remove_file(&path).ok();
    }

    /// Selecting an app tool that is not configured fails loudly instead
    /// of silently calling the model without tools.
    #[test]
    fn unknown_app_tool_errors() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());
        let err = call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "app_tools": ["missing"] })),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not configured"), "{err}");
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn scalar_prompt_produces_scalar_out() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("hello"), Value::Null),
            params(json!({ "model": "m" })),
            &env,
        )
        .unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("HELLO"));
        assert_eq!(gateway.calls().len(), 1);
    }

    #[test]
    fn array_prompt_mirrors_shape_and_keeps_order() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!(["a", "b", "c"]), Value::Null),
            params(json!({ "concurrency": 3 })),
            &env,
        )
        .unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["A", "B", "C"]
        );
        assert_eq!(gateway.calls().len(), 3);
    }

    #[test]
    fn unwired_prompt_falls_back_to_params() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway.clone());
        // The runner resolves the unwired port to `params.prompt`; this
        // test pins that the executor reads exactly that.
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("typed in body"));

        let outs = call(inputs, params(json!({})), &env).unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("TYPED IN BODY"));
    }

    #[test]
    fn missing_prompt_errors() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway);
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), Value::Null);

        let err = call(inputs, params(json!({})), &env).unwrap_err();
        assert!(err.to_string().contains("prompt is required"));
    }

    #[test]
    fn unknown_provider_errors_before_calling_out() {
        let gateway = Arc::new(MockGateway::new(upper));
        let env = env_of(gateway.clone());
        let mut inputs = Map::new();
        inputs.insert("prompt".into(), json!("hi"));

        let err = call(inputs, params(json!({ "provider": "nope" })), &env).unwrap_err();
        assert!(err.to_string().contains("unknown AI provider"));
        assert!(gateway.calls().is_empty());
    }

    // ---------- provider / model resolution ----------

    fn provider(
        id: &str,
        kind: AiProviderKind,
        models: &[&str],
        base_url: Option<&str>,
    ) -> ProviderInfo {
        ProviderInfo {
            id: id.into(),
            kind,
            label: String::new(),
            base_url: base_url.map(str::to_string),
            models: models.iter().map(|m| m.to_string()).collect(),
            has_key: true,
        }
    }

    fn gateway_with(providers: Vec<ProviderInfo>) -> Arc<MockGateway> {
        Arc::new(MockGateway::new(echo).with_providers(providers))
    }

    /// The `model` param is `providerId::model`: the provider's kind,
    /// endpoint and key come from the global config, the model verbatim.
    #[test]
    fn provider_id_in_the_model_selects_that_provider() {
        let gateway = gateway_with(vec![
            provider(
                "ollama",
                AiProviderKind::OpenAi,
                &["llama3.1:70b"],
                Some("http://127.0.0.1:11434/v1"),
            ),
            provider("openai", AiProviderKind::OpenAi, &["gpt-4o"], None),
        ]);
        let env = env_of(gateway.clone());

        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "model": "ollama::llama3.1:70b" })),
            &env,
        )
        .unwrap();
        let calls = gateway.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].provider_id, "ollama");
        assert_eq!(calls[0].provider, AiProviderKind::OpenAi);
        // The Ollama tag keeps its own `:`: only the first `::` splits.
        assert_eq!(calls[0].model, "llama3.1:70b");
        assert_eq!(
            calls[0].base_url.as_deref(),
            Some("http://127.0.0.1:11434/v1")
        );

        // The model list is advisory: a model typed by hand is still sent.
        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "model": "ollama::not-listed" })),
            &env,
        )
        .unwrap();
        assert_eq!(gateway.calls()[1].model, "not-listed");
    }

    #[test]
    fn unknown_provider_id_errors_before_calling_out() {
        let gateway = gateway_with(vec![provider("openai", AiProviderKind::OpenAi, &["gpt-4o"], None)]);
        let env = env_of(gateway.clone());

        let err = call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "model": "nope::gpt-4o" })),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not configured"), "{err}");
        assert!(err.to_string().contains("openai"), "{err}");
        assert!(gateway.calls().is_empty());
    }

    /// A model name wins over a stale `provider` param: the provider that
    /// advertises it is the one that gets the call.
    #[test]
    fn a_configured_model_name_selects_its_provider() {
        let gateway = gateway_with(vec![
            provider("openai", AiProviderKind::OpenAi, &["gpt-4o"], None),
            provider(
                "claude",
                AiProviderKind::Anthropic,
                &["claude-3-5-haiku-latest"],
                None,
            ),
        ]);
        let env = env_of(gateway.clone());

        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "provider": "openai", "model": "claude-3-5-haiku-latest" })),
            &env,
        )
        .unwrap();
        let calls = gateway.calls();
        assert_eq!(calls[0].provider_id, "claude");
        assert_eq!(calls[0].provider, AiProviderKind::Anthropic);

        // Case-insensitive fallback, for a hand-edited blueprint.
        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "model": "GPT-4o" })),
            &env,
        )
        .unwrap();
        assert_eq!(gateway.calls()[1].provider_id, "openai");
    }

    /// With no model list and no model chosen, the provider's kind decides.
    #[test]
    fn blank_model_falls_back_to_the_kind_default() {
        let gateway = gateway_with(vec![provider("claude", AiProviderKind::Anthropic, &[], None)]);
        let env = env_of(gateway.clone());

        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "provider": "anthropic" })),
            &env,
        )
        .unwrap();
        let calls = gateway.calls();
        assert_eq!(calls[0].provider_id, "claude");
        assert_eq!(calls[0].provider, AiProviderKind::Anthropic);
        assert_eq!(calls[0].model, "claude-3-5-haiku-latest");
    }

    #[test]
    fn provider_param_without_a_matching_provider_errors() {
        let gateway = gateway_with(vec![provider("ollama", AiProviderKind::OpenAi, &["llama3.1:70b"], None)]);
        let env = env_of(gateway.clone());

        let err = call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "provider": "anthropic" })),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no configured provider of kind 'anthropic'"), "{err}");
        assert!(gateway.calls().is_empty());
    }

    /// Nothing chosen and several configured: pick one that can answer.
    #[test]
    fn several_providers_resolve_to_a_configured_one() {
        let keyless = ProviderInfo {
            has_key: false,
            ..provider("ollama", AiProviderKind::OpenAi, &["llama3.1:70b"], Some("http://127.0.0.1:11434/v1"))
        };
        let gateway = gateway_with(vec![
            keyless,
            provider("hosted", AiProviderKind::OpenAi, &["gpt-4o"], None),
        ]);
        let env = env_of(gateway.clone());

        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "model": "some-model" })),
            &env,
        )
        .unwrap();
        assert_eq!(gateway.calls()[0].provider_id, "hosted");
    }

    /// The compatibility case: a blueprint written before providers were
    /// global (`docs/examples/AI-blueprint.json`) and an app where nothing
    /// is configured must behave exactly as before.
    #[test]
    fn legacy_params_still_run_with_an_empty_catalog() {
        let gateway = gateway_with(Vec::new());
        let env = env_of(gateway.clone());

        call(
            inputs(json!("hi"), Value::Null),
            params(json!({
                "provider": "openai",
                "model": "google/gemma-4-e4b",
                "base_url": "http://127.0.0.1:1234",
            })),
            &env,
        )
        .unwrap();
        let calls = gateway.calls();
        // No provider record involved: the gateway resolves the key by kind.
        assert_eq!(calls[0].provider_id, "");
        assert_eq!(calls[0].provider, AiProviderKind::OpenAi);
        assert_eq!(calls[0].model, "google/gemma-4-e4b");
        assert_eq!(calls[0].base_url.as_deref(), Some("http://127.0.0.1:1234"));
    }

    /// A `base_url` left in an old blueprint still picks the endpoint, but
    /// an explicit `providerId::model` is a deliberate choice of endpoint.
    #[test]
    fn legacy_base_url_wins_unless_the_provider_is_named() {
        let gateway = gateway_with(vec![provider(
            "ollama",
            AiProviderKind::OpenAi,
            &["llama3.1:70b"],
            Some("http://127.0.0.1:11434/v1"),
        )]);
        let env = env_of(gateway.clone());

        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "model": "custom-model", "base_url": "http://127.0.0.1:1234" })),
            &env,
        )
        .unwrap();
        let calls = gateway.calls();
        assert_eq!(calls[0].provider_id, "ollama");
        assert_eq!(calls[0].base_url.as_deref(), Some("http://127.0.0.1:1234"));

        call(
            inputs(json!("hi"), Value::Null),
            params(json!({ "model": "ollama::llama3.1:70b", "base_url": "http://127.0.0.1:1234" })),
            &env,
        )
        .unwrap();
        assert_eq!(
            gateway.calls()[1].base_url.as_deref(),
            Some("http://127.0.0.1:11434/v1")
        );
    }

    // ---------- data input ----------

    #[test]
    fn data_is_appended_after_a_blank_line() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Summarize:"), json!("one, two")),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(
            outs.get("out").unwrap().as_str(),
            Some("Summarize:\n\none, two")
        );
        assert_eq!(gateway.calls().len(), 1);
    }

    #[test]
    fn placeholder_is_replaced_in_place() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(
                json!("Translate to English: {{data}} (keep it short)"),
                json!("hola"),
            ),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(
            outs.get("out").unwrap().as_str(),
            Some("Translate to English: hola (keep it short)")
        );
    }

    #[test]
    fn every_placeholder_occurrence_is_replaced() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("{{data}} vs {{data}}"), json!("X")),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(outs.get("out").unwrap().as_str(), Some("X vs X"));
    }

    #[test]
    fn structured_data_is_serialized_as_json() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Classify:"), json!({ "name": "ana", "age": 31 })),
            params(json!({})),
            &env,
        )
        .unwrap();
        // `serde_json` runs with `preserve_order`, so the object keeps
        // the insertion order of the blueprint's params.
        assert_eq!(
            outs.get("out").unwrap().as_str(),
            Some("Classify:\n\n{\"name\":\"ana\",\"age\":31}")
        );
    }

    #[test]
    fn placeholder_without_data_errors_instead_of_sending_a_hole() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let err = call(
            inputs(json!("Summarize: {{data}}"), Value::Null),
            params(json!({})),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("{{data}}"));
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn array_data_broadcasts_one_call_per_element() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Row:"), json!(["a", "b"])),
            params(json!({ "concurrency": 2 })),
            &env,
        )
        .unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["Row:\n\na", "Row:\n\nb"]
        );
        assert_eq!(gateway.calls().len(), 2);
    }

    #[test]
    fn equal_length_arrays_are_zipped() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!(["P1", "P2"]), json!(["d1", "d2"])),
            params(json!({})),
            &env,
        )
        .unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["P1\n\nd1", "P2\n\nd2"]
        );
    }

    #[test]
    fn single_element_data_broadcasts_over_a_prompt_array() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!(["P1", "P2"]), json!(["only"])),
            params(json!({})),
            &env,
        )
        .unwrap();
        let arr = outs.get("out").unwrap().as_array().unwrap();
        assert_eq!(
            arr.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(),
            vec!["P1\n\nonly", "P2\n\nonly"]
        );
    }

    #[test]
    fn mismatched_array_lengths_error() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let err = call(
            inputs(json!(["P1", "P2"]), json!(["d1", "d2", "d3"])),
            params(json!({})),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("must have the same length"));
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn empty_data_array_yields_an_empty_output_array() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        let outs = call(
            inputs(json!("Row:"), json!([])),
            params(json!({})),
            &env,
        )
        .unwrap();
        assert_eq!(outs.get("out").unwrap().as_array().unwrap().len(), 0);
        assert!(gateway.calls().is_empty());
    }

    #[test]
    fn blank_prompt_is_an_error_even_with_data() {
        let gateway = Arc::new(MockGateway::new(echo));
        let env = env_of(gateway.clone());

        // Data alone is not an instruction: sending it as the prompt
        // would make the node silently "work" with no question asked.
        let err = call(
            inputs(json!("   "), json!("just the data")),
            params(json!({})),
            &env,
        )
        .unwrap_err();
        assert!(err.to_string().contains("prompt is required"));
        assert!(gateway.calls().is_empty());
    }
}
