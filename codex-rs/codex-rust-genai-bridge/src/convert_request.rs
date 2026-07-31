use codex_api::ResponsesApiRequest;
use codex_protocol::models::{ContentItem, FunctionCallOutputBody, ResponseItem};
use codex_tools::ToolName;
use codex_tools::code_mode_name_for_tool_name;
use genai::chat::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, MessageContent, Tool, ToolCall, ToolResponse,
};
use serde_json::Value;

/// Converts a Codex `ResponsesApiRequest` into a rust-genai `ChatRequest`.
///
/// Returns `None` if the input contains no convertible messages (e.g. only
/// internal/local items).
pub fn responses_request_to_chat_request(request: &ResponsesApiRequest) -> Option<ChatRequest> {
    let system = if request.instructions.is_empty() {
        None
    } else {
        Some(request.instructions.clone())
    };

    let messages = convert_response_items(&request.input);

    if messages.is_empty() && system.is_none() {
        return None;
    }

    let mut chat_req = ChatRequest::new(messages);
    if let Some(sys) = system {
        chat_req = chat_req.with_system(sys);
    }

    // `request.tools` is `Option<ResponsesApiTools>` (opaque raw JSON). Extract
    // the tool array via the Serialize impl (as_raw_value is pub(crate)-gated).
    let mut tools: Vec<Value> = request
        .tools
        .as_ref()
        .and_then(|t| serde_json::to_value(t).ok())
        .and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok())
        .unwrap_or_default();
    // Responses-Lite (`use_responses_lite`) carries the tool list inside a
    // `ResponseItem::AdditionalTools` in `input` instead of `request.tools`.
    // Chat Completions needs those tools too, so merge them in here; parse_tools
    // then flattens any namespace specs exactly like the non-lite path.
    for item in &request.input {
        if let ResponseItem::AdditionalTools { tools: extra, .. } = item {
            tools.extend(extra.iter().cloned());
        }
    }
    let tools = parse_tools(&tools);
    if !tools.is_empty() {
        chat_req = chat_req.with_tools(tools);
    }

    if request.store {
        chat_req = chat_req.with_store(true);
    }

    Some(chat_req)
}

fn convert_response_items(items: &[ResponseItem]) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut pending_thought_signatures: Vec<String> = Vec::new();

    for item in items {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let role = map_role(role);
                let parts = convert_content_items(content);
                let msg = ChatMessage::new(role, MessageContent::from_parts(parts));
                messages.push(msg);
            }
            ResponseItem::Reasoning {
                encrypted_content, ..
            } => {
                if let Some(ec) = encrypted_content {
                    pending_thought_signatures.push(ec.clone());
                    // Also inject as ReasoningContent into the last assistant
                    // message so genai echoes it back via the reasoning_content
                    // field — DeepSeek requires this on subsequent requests.
                    if let Some(last_msg) = messages.last_mut()
                        && last_msg.role == ChatRole::Assistant
                    {
                        last_msg
                            .content
                            .push(ContentPart::ReasoningContent(ec.clone()));
                    }
                }
            }
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                let fn_arguments: Value =
                    serde_json::from_str(arguments).unwrap_or(Value::String(arguments.clone()));
                let mut tool_call = ToolCall {
                    call_id: call_id.clone(),
                    fn_name: name.clone(),
                    fn_arguments,
                    thought_signatures: None,
                };
                // Attach pending thought signatures to the first tool call
                if !pending_thought_signatures.is_empty() {
                    tool_call.thought_signatures =
                        Some(std::mem::take(&mut pending_thought_signatures));
                }
                let tc_part = ContentPart::ToolCall(tool_call);
                // If the last message is already an Assistant, append the tool
                // call to it. This avoids creating consecutive Assistant messages
                // where only one carries reasoning_content — providers like
                // DeepSeek require reasoning_content on every assistant message
                // when thinking mode is active.
                if let Some(last_msg) = messages.last_mut()
                    && last_msg.role == ChatRole::Assistant
                {
                    last_msg.content.push(tc_part);
                } else {
                    messages.push(ChatMessage::assistant(MessageContent::from(tc_part)));
                }
            }
            ResponseItem::CustomToolCall {
                name,
                input,
                call_id,
                ..
            } => {
                let fn_arguments: Value =
                    serde_json::from_str(input).unwrap_or(Value::String(input.clone()));
                let tool_call = ToolCall {
                    call_id: call_id.clone(),
                    fn_name: name.clone(),
                    fn_arguments,
                    thought_signatures: None,
                };
                let tc_part = ContentPart::ToolCall(tool_call);
                if let Some(last_msg) = messages.last_mut()
                    && last_msg.role == ChatRole::Assistant
                {
                    last_msg.content.push(tc_part);
                } else {
                    messages.push(ChatMessage::assistant(MessageContent::from(tc_part)));
                }
            }
            ResponseItem::FunctionCallOutput { call_id, output, .. } => {
                let content = match &output.body {
                    FunctionCallOutputBody::Text(text) => text.clone(),
                    FunctionCallOutputBody::ContentItems(items) => {
                        serde_json::to_string(items).unwrap_or_default()
                    }
                };
                messages.push(ChatMessage::tool(MessageContent::from(
                    ContentPart::ToolResponse(ToolResponse::new(call_id, content)),
                )));
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                let content = match &output.body {
                    FunctionCallOutputBody::Text(text) => text.clone(),
                    FunctionCallOutputBody::ContentItems(items) => {
                        serde_json::to_string(items).unwrap_or_default()
                    }
                };
                messages.push(ChatMessage::tool(MessageContent::from(
                    ContentPart::ToolResponse(ToolResponse::new(call_id, content)),
                )));
            }
            // Internal/local events — not sent to the model.
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::AgentMessage { .. } => {
                // Skip — these items are internal to Codex.
            }
            ResponseItem::Other => {
                tracing::warn!("Skipping unknown ResponseItem::Other in request conversion");
            }
        }
    }

    messages
}

