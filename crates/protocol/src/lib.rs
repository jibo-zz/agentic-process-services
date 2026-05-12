//! Shared API DTOs for server responses and future CLI clients.

use serde::{Deserialize, Serialize};

pub const RPC_PATH: &str = "/rpc";
pub const RPC_HEALTH_CHECK: &str = "health.check";
pub const RPC_LLM_GENERATE: &str = "llm.generate";
pub const CHAT_STREAM_PATH: &str = "/chat/stream";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,   // "user" or "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,  // full history; last entry must be the new user message
}
pub const LLM_PREAMBLE: &str = "You are usful assistance.";

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
