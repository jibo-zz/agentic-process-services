mod tool_bridge;
mod tools;

use agentic_db::DatabaseConnection;
use agentic_db::repo::{messages, sessions};
use agentic_protocol::{
    CHAT_STREAM_PATH, ChatRequest, ChatStreamEvent, HealthResponse, LLM_PREAMBLE, RPC_HEALTH_CHECK,
    RPC_PATH, RPC_SESSIONS_GET, RPC_SESSIONS_LIST, RPC_TOOLS_LIST, RPC_TOOLS_RESULT, RpcError,
    RpcRequest, RpcResponse, ToolResultAck, ToolResultParams, ToolState, ToolsListResponse,
    UiMessage, UiRole,
};
use axum::{
    Json, Router,
    extract::State,
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
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tool_bridge::ToolBridge;

pub type AppType = Router;
const AGENT_MAX_TOOL_TURNS: usize = 12;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub bridge: ToolBridge,
}

pub fn app(db: DatabaseConnection) -> AppType {
    let state = Arc::new(AppState {
        db,
        bridge: ToolBridge::new(),
    });
    Router::new()
        .route("/health", get(health))
        .route(CHAT_STREAM_PATH, post(chat_stream))
        .route(RPC_PATH, post(rpc))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(health_response())
}

async fn chat_stream(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.message.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "message must not be empty".to_owned(),
        ));
    }

    if req.message == "debug:tool-stream" {
        return Ok(Sse::new(debug_tool_stream()).keep_alive(KeepAlive::default()));
    }

    let session_id = req.session_id.clone();
    let prompt = req.message.clone();
    let title = prompt.chars().take(60).collect::<String>();

    // Upsert session, count prior messages, load LLM history — all before inserting user turn
    sessions::upsert(&state.db, &session_id, &title)
        .await
        .map_err(internal_error)?;

    let user_position = messages::count(&state.db, &session_id)
        .await
        .map_err(internal_error)?;

    let prior_chat_messages = sessions::load_history(&state.db, &session_id)
        .await
        .map_err(internal_error)?;

    // Insert user message
    let user_msg = UiMessage::user_text(&prompt);
    messages::insert(&state.db, &session_id, user_position, &user_msg)
        .await
        .map_err(internal_error)?;

    let prior_rig_messages: Vec<RigMessage> = prior_chat_messages
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
    let stream_auth = state.bridge.new_stream();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
    let local_tool_context =
        tools::LocalToolContext::new(state.bridge.clone(), stream_auth.clone(), event_tx.clone());
    let preamble = format!(
        "{LLM_PREAMBLE}\n\nYou are a coding agent. Use local file tools to inspect and modify the user's workspace instead of guessing, but keep research focused and answer once you have enough evidence. Use read_file before editing existing files. write_file, edit_file, delete_file, and delete_directory require user approval in the CLI. Use delete_file when the user asks to remove a file; use delete_directory only for empty directories. Do not empty a file as a substitute for deletion. Use get_current_weather for current weather questions; it defaults to celsius and should use fahrenheit only when explicitly requested."
    );
    let agent = client
        .agent(deepseek::DEEPSEEK_V4_FLASH)
        .preamble(&preamble)
        .default_max_turns(AGENT_MAX_TOOL_TURNS)
        .tool(tools::CurrentWeatherTool)
        .tool(tools::ListFilesProxyTool::new(local_tool_context.clone()))
        .tool(tools::ReadFileProxyTool::new(local_tool_context.clone()))
        .tool(tools::SearchFilesProxyTool::new(local_tool_context.clone()))
        .tool(tools::WriteFileProxyTool::new(local_tool_context.clone()))
        .tool(tools::EditFileProxyTool::new(local_tool_context.clone()))
        .tool(tools::DeleteFileProxyTool::new(local_tool_context.clone()))
        .tool(tools::DeleteDirectoryProxyTool::new(local_tool_context))
        .build();

    let agent_stream = agent
        .stream_prompt(&prompt)
        .multi_turn(AGENT_MAX_TOOL_TURNS)
        .with_history(prior_rig_messages)
        .await
        .filter_map(|item| async move { stream_item_to_chat_event(item) })
        .then(|event| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            event
        });

    tokio::spawn(async move {
        tokio::pin!(agent_stream);
        while let Some(event) = agent_stream.next().await {
            if event_tx.send(event).is_err() {
                return;
            }
        }
        let _ = event_tx.send(ChatStreamEvent::MessageDone);
    });

    let event_stream = stream::unfold(event_rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    });

    let full_stream: BoxStream<ChatStreamEvent> = stream::iter([
        ChatStreamEvent::StreamReady {
            stream_id: stream_auth.stream_id,
            stream_secret: stream_auth.stream_secret,
        },
        ChatStreamEvent::MessageStart {
            role: UiRole::Assistant,
        },
    ])
    .chain(event_stream)
    .boxed();

    // Side-channel: accumulate assistant message and persist on MessageDone
    let (persist_tx, mut persist_rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
    let state_clone = state.clone();
    let session_id_clone = session_id.clone();
    let assistant_position = user_position + 1;

    tokio::spawn(async move {
        let mut assistant_msg = UiMessage::assistant();
        while let Some(event) = persist_rx.recv().await {
            if matches!(event, ChatStreamEvent::MessageDone) {
                let db = &state_clone.db;
                let _ = sessions::touch_updated_at(db, &session_id_clone).await;
                let _ = messages::insert(db, &session_id_clone, assistant_position, &assistant_msg)
                    .await;
                break;
            }
            assistant_msg.apply_stream_event(&event);
        }
    });

    let sse_stream: BoxStream<Result<Event, Infallible>> = full_stream
        .inspect(move |event| {
            let _ = persist_tx.send(sanitize_event_for_persistence(event));
        })
        .map(stream_event)
        .boxed();

    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::default()))
}