fn map_role(codex_role: &str) -> ChatRole {
    match codex_role {
        "user" => ChatRole::User,
        "assistant" => ChatRole::Assistant,
        "developer" | "system" => ChatRole::System,
        "tool" => ChatRole::Tool,
        other => {
            tracing::warn!(
                role = %other,
                "Unknown response item role, defaulting to User"
            );
            ChatRole::User
        }
    }
}

fn convert_content_items(items: &[ContentItem]) -> Vec<ContentPart> {
    items
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Some(ContentPart::Text(text.clone()))
            }
            ContentItem::InputImage { image_url, .. } => {
                // Infer content type from URL extension or default to image/png
                let content_type = if image_url.ends_with(".jpg") || image_url.ends_with(".jpeg") {
                    "image/jpeg"
                } else if image_url.ends_with(".webp") {
                    "image/webp"
                } else if image_url.ends_with(".gif") {
                    "image/gif"
                } else {
                    "image/png"
                };
                Some(ContentPart::Binary(genai::chat::Binary::from_url(
                    content_type,
                    image_url,
                    None,
                )))
            }
            ContentItem::InputAudio { .. } => {
                // 国内 LLM 适配暂不支持音频输入，跳过。
                tracing::warn!("Skipping ContentItem::InputAudio (unsupported by genai bridge)");
                None
            }
        })
        .collect()
}

fn parse_tools(tools: &[Value]) -> Vec<Tool> {
    let mut parsed = Vec::new();
    for v in tools {
        match v.get("type").and_then(|t| t.as_str()) {
            // Responses API `namespace` tools group several function tools
            // (e.g. all tools from one MCP server). Chat Completions has no
            // namespace concept, so flatten each child function into a
            // standalone tool whose name follows the conventional
            // `mcp__<server>__<tool>` form that the model emits and the
            // registry's flat-name index recognizes.
            Some("namespace") => {
                let namespace = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let Some(children) = v.get("tools").and_then(|t| t.as_array()) else {
                    continue;
                };
                for child in children {
                    let Some(child_name) = child.get("name").and_then(|n| n.as_str()) else {
                        continue;
                    };
                    let flat_name = code_mode_name_for_tool_name(&ToolName::namespaced(
                        namespace,
                        child_name,
                    ));
                    parsed.push(tool_from_value(child, flat_name));
                }
            }
            // Top-level function tools (and any tool without a recognized
            // `type`) pass through with their own declared name.
            _ => {
                let Some(name) = v.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                parsed.push(tool_from_value(v, name.to_string()));
            }
        }
    }
    parsed
}

