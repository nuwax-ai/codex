use super::*;
#[cfg(test)]
mod mapping {
    use super::*;

    use crate::request_tools::parse_tools;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::openai_models::ReasoningEffort;
    use pretty_assertions::assert_eq;
    use rig_core::completion::message::AssistantContent;
    use rig_core::completion::message::ToolResultContent;
    use rig_core::completion::message::UserContent;
    fn responses_request_to_completion_request(
        request: &ResponsesApiRequest,
    ) -> Option<(CompletionRequest, std::collections::HashSet<String>)> {
        super::responses_request_to_completion_request(request, RigProtocol::Chat, "test").ok()
    }

    fn base_request(input: Vec<ResponseItem>) -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "test-model".into(),
            instructions: "be helpful".into(),
            input,
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
            access_programs: None,
        }
    }

    fn user_text(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn assistant_text(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "assistant".into(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn reasoning_item(content: &str) -> ResponseItem {
        ResponseItem::Reasoning {
            id: None,
            summary: vec![],
            content: Some(vec![
                codex_protocol::models::ReasoningItemContent::ReasoningText {
                    text: content.to_string(),
                },
            ]),
            encrypted_content: Some(content.to_string()),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn function_call(call_id: &str, name: &str, arguments: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.into(),
            namespace: None,
            arguments: arguments.into(),
            encrypted_function_args: None,
            call_id: call_id.into(),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn tool_output(call_id: &str, output: &str) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some(call_id.into()),
            name: None,
            namespace: None,
            output: codex_protocol::models::FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(output.into()),
                success: Some(true),
            },
            internal_chat_message_metadata_passthrough: None,
        }
    }

    #[test]
    fn empty_input_yields_none() {
        assert!(responses_request_to_completion_request(&base_request(vec![])).is_none());
    }

    #[test]
    fn system_prompt_becomes_leading_system_message() {
        let (req, _) =
            responses_request_to_completion_request(&base_request(vec![user_text("hi")]))
                .expect("convertible");
        assert_eq!(req.chat_history.len(), 2);
        assert!(matches!(
            req.chat_history.first(),
            Some(Message::System { content }) if content == "be helpful"
        ));
    }

    #[test]
    fn reasoning_echoes_into_previous_assistant_message() {
        let (req, _) = responses_request_to_completion_request(&base_request(vec![
            assistant_text("answer"),
            reasoning_item("thinking..."),
        ]))
        .expect("convertible");
        // system + one assistant message carrying text AND reasoning
        assert_eq!(req.chat_history.len(), 2);
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant") else {
            panic!("expected assistant message");
        };
        assert_eq!(content.len(), 2);
        assert!(matches!(content[0], AssistantContent::Text(_)));
        assert!(matches!(content[1], AssistantContent::Reasoning(_)));
    }

    #[test]
    fn function_call_merges_into_last_assistant_message() {
        let (req, _) = responses_request_to_completion_request(&base_request(vec![
            user_text("weather?"),
            assistant_text("let me check"),
            function_call("call_1", "get_weather", r#"{"city":"北京"}"#),
        ]))
        .expect("convertible");
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant") else {
            panic!("expected assistant message");
        };
        // text + tool call in one assistant message (Chat Completions shape)
        assert_eq!(content.len(), 2);
        assert!(matches!(content[1], AssistantContent::ToolCall(_)));
    }

    #[test]
    fn tool_result_resolves_name_from_call_id_and_lands_in_user_message() {
        let (req, _) = responses_request_to_completion_request(&base_request(vec![
            function_call("call_1", "get_weather", r#"{"city":"北京"}"#),
            tool_output("call_1", "sunny"),
        ]))
        .expect("convertible");
        let Message::User { content } = req.chat_history.last().expect("user") else {
            panic!("expected user message");
        };
        let UserContent::ToolResult(result) = content.last().expect("tool result") else {
            panic!("expected tool result");
        };
        assert_eq!(result.name, "get_weather");
        assert!(matches!(
            result.content.first(),
            Some(ToolResultContent::Text(_))
        ));
    }

    #[test]
    fn namespace_tools_flatten_to_mcp_names() {
        let tools_json = r#"[{
            "type": "namespace",
            "name": "mcp__memory",
            "tools": [
                {"type": "function", "name": "create_entities", "description": "d",
                 "parameters": {"type": "object"}}
            ]
        }]"#;
        let tools: Vec<Value> = serde_json::from_str(tools_json).expect("json");
        let selection = parse_tools(&tools);
        let (parsed, custom) = (selection.definitions, selection.custom_names);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "mcp__memory__create_entities");
        assert!(custom.is_empty());
    }

    #[test]
    fn hosted_tools_are_dropped() {
        let tools_json = r#"[
            {"type": "web_search"},
            {"type": "function", "name": "f", "description": "", "parameters": {}}
        ]"#;
        let tools: Vec<Value> = serde_json::from_str(tools_json).expect("json");
        let parsed = parse_tools(&tools).definitions;
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "f");
    }

    #[test]
    fn tool_choice_maps_known_values() {
        use rig_core::completion::message::ToolChoice;
        assert!(matches!(map_tool_choice("auto"), Some(ToolChoice::Auto)));
        assert!(matches!(map_tool_choice("none"), Some(ToolChoice::None)));
        assert!(matches!(
            map_tool_choice("required"),
            Some(ToolChoice::Required)
        ));
        assert!(matches!(
            map_tool_choice("get_weather"),
            Some(ToolChoice::Specific { .. })
        ));
    }

    #[test]
    fn high_reasoning_efforts_pass_through_unclamped() {
        let mut request = base_request(vec![user_text("hi")]);
        request.reasoning = Some(codex_api::Reasoning {
            effort: Some(ReasoningEffort::XHigh),
            summary: None,
            context: None,
        });
        let (req, _) = responses_request_to_completion_request(&request).expect("convertible");
        let params = req.additional_params.expect("params present");
        assert_eq!(params["reasoning_effort"], "xhigh");
    }
}

