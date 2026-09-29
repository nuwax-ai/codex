//! Tool declaration and history names must use the same flattening rules.

use std::collections::HashMap;
use std::collections::HashSet;

use codex_api::ResponsesApiRequest;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolName;
use codex_tools::code_mode_name_for_tool_name;
use rig_core::completion::request::ToolDefinition;
use serde_json::Value;
use serde_json::json;

pub(crate) struct RequestTools {
    pub(crate) definitions: Vec<ToolDefinition>,
    pub(crate) meta: ToolMeta,
}

/// Request-scoped tool metadata the stream layer needs after the
/// definitions have been moved into the `CompletionRequest`: names of custom
/// tools (for `CustomToolCall` restoration) and per-tool `strict` flags (for
/// wire injection). Returned alongside the definitions so callers never have
/// to re-parse the tool list.
pub(crate) struct ToolMeta {
    pub(crate) custom_names: HashSet<String>,
    pub(crate) strict: HashMap<String, bool>,
    pub(crate) result_errors: HashMap<String, bool>,
    /// Raw hosted (server-side) tool declarations from the Responses request.
    /// Protocol-specific translation happens in `hosted_tools` (Anthropic
    /// server tools; Chat has no hosted-tool concept and drops them).
    pub(crate) hosted_tools: Vec<Value>,
}

pub(crate) fn flat_name(name: &str, namespace: Option<&str>) -> String {
    match namespace {
        Some(namespace) => code_mode_name_for_tool_name(&ToolName::namespaced(namespace, name)),
        None => name.to_string(),
    }
}

pub(crate) fn request_tools(request: &ResponsesApiRequest) -> RequestTools {
    let mut tools: Vec<Value> = match request.tools.as_ref() {
        None => Vec::new(),
        // Practically unreachable: the raw tool JSON was already valid and
        // shallow. A silent empty list would strip every tool from the
        // request, so surface it loudly in debug builds (only a >128-deep
        // nested schema can trip serde_json's depth limit).
        Some(tools) => serde_json::to_value(tools)
            .ok()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_else(|| {
                debug_assert!(
                    false,
                    "failed to round-trip request tools into the tool list"
                );
                Vec::new()
            }),
    };
    for item in &request.input {
        if let ResponseItem::AdditionalTools { tools: extra, .. } = item {
            tools.extend(extra.iter().cloned());
        }
    }
    let mut selection = parse_tools(&tools);
    for item in &request.input {
        let (call_id, output) = match item {
            ResponseItem::FunctionCallOutput {
                call_id: Some(call_id),
                output,
                ..
            }
            | ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => (call_id, output),
            _ => continue,
        };
        if let Some(success) = output.success {
            selection
                .meta
                .result_errors
                .insert(call_id.clone(), !success);
        }
    }
    selection
}

pub(crate) fn parse_tools(tools: &[Value]) -> RequestTools {
    let mut definitions = Vec::new();
    let mut custom = HashSet::new();
    let mut strict = HashMap::new();
    let mut hosted_tools = Vec::new();
    for tool in tools {
        if tool["type"] == "namespace" {
            if let (Some(namespace), Some(children)) =
                (tool["name"].as_str(), tool["tools"].as_array())
            {
                for child in children {
                    append_tool(
                        child,
                        Some(namespace),
                        &mut definitions,
                        &mut custom,
                        &mut strict,
                        &mut hosted_tools,
                    );
                }
            }
        } else {
            append_tool(
                tool,
                None,
                &mut definitions,
                &mut custom,
                &mut strict,
                &mut hosted_tools,
            );
        }
    }
    RequestTools {
        definitions,
        meta: ToolMeta {
            custom_names: custom,
            strict,
            result_errors: HashMap::new(),
            hosted_tools,
        },
    }
}

fn append_tool(
    tool: &Value,
    namespace: Option<&str>,
    definitions: &mut Vec<ToolDefinition>,
    custom: &mut HashSet<String>,
    strict: &mut HashMap<String, bool>,
    hosted_tools: &mut Vec<Value>,
) {
    // Hosted (server-side) tools carry no function schema and no `name`;
    // they never become function definitions. Keep the raw declaration for
    // per-protocol translation by `hosted_tools`.
    if !matches!(tool["type"].as_str(), Some("function") | Some("custom")) {
        hosted_tools.push(tool.clone());
        return;
    }
    let Some(name) = tool["name"].as_str() else {
        return;
    };
    let name = flat_name(name, namespace);
    let parameters = match tool["type"].as_str() {
        Some("custom") => {
            custom.insert(name.clone());
            json!({"type":"object", "properties":{"input":{"type":"string", "description":"The complete tool input text"}}, "required":["input"], "additionalProperties":false})
        }
        Some("function") => tool
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object", "properties":{}})),
        // Unreachable: hosted tool types returned above.
        _ => return,
    };
    if let Some(value) = tool["strict"].as_bool() {
        strict.insert(name.clone(), value);
    }
    // Custom tools carry their contract in the wrapper schema; a missing
    // description still needs to tell the model how to use the input field.
    let fallback_description = if tool["type"].as_str() == Some("custom") {
        "Custom tool; pass the complete tool input text in the `input` field."
    } else {
        ""
    };
    definitions.push(ToolDefinition {
        name,
        description: tool["description"]
            .as_str()
            .filter(|description| !description.is_empty())
            .unwrap_or(fallback_description)
            .to_string(),
        parameters,
    });
}
