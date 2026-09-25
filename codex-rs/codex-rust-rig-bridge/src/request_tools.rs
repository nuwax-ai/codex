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
    pub(crate) custom_names: HashSet<String>,
    pub(crate) strict: HashMap<String, bool>,
}

pub(crate) fn flat_name(name: &str, namespace: Option<&str>) -> String {
    match namespace {
        Some(namespace) => code_mode_name_for_tool_name(&ToolName::namespaced(namespace, name)),
        None => name.to_string(),
    }
}

pub(crate) fn request_tools(request: &ResponsesApiRequest) -> RequestTools {
    let mut tools: Vec<Value> = request
        .tools
        .as_ref()
        .and_then(|tools| serde_json::to_value(tools).ok())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    for item in &request.input {
        if let ResponseItem::AdditionalTools { tools: extra, .. } = item {
            tools.extend(extra.iter().cloned());
        }
    }
    parse_tools(&tools)
}

pub(crate) fn parse_tools(tools: &[Value]) -> RequestTools {
    let mut definitions = Vec::new();
    let mut custom = HashSet::new();
    let mut strict = HashMap::new();
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
                    );
                }
            }
        } else {
            append_tool(tool, None, &mut definitions, &mut custom, &mut strict);
        }
    }
    RequestTools {
        definitions,
        custom_names: custom,
        strict,
    }
}

fn append_tool(
    tool: &Value,
    namespace: Option<&str>,
    definitions: &mut Vec<ToolDefinition>,
    custom: &mut HashSet<String>,
    strict: &mut HashMap<String, bool>,
) {
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
        other => {
            tracing::warn!(tool_type = ?other, "Dropping hosted tool with no chat protocol equivalent");
            return;
        }
    };
    if let Some(value) = tool["strict"].as_bool() {
        strict.insert(name.clone(), value);
    }
    definitions.push(ToolDefinition {
        name,
        description: tool["description"].as_str().unwrap_or_default().to_string(),
        parameters,
    });
}