fn tool_from_value(v: &Value, name: String) -> Tool {
    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let schema = v.get("parameters").or(v.get("input_schema")).cloned();
    Tool {
        name: name.into(),
        description,
        schema,
        strict: v.get("strict").and_then(|s| s.as_bool()),
        config: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::ContentItem;
    use codex_protocol::ResponseItemId;

    #[test]
    fn test_single_user_message() {
        let request = ResponsesApiRequest {
            model: "gpt-5".into(),
            instructions: String::new(),
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".into(),
                content: vec![ContentItem::InputText {
                    text: "Hello".into(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request).unwrap();
        assert_eq!(result.messages.len(), 1);
        let msg = &result.messages[0];
        assert_eq!(msg.role, ChatRole::User);
        assert_eq!(msg.content.parts().len(), 1);
    }

    #[test]
    fn test_system_instructions() {
        let request = ResponsesApiRequest {
            model: "gpt-5".into(),
            instructions: "You are helpful.".into(),
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".into(),
                content: vec![ContentItem::InputText { text: "Hi".into() }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request).unwrap();
        assert_eq!(result.system, Some("You are helpful.".into()));
    }

    #[test]
    fn test_function_call_and_output() {
        let request = ResponsesApiRequest {
            model: "gpt-5".into(),
            instructions: String::new(),
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "get_weather".into(),
                    namespace: None,
                    arguments: r#"{"city":"SF"}"#.into(),
                    encrypted_function_args: None,
                    call_id: "call_1".into(),
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::FunctionCallOutput {
                    id: None,
                    call_id: "call_1".into(),
                    output: codex_protocol::models::FunctionCallOutputPayload {
                        body: FunctionCallOutputBody::Text("Sunny".into()),
                        success: Some(true),
                    },
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request).unwrap();
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.messages[0].role, ChatRole::Assistant);
        assert_eq!(result.messages[1].role, ChatRole::Tool);
    }

    #[test]
    fn test_empty_input() {
        let request = ResponsesApiRequest {
            model: "gpt-5".into(),
            instructions: String::new(),
            input: vec![],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        assert!(responses_request_to_chat_request(&request).is_none());
    }

    /// Simulates the second turn of a conversation where the first turn
    /// produced both a text response and reasoning content (e.g. DeepSeek thinking mode).
    /// The Reasoning item MUST be injected as ContentPart::ReasoningContent into
    /// the last assistant message so the provider gets its reasoning echoed back.
    #[test]
    fn test_reasoning_injected_into_assistant_message() {
        let request = ResponsesApiRequest {
            model: "deepseek-v4-flash".into(),
            instructions: "You are helpful.".into(),
            input: vec![
                // First turn: user asks something
                ResponseItem::Message {
                    id: None,
                    role: "user".into(),
                    content: vec![ContentItem::InputText {
                        text: "Hello".into(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                // First turn: assistant responds with text
                ResponseItem::Message {
                    id: None,
                    role: "assistant".into(),
                    content: vec![ContentItem::OutputText {
                        text: "Hi there!".into(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                // First turn: reasoning content from the assistant's thinking
                ResponseItem::Reasoning {
                    id: Some(ResponseItemId::from_server("rsn_1".to_string())),
                    summary: vec![],
                    content: None,
                    encrypted_content: Some("Let me think about this...".into()),
                    internal_chat_message_metadata_passthrough: None,
                },
                // Second turn: user follows up
                ResponseItem::Message {
                    id: None,
                    role: "user".into(),
                    content: vec![ContentItem::InputText {
                        text: "What was my first question?".into(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request).unwrap();
        assert_eq!(result.system, Some("You are helpful.".into()));
        // Messages: user(Hello), assistant(+ReasoningContent), user(What was...)
        assert_eq!(result.messages.len(), 3);

        // The assistant message should have both text and reasoning content
        let assistant_msg = &result.messages[1];
        assert_eq!(assistant_msg.role, ChatRole::Assistant);
        let parts = assistant_msg.content.parts();
        assert_eq!(
            parts.len(),
            2,
            "assistant message should have text + reasoning content"
        );
        assert!(
            matches!(&parts[0], ContentPart::Text(t) if t == "Hi there!"),
            "first part should be text"
        );
        assert!(
            matches!(&parts[1], ContentPart::ReasoningContent(r) if r == "Let me think about this..."),
            "second part should be ReasoningContent"
        );
    }

    /// Reasoning that arrives BEFORE the assistant Message item (unusual order)
    /// should NOT cause a crash — it's silently dropped since there's no
    /// preceding Assistant message to attach to.
    #[test]
    fn test_reasoning_before_assistant_message_is_benign() {
        let request = ResponsesApiRequest {
            model: "deepseek-v4-flash".into(),
            instructions: String::new(),
            input: vec![
                // Reasoning BEFORE assistant message (should be skipped)
                ResponseItem::Reasoning {
                    id: Some(ResponseItemId::from_server("rsn_orphan".to_string())),
                    summary: vec![],
                    content: None,
                    encrypted_content: Some("orphan reasoning".into()),
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".into(),
                    content: vec![ContentItem::OutputText {
                        text: "response".into(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request).unwrap();
        assert_eq!(result.messages.len(), 1);
        let msg = &result.messages[0];
        assert_eq!(msg.role, ChatRole::Assistant);
        // Only text, no ReasoningContent since it arrived before the message
        assert_eq!(msg.content.parts().len(), 1);
    }

    /// Full round-trip test simulating a real DeepSeek turn with tool calls:
    /// User → Assistant(text+reasoning) + FunctionCall → Tool result → next request.
    /// The assistant message MUST have reasoning_content echoed back, and
    /// consecutive assistant items MUST be merged into one message.
    #[test]
    fn test_tool_call_merges_with_reasoning_assistant() {
        let request = ResponsesApiRequest {
            model: "deepseek-v4-flash".into(),
            instructions: "You are helpful.".into(),
            input: vec![
                // Turn 1: user asks
                ResponseItem::Message {
                    id: None,
                    role: "user".into(),
                    content: vec![ContentItem::InputText {
                        text: "List files".into(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                // Turn 1: assistant responds (empty text — typical when model calls tool immediately)
                ResponseItem::Message {
                    id: None,
                    role: "assistant".into(),
                    content: vec![],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                // Turn 1: reasoning content (from DeepSeek thinking mode)
                ResponseItem::Reasoning {
                    id: Some(ResponseItemId::from_server("rsn_1".to_string())),
                    summary: vec![],
                    content: None,
                    encrypted_content: Some("I should list the files to help the user.".into()),
                    internal_chat_message_metadata_passthrough: None,
                },
                // Turn 1: tool call (model decided to run ls)
                ResponseItem::FunctionCall {
                    id: None,
                    name: "exec_command".into(),
                    namespace: None,
                    arguments: r#"{"cmd":"ls"}"#.into(),
                    encrypted_function_args: None,
                    call_id: "call_1".into(),
                    internal_chat_message_metadata_passthrough: None,
                },
                // Turn 1: tool result
                ResponseItem::FunctionCallOutput {
                    id: None,
                    call_id: "call_1".into(),
                    output: codex_protocol::models::FunctionCallOutputPayload {
                        body: FunctionCallOutputBody::Text("file1.txt\nfile2.txt".into()),
                        success: Some(true),
                    },
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request).unwrap();
        // Messages: user, assistant(merged: text+reasoning+tool_call), tool
        assert_eq!(
            result.messages.len(),
            3,
            "should merge text assistant + tool call into one message"
        );

        // The assistant message (index 1) must have: text (empty), reasoning, AND tool call
        let assistant = &result.messages[1];
        assert_eq!(assistant.role, ChatRole::Assistant);
        let parts = assistant.content.parts();
        assert_eq!(parts.len(), 2, "should have reasoning + tool_call parts");
        assert!(
            matches!(&parts[0], ContentPart::ReasoningContent(r) if r == "I should list the files to help the user."),
            "first part should be ReasoningContent (injected by Reasoning item)"
        );
        assert!(
            matches!(&parts[1], ContentPart::ToolCall(_)),
            "second part should be ToolCall (merged from FunctionCall item)"
        );

        // Tool message
        let tool = &result.messages[2];
        assert_eq!(tool.role, ChatRole::Tool);
    }

    /// If there's no preceding assistant message at all, the reasoning
    /// is simply skipped (encrypted_content is still collected for
    /// thought_signatures but no ReasoningContent part is emitted).
    #[test]
    fn test_reasoning_without_assistant_message_is_skipped() {
        let request = ResponsesApiRequest {
            model: "deepseek-v4-flash".into(),
            instructions: String::new(),
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".into(),
                    content: vec![ContentItem::InputText {
                        text: "Hello".into(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                // Reasoning without a preceding assistant message
                ResponseItem::Reasoning {
                    id: Some(ResponseItemId::from_server("rsn_no_assistant".to_string())),
                    summary: vec![],
                    content: None,
                    encrypted_content: Some("thinking".into()),
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request).unwrap();
        assert_eq!(result.messages.len(), 1);
        let msg = &result.messages[0];
        assert_eq!(msg.role, ChatRole::User);
    }

    #[test]
    fn parse_tools_flattens_namespace_tools() {
        // A serialized `ToolSpec::Namespace` (e.g. one MCP server's tools).
        // Chat Completions has no namespace concept, so `parse_tools` must
        // flatten each child function into a standalone tool named with the
        // conventional `mcp__<server>__<tool>` form.
        let tools = vec![serde_json::json!({
            "type": "namespace",
            "name": "mcp__memory",
            "description": "Memory server tools",
            "tools": [
                {
                    "type": "function",
                    "name": "create_entities",
                    "description": "Create entities.",
                    "strict": false,
                    "parameters": {"type": "object", "properties": {}}
                },
                {
                    "type": "function",
                    "name": "search_nodes",
                    "description": "Search nodes.",
                    "strict": false,
                    "parameters": {"type": "object", "properties": {}}
                }
            ]
        })];

        let parsed = parse_tools(&tools);
        assert_eq!(parsed.len(), 2, "each child function should become one tool");

        let names: Vec<String> = parsed.iter().map(|t| t.name.to_string()).collect();
        assert_eq!(
            names,
            vec!["mcp__memory__create_entities", "mcp__memory__search_nodes"],
            "child names must be flattened with the conventional mcp__<server>__<tool> form"
        );

        // Schema and description must come from the child, not the namespace wrapper.
        assert_eq!(parsed[0].description.as_deref(), Some("Create entities."));
        assert!(
            parsed[0].schema.is_some(),
            "child parameters schema must be preserved"
        );
    }

    #[test]
    fn parse_tools_preserves_top_level_function_tools() {
        // A top-level (non-namespace) function tool must pass through unchanged.
        let tools = vec![serde_json::json!({
            "type": "function",
            "name": "shell",
            "description": "Run a shell command.",
            "strict": false,
            "parameters": {"type": "object"}
        })];

        let parsed = parse_tools(&tools);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name.to_string(), "shell");
        assert_eq!(parsed[0].description.as_deref(), Some("Run a shell command."));
    }

    #[test]
    fn responses_lite_additional_tools_are_flattened() {
        // Responses-Lite (`use_responses_lite`) carries the tool list inside a
        // `ResponseItem::AdditionalTools` in `input` with `request.tools = None`.
        // The Chat-Completions bridge must still surface those tools, flattening
        // namespace specs exactly like the non-lite path.
        let request = ResponsesApiRequest {
            model: "gpt-5".into(),
            instructions: String::new(),
            input: vec![
                ResponseItem::AdditionalTools {
                    id: None,
                    role: "developer".to_string(),
                    tools: vec![serde_json::json!({
                        "type": "namespace",
                        "name": "mcp__memory",
                        "description": "Memory server tools",
                        "tools": [{
                            "type": "function",
                            "name": "create_entities",
                            "description": "Create entities.",
                            "strict": false,
                            "parameters": {"type": "object", "properties": {}}
                        }]
                    })],
                },
                ResponseItem::Message {
                    id: None,
                    role: "user".into(),
                    content: vec![ContentItem::InputText { text: "hi".into() }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: false,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        };

        let result = responses_request_to_chat_request(&request)
            .expect("lite request with a user message should produce a chat request");
        let tools = result
            .tools
            .expect("AdditionalTools should be surfaced as chat request tools");
        assert_eq!(
            tools.len(),
            1,
            "the namespace spec should flatten to one tool"
        );
        assert_eq!(tools[0].name.to_string(), "mcp__memory__create_entities");
        assert_eq!(tools[0].description.as_deref(), Some("Create entities."));
    }
}
