//! Shared API DTOs for server responses and future CLI clients.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RPC_PATH: &str = "/rpc";
pub const RPC_HEALTH_CHECK: &str = "health.check";
pub const RPC_LLM_GENERATE: &str = "llm.generate";
pub const RPC_SESSIONS_LIST: &str = "sessions.list";
pub const RPC_SESSIONS_GET: &str = "sessions.get";
pub const RPC_TOOLS_LIST: &str = "tools.list";
pub const RPC_TOOLS_RESULT: &str = "tools.result";
pub const CHAT_STREAM_PATH: &str = "/chat/stream";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" or "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub session_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at_secs: u64,
    pub message_count: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionKind {
    ServerNative,
    LocalProxy,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    ReadOnly,
    WritesFiles,
    DeletesFiles,
    DeletesDirectories,
    Network,
    ExternalProcess,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputPolicy {
    FullToModelSummaryToUi,
    SummaryOnly,
    FullAllowed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub execution: ToolExecutionKind,
    pub approval_required: bool,
    pub risk: ToolRisk,
    pub output_policy: ToolOutputPolicy,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResponse {
    pub tools: Vec<ToolDescriptor>,
}

pub const LLM_PREAMBLE: &str = "You are usful assistance.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiMessage {
    pub role: UiRole,
    pub parts: Vec<UiPart>,
}

impl UiMessage {
    pub fn user_text(content: impl Into<String>) -> Self {
        Self {
            role: UiRole::User,
            parts: vec![UiPart::Text {
                content: content.into(),
            }],
        }
    }

    pub fn assistant() -> Self {
        Self {
            role: UiRole::Assistant,
            parts: vec![],
        }
    }

    pub fn apply_stream_event(&mut self, event: &ChatStreamEvent) {
        match event {
            ChatStreamEvent::TextDelta { text } => {
                if let Some(UiPart::Text { content }) = self.parts.last_mut() {
                    content.push_str(text);
                } else {
                    self.parts.push(UiPart::Text {
                        content: text.clone(),
                    });
                }
            }
            ChatStreamEvent::ReasoningDelta { text } => {
                if let Some(UiPart::Reasoning { content }) = self.parts.last_mut() {
                    content.push_str(text);
                } else {
                    self.parts.push(UiPart::Reasoning {
                        content: text.clone(),
                    });
                }
            }
            ChatStreamEvent::ToolUpdate {
                id,
                name,
                state,
                input,
                output,
                error,
            } => {
                let pos = self
                    .parts
                    .iter()
                    .position(|p| matches!(p, UiPart::Tool { id: tid, .. } if tid == id));
                if let Some(idx) = pos {
                    if let UiPart::Tool {
                        state: s,
                        input: inp,
                        output: out,
                        error: err,
                        ..
                    } = &mut self.parts[idx]
                    {
                        *s = *state;
                        if input.is_some() {
                            *inp = input.clone();
                        }
                        if output.is_some() {
                            *out = output.clone();
                        }
                        if error.is_some() {
                            *err = error.clone();
                        }
                    }
                } else {
                    self.parts.push(UiPart::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        state: *state,
                        input: input.clone(),
                        output: output.clone(),
                        error: error.clone(),
                    });
                }
            }
            ChatStreamEvent::LocalToolRequest {
                invocation_id,
                name,
                approval_required,
                summary,
                ..
            } => {
                self.parts.push(UiPart::Tool {
                    id: invocation_id.clone(),
                    name: name.clone(),
                    state: if *approval_required {
                        ToolState::AwaitingApproval
                    } else {
                        ToolState::Calling
                    },
                    input: Some(serde_json::json!({ "summary": summary })),
                    output: None,
                    error: None,
                });
            }
            ChatStreamEvent::Error { message } => {
                self.parts.push(UiPart::Error {
                    message: message.clone(),
                });
            }
            ChatStreamEvent::StreamReady { .. }
            | ChatStreamEvent::MessageStart { .. }
            | ChatStreamEvent::MessageDone => {}
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiPart {
    Text {
        content: String,
    },
    Reasoning {
        content: String,
    },
    Tool {
        id: String,
        name: String,
        state: ToolState,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Streaming,
    Calling,
    AwaitingApproval,
    Complete,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatStreamEvent {
    StreamReady {
        stream_id: String,
        stream_secret: String,
    },
    MessageStart {
        role: UiRole,
    },
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolUpdate {
        id: String,
        name: String,
        state: ToolState,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    LocalToolRequest {
        invocation_id: String,
        name: String,
        input: Value,
        approval_required: bool,
        summary: String,
    },
    Error {
        message: String,
    },
    MessageDone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultParams {
    pub stream_id: String,
    pub stream_secret: String,
    pub invocation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultAck {
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmChunk {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}

impl HealthResponse {
    pub fn ok(service: impl Into<String>) -> Self {
        Self {
            status: "ok".to_owned(),
            service: service.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub output: String,
}

impl LlmResponse {
    pub fn new(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    pub id: u64,
}

impl RpcRequest {
    pub fn method(id: u64, method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            method: method.into(),
            params: None,
            id,
        }
    }

    pub fn with_params(mut self, params: serde_json::Value) -> Self {
        self.params = Some(params);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse<T> {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
    pub id: u64,
}

impl<T> RpcResponse<T> {
    pub fn success(id: u64, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn failure(id: u64, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            result: None,
            error: Some(error),
            id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "invalid JSON-RPC request".to_owned(),
        }
    }

    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {}", method.into()),
        }
    }

    pub fn internal_error(error: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: error.into(),
        }
    }
}