#[cfg(test)]
mod text_format_tests {
    use super::*;

    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseItem;

    use pretty_assertions::assert_eq;
    fn responses_request_to_completion_request(
        request: &ResponsesApiRequest,
    ) -> Option<(CompletionRequest, std::collections::HashSet<String>)> {
        super::responses_request_to_completion_request(request, RigProtocol::Chat, "test").ok()
    }

    #[test]
    fn text_format_maps_to_output_schema() {
        let mut request = base_request(vec![user_text_fixture("hi")]);
        request.text = Some(codex_api::TextControls {
            verbosity: None,
            format: Some(codex_api::TextFormat {
                r#type: codex_api::TextFormatType::JsonSchema,
                strict: false,
                schema: serde_json::json!({"type": "object", "properties": {"answer": {"type": "string"}}}),
                name: "answer_shape".into(),
            }),
        });
        let (req, _) = responses_request_to_completion_request(&request).expect("convertible");
        let params = req.additional_params.expect("output format mapped");
        assert_eq!(
            params["response_format"]["json_schema"],
            serde_json::json!({
                "name": "answer_shape", "strict": false,
                "schema": {"type": "object", "properties": {"answer": {"type": "string"}}}
            })
        );
    }

    #[test]
    fn verbosity_maps_to_additional_params() {
        let mut request = base_request(vec![user_text_fixture("hi")]);
        request.text = Some(codex_api::TextControls {
            verbosity: Some(codex_api::OpenAiVerbosity::Low),
            format: None,
        });
        let (req, _) = responses_request_to_completion_request(&request).expect("convertible");
        assert_eq!(req.additional_params.expect("params")["verbosity"], "low");
    }

    fn base_request(input: Vec<ResponseItem>) -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "m".into(),
            instructions: String::new(),
            input,
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
            access_programs: None,
        }
    }

    fn user_text_fixture(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }
}

#[cfg(test)]
mod review_fix_tests {
    use super::*;

    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::ImageReference;
    use codex_protocol::models::ResponseItem;

    use pretty_assertions::assert_eq;
    use rig_core::completion::message::AssistantContent;
    use rig_core::completion::message::DocumentSourceKind;
    use rig_core::completion::message::UserContent;
    fn responses_request_to_completion_request(
        request: &ResponsesApiRequest,
    ) -> Option<(CompletionRequest, std::collections::HashSet<String>)> {
        super::responses_request_to_completion_request(request, RigProtocol::Chat, "test").ok()
    }