fn sanitize_event_for_persistence(event: &ChatStreamEvent) -> ChatStreamEvent {
    match event {
        ChatStreamEvent::ToolUpdate {
            id,
            name,
            state,
            input,
            output,
            error,
        } => ChatStreamEvent::ToolUpdate {
            id: id.clone(),
            name: name.clone(),
            state: *state,
            input: input.as_ref().map(sanitize_tool_value),
            output: output.as_ref().map(sanitize_tool_value),
            error: error.clone(),
        },
        ChatStreamEvent::LocalToolRequest {
            invocation_id,
            name,
            approval_required,
            summary,
            ..
        } => ChatStreamEvent::LocalToolRequest {
            invocation_id: invocation_id.clone(),
            name: name.clone(),
            input: serde_json::json!({ "summary": summary }),
            approval_required: *approval_required,
            summary: summary.clone(),
        },
        ChatStreamEvent::StreamReady { .. } => ChatStreamEvent::StreamReady {
            stream_id: String::new(),
            stream_secret: String::new(),
        },
        event => event.clone(),
    }
}

fn sanitize_tool_value(value: &serde_json::Value) -> serde_json::Value {
    let mut output = serde_json::Map::new();
    for key in [
        "summary",
        "path",
        "query",
        "bytes",
        "lines",
        "truncated",
        "overwritten",
        "replacements",
    ] {
        if let Some(value) = value.get(key) {
            output.insert(key.to_owned(), value.clone());
        }
    }
    if output.is_empty() {
        serde_json::json!({ "summary": "Tool event" })
    } else {
        serde_json::Value::Object(output)
    }
}

fn stream_item_to_chat_event<R>(
    item: Result<MultiTurnStreamItem<R>, impl std::fmt::Display>,
) -> Option<ChatStreamEvent> {
    match item {
        Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
            assistant_content_to_chat_event(content)
        }
        Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
            tool_result,
            ..
        })) => Some(tool_result_event(tool_result)),
        Ok(MultiTurnStreamItem::FinalResponse(_)) => None,
        Ok(_) => None,
        Err(error) => Some(ChatStreamEvent::Error {
            message: error.to_string(),
        }),
    }
}

