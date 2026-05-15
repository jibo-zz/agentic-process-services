mod tools;

use agentic_protocol::{
    CHAT_STREAM_PATH, ChatRequest, ChatStreamEvent, HealthResponse, LLM_PREAMBLE, RPC_HEALTH_CHECK,
    RPC_PATH, RpcError, RpcRequest, RpcResponse, ToolState, UiRole,
};
use axum::{
    Json, Router,
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::{
    StreamExt,
    stream::{self, BoxStream},
};
use rig::{
    OneOrMany,
    agent::MultiTurnStreamItem,
    client::{CompletionClient, ProviderClient},
    completion::{
        AssistantContent, Message as RigMessage,
        message::{Text as RigText, ToolResultContent, UserContent},
    },
    providers::deepseek,
    streaming::{
        StreamedAssistantContent, StreamedUserContent, StreamingPrompt, ToolCallDeltaContent,
    },
};
use std::{convert::Infallible, time::Duration};

pub type AppType = Router;

pub fn app() -> AppType {
    Router::new()
        .route("/health", get(health))
        .route(CHAT_STREAM_PATH, post(chat_stream))
        .route(RPC_PATH, post(rpc))
}

async fn health() -> Json<HealthResponse> {
    Json(health_response())
}

async fn chat_stream(
    Json(req): Json<ChatRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.messages.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "messages must not be empty".to_owned(),
        ));
    }
    let last = req.messages.last().unwrap();
    if last.role != "user" {
        return Err((
            StatusCode::BAD_REQUEST,
            "last message role must be 'user'".to_owned(),
        ));
    }

    let prompt = last.content.clone();
    if prompt == "debug:tool-stream" {
        return Ok(Sse::new(debug_tool_stream()).keep_alive(KeepAlive::default()));
    }

    let prior = &req.messages[..req.messages.len() - 1];

    let prior_rig_messages: Vec<RigMessage> = prior
        .iter()
        .map(|msg| {
            if msg.role == "assistant" {
                RigMessage::Assistant {
                    id: None,
                    content: OneOrMany::one(AssistantContent::Text(RigText {
                        text: msg.content.clone(),
                    })),
                }
            } else {
                RigMessage::User {
                    content: OneOrMany::one(UserContent::Text(RigText {
                        text: msg.content.clone(),
                    })),
                }
            }
        })
        .collect();

    let client = deepseek::Client::from_env().map_err(internal_error)?;
    let preamble = format!(
        "{LLM_PREAMBLE}\n\nUse the get_current_weather tool for current weather questions. The tool defaults to celsius; request fahrenheit only if the user explicitly asks for it."
    );
    let agent = client
        .agent(deepseek::DEEPSEEK_V4_FLASH)
        .preamble(&preamble)
        .tool(tools::CurrentWeatherTool)
        .build();

    let stream = agent
        .stream_prompt(&prompt)
        .with_history(prior_rig_messages)
        .await
        .filter_map(|item| async move { stream_item_event(item) })
        .then(|event| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            event
        });

    let stream = stream::iter([stream_event(ChatStreamEvent::MessageStart {
        role: UiRole::Assistant,
    })])
    .chain(stream)
    .chain(stream::iter([stream_event(ChatStreamEvent::MessageDone)]))
    .boxed();

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn stream_item_event<R>(
    item: Result<MultiTurnStreamItem<R>, impl std::fmt::Display>,
) -> Option<Result<Event, Infallible>> {
    match item {
        Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => assistant_content_event(content),
        Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
            tool_result,
            ..
        })) => Some(stream_event(tool_result_event(tool_result))),
        Ok(MultiTurnStreamItem::FinalResponse(_)) => None,
        Ok(_) => None,
        Err(error) => Some(stream_event(ChatStreamEvent::Error {
            message: error.to_string(),
        })),
    }
}

fn assistant_content_event<R>(
    content: StreamedAssistantContent<R>,
) -> Option<Result<Event, Infallible>> {
    match content {
        StreamedAssistantContent::Text(text) if !text.text.is_empty() => {
            Some(stream_event(ChatStreamEvent::TextDelta { text: text.text }))
        }
        StreamedAssistantContent::Reasoning(reasoning) => {
            let text = reasoning.display_text();
            (!text.is_empty()).then(|| stream_event(ChatStreamEvent::ReasoningDelta { text }))
        }
        StreamedAssistantContent::ReasoningDelta { reasoning, .. } if !reasoning.is_empty() => {
            Some(stream_event(ChatStreamEvent::ReasoningDelta {
                text: reasoning,
            }))
        }
        StreamedAssistantContent::ToolCall { tool_call, .. } => {
            Some(stream_event(ChatStreamEvent::ToolUpdate {
                id: tool_call.id,
                name: tool_call.function.name,
                state: ToolState::Calling,
                input: Some(tool_call.function.arguments),
                output: None,
                error: None,
            }))
        }
        StreamedAssistantContent::ToolCallDelta { id, content, .. } => {
            let (name, input) = match content {
                ToolCallDeltaContent::Name(name) => (name, None),
                ToolCallDeltaContent::Delta(delta) => {
                    (String::new(), Some(serde_json::json!({ "delta": delta })))
                }
            };
            Some(stream_event(ChatStreamEvent::ToolUpdate {
                id,
                name,
                state: ToolState::Streaming,
                input,
                output: None,
                error: None,
            }))
        }
        StreamedAssistantContent::Final(_) | StreamedAssistantContent::Text(_) => None,
        StreamedAssistantContent::ReasoningDelta { .. } => None,
    }
}