    fn base_request(input: Vec<ResponseItem>) -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "m".into(),
            instructions: String::new(),
            input,
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
            access_programs: None,
        }
    }

    fn user_text(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn assistant_text(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "assistant".into(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn reasoning(content: &str) -> ResponseItem {
        ResponseItem::Reasoning {
            id: None,
            summary: vec![],
            content: Some(vec![
                codex_protocol::models::ReasoningItemContent::ReasoningText {
                    text: content.to_string(),
                },
            ]),
            encrypted_content: Some(content.to_string()),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn function_call(call_id: &str, name: &str, arguments: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.into(),
            namespace: None,
            arguments: arguments.into(),
            encrypted_function_args: None,
            call_id: call_id.into(),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    /// Real history order is Reasoning → Message → FunctionCall; all three
    /// MUST merge into a single assistant message so the reasoning replay
    /// survives the OpenAI wire (a reasoning-only assistant message ahead
    /// of the text gets dropped).
    #[test]
    fn real_history_order_merges_into_one_assistant_message() {
        let (req, _) = responses_request_to_completion_request(&base_request(vec![
            user_text("weather?"),
            reasoning("thinking about it"),
            assistant_text("let me check"),
            function_call("c1", "get_weather", r#"{"city":"北京"}"#),
        ]))
        .expect("convertible");
        let assistant_count = req
            .chat_history
            .iter()
            .filter(|m| matches!(m, Message::Assistant { .. }))
            .count();
        assert_eq!(
            assistant_count, 1,
            "reasoning+text+tool call must be ONE assistant message"
        );
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant") else {
            panic!()
        };
        // reasoning + text + tool call, in order
        assert!(matches!(content[0], AssistantContent::Reasoning(_)));
        assert!(matches!(content[1], AssistantContent::Text(_)));
        assert!(matches!(content[2], AssistantContent::ToolCall(_)));
    }

    /// AgentMessage plaintext reaches the model as assistant text.
    #[test]
    fn agent_message_forwards_plaintext() {
        let (req, _) = responses_request_to_completion_request(&base_request(vec![
            ResponseItem::AgentMessage {
                id: None,
                author: "agent".into(),
                recipient: "main".into(),
                content: vec![
                    codex_protocol::models::AgentMessageInputContent::InputText {
                        text: "task complete".into(),
                    },
                    codex_protocol::models::AgentMessageInputContent::EncryptedContent {
                        encrypted_content: "opaque".into(),
                    },
                ],
                internal_chat_message_metadata_passthrough: None,
            },
        ]))
        .expect("convertible");
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant") else {
            panic!("expected assistant message");
        };
        assert!(content.iter().any(|part| matches!(
            part,
            AssistantContent::Text(t) if t.text.contains("task complete")
        )));
    }

    /// Tool-result images become typed image blocks, never base64 text.
    #[test]
    fn tool_result_data_url_image_becomes_image_block() {
        let (req, _) = responses_request_to_completion_request(&base_request(vec![
            function_call("c1", "view_image", "{}"),
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: Some("c1".into()),
                name: None,
                namespace: None,
                output: codex_protocol::models::FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::ContentItems(vec![
                        codex_protocol::models::FunctionCallOutputContentItem::InputImage {
                            image: ImageReference::Inline {
                                image_url: "data:image/png;base64,aGVsbG8=".into(),
                            },
                            detail: None,
                        },
                        codex_protocol::models::FunctionCallOutputContentItem::InputText {
                            text: "screenshot".into(),
                        },
                    ]),
                    success: Some(true),
                },
                internal_chat_message_metadata_passthrough: None,
            },
        ]))
        .expect("convertible");
        let Message::User { content } = req.chat_history.last().expect("user") else {
            panic!()
        };
        assert!(matches!(content.first(), Some(UserContent::ToolResult(_))));
        assert!(content.iter().any(|part| matches!(part,
            UserContent::Image(img) if matches!(img.data, DocumentSourceKind::Base64(_))
        )));
        let encoded = serde_json::to_value(&req.chat_history).expect("history");
        assert!(!encoded.to_string().contains("\\\"image_url\\\""));
    }

    /// Input data-URL images decode into base64 sources (Anthropic wire
    /// would drop them as remote links).
    #[test]
    fn input_data_url_image_decodes_to_base64() {
        let (req, _) =
            responses_request_to_completion_request(&base_request(vec![ResponseItem::Message {
                id: None,
                role: "user".into(),
                content: vec![ContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/jpeg;base64,QUJD".into(),
                    },
                    detail: None,
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }]))
            .expect("convertible");
        let Message::User { content } = req.chat_history.last().expect("user") else {
            panic!()
        };
        assert!(matches!(
            content[0],
            UserContent::Image(ref img)
                if matches!(img.data, DocumentSourceKind::Base64(_))
        ));
    }
}

#[cfg(test)]
mod custom_tool_tests {
    use super::*;

    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseItem;

    use pretty_assertions::assert_eq;
    fn responses_request_to_completion_request(
        request: &ResponsesApiRequest,
    ) -> Option<(CompletionRequest, std::collections::HashSet<String>)> {
        super::responses_request_to_completion_request(request, RigProtocol::Chat, "test").ok()
    }

    #[test]
    fn custom_tool_gets_input_wrapper_schema_and_round_trips() {
        let mut request = ResponsesApiRequest {
            model: "m".into(),
            instructions: String::new(),
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".into(),
                content: vec![ContentItem::InputText {
                    text: "patch it".into(),
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
            access_programs: None,
        };
        let tools_json =
            r#"[{"type":"custom","name":"apply_patch","description":"Apply a patch"}]"#;
        let raw = serde_json::value::RawValue::from_string(tools_json.to_string()).expect("json");
        request.tools = Some(codex_api::ResponsesApiTools::from(std::sync::Arc::from(
            raw,
        )));

        let (req, custom) = responses_request_to_completion_request(&request).expect("ok");
        assert!(custom.contains("apply_patch"));
        assert_eq!(req.tools.len(), 1);
        // wrapper schema: input is a required string
        let schema = &req.tools[0].parameters;
        assert_eq!(schema["properties"]["input"]["type"], "string");
        assert_eq!(schema["required"][0], "input");
    }
}
