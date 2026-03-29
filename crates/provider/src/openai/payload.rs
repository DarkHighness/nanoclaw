use crate::{
    ProviderError, RequestOptions, Result, data_url, merge_top_level_object,
    render_instruction_text, stringify_json, tool_result_roundtrip_text, tool_schema,
};
use serde_json::{Map, Value, json};
use types::{
    Message, MessagePart, MessageRole, ModelRequest, ProviderContinuation, ReasoningContent,
};

fn openai_instruction_text(request: &ModelRequest) -> Option<String> {
    let mut frames = request.instructions.clone();
    frames.extend(request.additional_context.iter().cloned());
    render_instruction_text(&frames)
}

pub(crate) fn build_openai_realtime_request_event(
    model: String,
    request: ModelRequest,
    request_options: &RequestOptions,
) -> Result<Value> {
    let instructions = openai_instruction_text(&request);
    let mut response = Map::new();
    response.insert("model".to_string(), Value::String(model));
    response.insert(
        "modalities".to_string(),
        Value::Array(vec![Value::String("text".to_string())]),
    );
    if let Some(instructions) = instructions {
        response.insert("instructions".to_string(), Value::String(instructions));
    }
    if let Some(temperature) = request_options.temperature {
        response.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = request_options.max_tokens {
        response.insert("max_output_tokens".to_string(), json!(max_tokens));
    }
    if !request.tools.is_empty() {
        response.insert(
            "tools".to_string(),
            Value::Array(request.tools.iter().map(tool_schema).collect()),
        );
    }
    let input = serialize_openai_input_items(&request.messages)?;
    if !input.is_empty() {
        response.insert("input".to_string(), Value::Array(input));
    }
    merge_top_level_object(
        &mut response,
        request_options.additional_params.as_ref(),
        "OpenAI",
    )?;

    Ok(json!({
        "type": "response.create",
        "response": response,
    }))
}

pub(crate) fn build_openai_responses_body(
    model: String,
    request: ModelRequest,
    request_options: &RequestOptions,
) -> Result<Value> {
    let instructions = openai_instruction_text(&request);
    let mut object = Map::new();
    object.insert("model".to_string(), Value::String(model));
    object.insert("stream".to_string(), Value::Bool(true));
    if let Some(instructions) = instructions {
        // Responses treats `instructions` as a request-level prompt frame.
        // Keeping both stable instructions and injected context top-level
        // avoids duplicating them into the conversation chain when
        // `previous_response_id` is active.
        object.insert("instructions".to_string(), Value::String(instructions));
    }
    if let Some(temperature) = request_options.temperature {
        object.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = request_options.max_tokens {
        object.insert("max_output_tokens".to_string(), json!(max_tokens));
    }
    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                let mut schema = tool_schema(tool);
                if let Some(object) = schema.as_object_mut() {
                    // Responses defaults function tools to strict mode. Our shared
                    // tool schemas target general JSON Schema compatibility across
                    // providers, including optional fields and flatten-generated
                    // compositions that OpenAI strict mode rejects. Opt out here
                    // unless we add a dedicated OpenAI strict-schema compiler.
                    object.insert("strict".to_string(), Value::Bool(false));
                }
                schema
            })
            .collect::<Vec<_>>();
        object.insert("tools".to_string(), Value::Array(tools));
    }
    let input = serialize_openai_input_items(&request.messages)?;
    if !input.is_empty() {
        object.insert("input".to_string(), Value::Array(input));
    }
    merge_top_level_object(
        &mut object,
        request_options.additional_params.as_ref(),
        "OpenAI",
    )?;

    if let Some(prompt_cache_key) = &request_options.prompt_cache_key {
        object.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.clone()),
        );
    }
    if let Some(prompt_cache_retention) = request_options.prompt_cache_retention {
        object.insert(
            "prompt_cache_retention".to_string(),
            Value::String(prompt_cache_retention.as_api_value().to_string()),
        );
    }
    if let Some(options) = &request_options.openai_responses {
        if let Some(store) = options.store {
            object.insert("store".to_string(), Value::Bool(store));
        }
        if let Some(ProviderContinuation::OpenAiResponses { response_id }) = &request.continuation {
            if matches!(options.store, Some(false)) {
                return Err(ProviderError::config(
                    "OpenAI `previous_response_id` chaining requires stored responses; do not set store=false when chaining is enabled",
                ));
            }
            object.insert(
                "previous_response_id".to_string(),
                Value::String(response_id.to_string()),
            );
        }
        if let Some(server_compaction) = &options.server_compaction {
            object.insert(
                "context_management".to_string(),
                server_compaction.as_json(),
            );
        }
    }
    Ok(Value::Object(object))
}

fn serialize_openai_input_items(messages: &[Message]) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System | MessageRole::User => {
                let mut content = Vec::new();
                let mut trailing_items = Vec::new();
                for part in &message.parts {
                    if let Some(block) = openai_user_message_block(part) {
                        content.push(block);
                    } else if let Some(item) =
                        openai_input_item_from_part(part, message.role.clone())
                    {
                        trailing_items.push(item);
                    }
                }
                if !content.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": match message.role {
                            MessageRole::System => "system",
                            _ => "user",
                        },
                        "content": content,
                    }));
                }
                items.extend(trailing_items);
            }
            MessageRole::Assistant => {
                let mut content = Vec::new();
                let mut standalone_items = Vec::new();
                for part in &message.parts {
                    if let Some(block) = openai_assistant_message_block(part) {
                        content.push(block);
                    } else if let Some(item) =
                        openai_input_item_from_part(part, message.role.clone())
                    {
                        standalone_items.push(item);
                    }
                }
                if !content.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": content,
                    }));
                }
                items.extend(standalone_items);
            }
            MessageRole::Tool => {
                items.extend(
                    message
                        .parts
                        .iter()
                        .filter_map(|part| openai_input_item_from_part(part, message.role.clone())),
                );
            }
        }
    }
    Ok(items)
}