fn assistant_content_to_chat_event<R>(
    content: StreamedAssistantContent<R>,
) -> Option<ChatStreamEvent> {
    match content {
        StreamedAssistantContent::Text(text) if !text.text.is_empty() => {
            Some(ChatStreamEvent::TextDelta { text: text.text })
        }
        StreamedAssistantContent::Reasoning(reasoning) => {
            let text = reasoning.display_text();
            (!text.is_empty()).then_some(ChatStreamEvent::ReasoningDelta { text })
        }
        StreamedAssistantContent::ReasoningDelta { reasoning, .. } if !reasoning.is_empty() => {
            Some(ChatStreamEvent::ReasoningDelta { text: reasoning })
        }
        StreamedAssistantContent::ToolCall { tool_call, .. } => Some(ChatStreamEvent::ToolUpdate {
            id: tool_call.id,
            name: tool_call.function.name,
            state: ToolState::Calling,
            input: Some(tool_call.function.arguments),
            output: None,
            error: None,
        }),
        StreamedAssistantContent::ToolCallDelta { id, content, .. } => {
            let (name, input) = match content {
                ToolCallDeltaContent::Name(name) => (name, None),
                ToolCallDeltaContent::Delta(delta) => {
                    (String::new(), Some(serde_json::json!({ "delta": delta })))
                }
            };
            Some(ChatStreamEvent::ToolUpdate {
                id,
                name,
                state: ToolState::Streaming,
                input,
                output: None,
                error: None,
            })
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
            output: Some(sanitize_local_tool_result_for_ui(output)),
            error: None,
        },
    }
}

fn sanitize_local_tool_result_for_ui(output: serde_json::Value) -> serde_json::Value {
    let is_local_file_tool = output.get("content").is_some()
        || output.get("files").is_some()
        || output.get("matches").is_some()
        || output.get("overwritten").is_some()
        || output.get("replacements").is_some();
    if is_local_file_tool {
        agentic_tools::sanitized_output("local_file_tool", &output)
    } else {
        output
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

async fn rpc(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RpcRequest>,
) -> Json<RpcResponse<serde_json::Value>> {
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
        RPC_SESSIONS_LIST => match sessions::list(&state.db).await {
            Ok(list) => {
                let value = serde_json::to_value(list).unwrap_or(serde_json::Value::Array(vec![]));
                Json(RpcResponse::success(request.id, value))
            }
            Err(e) => Json(RpcResponse::failure(
                request.id,
                RpcError::internal_error(e.to_string()),
            )),
        },
        RPC_SESSIONS_GET => {
            let id = request
                .params
                .as_ref()
                .and_then(|p| p["id"].as_str())
                .unwrap_or("");
            if id.is_empty() {
                return Json(RpcResponse::failure(
                    request.id,
                    RpcError::invalid_request(),
                ));
            }
            match sessions::load_ui_messages(&state.db, id).await {
                Ok(msgs) => {
                    let value =
                        serde_json::to_value(msgs).unwrap_or(serde_json::Value::Array(vec![]));
                    Json(RpcResponse::success(request.id, value))
                }
                Err(e) => Json(RpcResponse::failure(
                    request.id,
                    RpcError::internal_error(e.to_string()),
                )),
            }
        }
        RPC_TOOLS_LIST => {
            let value = serde_json::to_value(ToolsListResponse {
                tools: agentic_tools::descriptors(),
            })
            .unwrap_or_else(|_| serde_json::json!({ "tools": [] }));
            Json(RpcResponse::success(request.id, value))
        }
        RPC_TOOLS_RESULT => {
            let Some(params) = request.params.clone() else {
                return Json(RpcResponse::failure(
                    request.id,
                    RpcError::invalid_request(),
                ));
            };
            let params = match serde_json::from_value::<ToolResultParams>(params) {
                Ok(params) => params,
                Err(_) => {
                    return Json(RpcResponse::failure(
                        request.id,
                        RpcError::invalid_request(),
                    ));
                }
            };
            match state.bridge.complete(params).await {
                Ok(ack) => {
                    let value = serde_json::to_value(ack).unwrap_or_else(|_| {
                        serde_json::to_value(ToolResultAck { accepted: true })
                            .expect("tool result ack serializes")
                    });
                    Json(RpcResponse::success(request.id, value))
                }
                Err(error) => Json(RpcResponse::failure(
                    request.id,
                    RpcError::internal_error(error),
                )),
            }
        }
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