fn tool_result_event(tool_result: rig::completion::message::ToolResult) -> ChatStreamEvent {
    let output = tool_result_output(&tool_result.content);
    match output {
        serde_json::Value::String(message) => ChatStreamEvent::ToolUpdate {
            id: tool_result.id,
            name: String::new(),
            state: ToolState::Failed,
            input: None,
            output: None,
            error: Some(message),
        },
        output => ChatStreamEvent::ToolUpdate {
            id: tool_result.id,
            name: String::new(),
            state: ToolState::Complete,
            input: None,
            output: Some(output),
            error: None,
        },
    }
}

fn tool_result_output(content: &OneOrMany<ToolResultContent>) -> serde_json::Value {
    let mut values = content
        .iter()
        .map(tool_result_content_value)
        .collect::<Vec<_>>();
    if values.len() == 1 {
        values.remove(0)
    } else {
        serde_json::Value::Array(values)
    }
}

fn tool_result_content_value(content: &ToolResultContent) -> serde_json::Value {
    match content {
        ToolResultContent::Text(text) => serde_json::from_str(&text.text)
            .unwrap_or_else(|_| serde_json::Value::String(text.text.clone())),
        ToolResultContent::Image(image) => serde_json::to_value(image)
            .unwrap_or_else(|_| serde_json::Value::String("<image>".to_owned())),
    }
}

fn debug_tool_stream() -> BoxStream<'static, Result<Event, Infallible>> {
    let events = vec![
        ChatStreamEvent::MessageStart {
            role: UiRole::Assistant,
        },
        ChatStreamEvent::ReasoningDelta {
            text: "I need to inspect a small input, call one tool, and handle a failure.\n"
                .to_owned(),
        },
        ChatStreamEvent::ToolUpdate {
            id: "tool-1".to_owned(),
            name: "read_config".to_owned(),
            state: ToolState::Streaming,
            input: Some(serde_json::json!({ "path": "apps/server/.env" })),
            output: None,
            error: None,
        },
        ChatStreamEvent::ToolUpdate {
            id: "tool-1".to_owned(),
            name: "read_config".to_owned(),
            state: ToolState::Calling,
            input: Some(serde_json::json!({ "path": "apps/server/.env" })),
            output: None,
            error: None,
        },
        ChatStreamEvent::ToolUpdate {
            id: "tool-1".to_owned(),
            name: "read_config".to_owned(),
            state: ToolState::Complete,
            input: Some(serde_json::json!({ "path": "apps/server/.env" })),
            output: Some(serde_json::json!({ "loaded": true, "keys": ["DEEPSEEK_API_KEY"] })),
            error: None,
        },
        ChatStreamEvent::ToolUpdate {
            id: "tool-2".to_owned(),
            name: "fetch_remote".to_owned(),
            state: ToolState::Failed,
            input: Some(serde_json::json!({ "url": "https://example.invalid/status" })),
            output: None,
            error: Some("DNS lookup failed".to_owned()),
        },
        ChatStreamEvent::TextDelta {
            text: "The config tool completed, but the remote status check failed. ".to_owned(),
        },
        ChatStreamEvent::Error {
            message: "Remote status is unavailable; continuing with local context.".to_owned(),
        },
        ChatStreamEvent::TextDelta {
            text: "This verifies text, reasoning, tool updates, and non-terminal errors."
                .to_owned(),
        },
        ChatStreamEvent::MessageDone,
    ];

    stream::iter(events)
        .then(|event| async move {
            tokio::time::sleep(Duration::from_millis(180)).await;
            stream_event(event)
        })
        .boxed()
}

fn stream_event(event: ChatStreamEvent) -> Result<Event, Infallible> {
    let data = serde_json::to_string(&event).expect("chat stream event serializes");
    Ok(Event::default().data(data))
}

async fn rpc(Json(request): Json<RpcRequest>) -> Json<RpcResponse<serde_json::Value>> {
    if request.jsonrpc != "2.0" {
        return Json(RpcResponse::failure(
            request.id,
            RpcError::invalid_request(),
        ));
    }

    match request.method.as_str() {
        RPC_HEALTH_CHECK => Json(RpcResponse::success(
            request.id,
            serde_json::to_value(health_response()).expect("health response serializes"),
        )),
        _ => Json(RpcResponse::failure(
            request.id,
            RpcError::method_not_found(request.method),
        )),
    }
}

fn health_response() -> HealthResponse {
    HealthResponse::ok(agentic_core::SERVICE_NAME)
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
