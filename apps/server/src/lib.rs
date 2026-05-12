use agentic_protocol::{
    CHAT_STREAM_PATH, ChatRequest, HealthResponse, LLM_PREAMBLE, LlmChunk,
    RPC_HEALTH_CHECK, RPC_PATH, RpcError, RpcRequest, RpcResponse,
};
use axum::{
    Json, Router,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures_util::StreamExt;
use rig::{
    agent::MultiTurnStreamItem,
    client::{CompletionClient, ProviderClient},
    completion::{AssistantContent, Message as RigMessage, message::{Text as RigText, UserContent}},
    providers::deepseek,
    streaming::{StreamedAssistantContent, StreamingPrompt},
    OneOrMany,
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
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)>
{
    if req.messages.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "messages must not be empty".to_owned()));
    }
    let last = req.messages.last().unwrap();
    if last.role != "user" {
        return Err((StatusCode::BAD_REQUEST, "last message role must be 'user'".to_owned()));
    }

    let prompt = last.content.clone();
    let prior = &req.messages[..req.messages.len() - 1];

    let prior_rig_messages: Vec<RigMessage> = prior
        .iter()
        .map(|msg| {
            if msg.role == "assistant" {
                RigMessage::Assistant {
                    id: None,
                    content: OneOrMany::one(AssistantContent::Text(
                        RigText { text: msg.content.clone() },
                    )),
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
    let agent = client
        .agent(deepseek::DEEPSEEK_V4_FLASH)
        .preamble(LLM_PREAMBLE)
        .build();

    let stream = agent
        .stream_prompt(&prompt)
        .with_history(prior_rig_messages)
        .await
        .filter_map(|item| async move {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                    text,
                ))) if !text.text.is_empty() => {
                    let data = serde_json::to_string(&LlmChunk {
                        text: text.text.to_string(),
                    })
                    .ok()?;
                    Some(Ok(Event::default().data(data)))
                }
                _ => None,
            }
        })
        .then(|event| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            event
        });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
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