fn openai_user_message_block(part: &MessagePart) -> Option<Value> {
    match part {
        MessagePart::Text { text } => Some(json!({ "type": "input_text", "text": text })),
        MessagePart::Image {
            mime_type,
            data_base64,
        } => Some(json!({
            "type": "input_image",
            "image_url": data_url(mime_type, data_base64),
        })),
        MessagePart::File {
            file_name,
            mime_type,
            data_base64,
            uri,
        } => {
            if let Some(uri) = uri {
                Some(json!({
                    "type": "input_file",
                    "file_url": uri,
                    "filename": file_name,
                }))
            } else {
                data_base64.as_ref().map(|data_base64| {
                    let file_data = mime_type
                        .as_deref()
                        .map(|mime_type| data_url(mime_type, data_base64))
                        .unwrap_or_else(|| data_base64.clone());
                    json!({
                        "type": "input_file",
                        "file_data": file_data,
                        "filename": file_name,
                    })
                })
            }
        }
        MessagePart::Resource {
            uri,
            text,
            metadata,
            ..
        } => Some(json!({
            "type": "input_text",
            "text": text.clone().unwrap_or_else(|| {
                metadata
                    .as_ref()
                    .map(|metadata| format!("{uri}\n{metadata}"))
                    .unwrap_or_else(|| uri.clone())
            }),
        })),
        MessagePart::Json { value } => Some(json!({
            "type": "input_text",
            "text": stringify_json(value),
        })),
        MessagePart::ProviderExtension { payload, .. } => Some(json!({
            "type": "input_text",
            "text": stringify_json(payload),
        })),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            (!text.is_empty()).then(|| json!({ "type": "input_text", "text": text }))
        }
        MessagePart::ToolCall { call } => Some(json!({
            "type": "input_text",
            "text": stringify_json(&call.arguments),
        })),
        MessagePart::ToolResult { .. } => None,
    }
}

fn openai_assistant_message_block(part: &MessagePart) -> Option<Value> {
    match part {
        MessagePart::Text { text } => Some(json!({ "type": "output_text", "text": text })),
        MessagePart::Resource {
            uri,
            text,
            metadata,
            ..
        } => Some(json!({
            // Assistant replay must use the Responses output-message shape.
            // OpenAI accepts `input_text` for user/system/developer messages,
            // but assistant history content only permits `output_text` or
            // `refusal`, while reasoning and tool calls are replayed as
            // standalone items.
            "type": "output_text",
            "text": text.clone().unwrap_or_else(|| {
                metadata
                    .as_ref()
                    .map(|metadata| format!("{uri}\n{metadata}"))
                    .unwrap_or_else(|| uri.clone())
            }),
        })),
        MessagePart::Json { value } => Some(json!({
            "type": "output_text",
            "text": stringify_json(value),
        })),
        MessagePart::ProviderExtension { payload, .. } => Some(json!({
            "type": "output_text",
            "text": stringify_json(payload),
        })),
        _ => None,
    }
}

fn openai_input_item_from_part(part: &MessagePart, role: MessageRole) -> Option<Value> {
    match part {
        MessagePart::ToolCall { call } if matches!(role, MessageRole::Assistant) => Some(json!({
            "type": "function_call",
            "id": call.id,
            "call_id": call.call_id,
            "name": call.tool_name,
            // Responses `function_call` items carry JSON-encoded arguments as a
            // string. We keep parsed arguments in the runtime transcript for
            // local tool execution, then re-encode them here so transcript
            // replay and continuation fallback preserve the provider's item shape.
            "arguments": stringify_json(&call.arguments),
        })),
        MessagePart::ToolResult { result } => Some(json!({
            "type": "function_call_output",
            "call_id": result.call_id,
            // Responses currently treat tool output as text, so rich local tool
            // results travel through the versioned round-trip envelope.
            "output": tool_result_roundtrip_text(result),
        })),
        MessagePart::Reasoning { reasoning } if matches!(role, MessageRole::Assistant) => {
            let Some(id) = reasoning.id.clone() else {
                return None;
            };
            let mut summary = Vec::new();
            let mut encrypted_content = None;
            for content in &reasoning.content {
                match content {
                    ReasoningContent::Text { text, .. } | ReasoningContent::Summary(text) => {
                        summary.push(json!({
                            "type": "summary_text",
                            "text": text,
                        }));
                    }
                    ReasoningContent::Encrypted(data) | ReasoningContent::Redacted { data } => {
                        encrypted_content.get_or_insert_with(|| data.clone());
                    }
                }
            }
            let mut item = Map::new();
            item.insert("type".to_string(), Value::String("reasoning".to_string()));
            item.insert("id".to_string(), Value::String(id.to_string()));
            item.insert("summary".to_string(), Value::Array(summary));
            if let Some(encrypted_content) = encrypted_content {
                item.insert(
                    "encrypted_content".to_string(),
                    Value::String(encrypted_content),
                );
            }
            Some(Value::Object(item))
        }
        _ => None,
    }
}
