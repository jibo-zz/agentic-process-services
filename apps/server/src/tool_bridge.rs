use agentic_protocol::{ChatStreamEvent, ToolResultAck, ToolResultParams};
use serde_json::Value;
use std::{collections::HashMap, error::Error, fmt, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct ToolBridge {
    pending: Arc<Mutex<HashMap<InvocationKey, PendingInvocation>>>,
}

impl fmt::Debug for ToolBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolBridge").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct StreamAuth {
    pub stream_id: String,
    pub stream_secret: String,
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct InvocationKey {
    stream_id: String,
    invocation_id: String,
}

struct PendingInvocation {
    stream_secret: String,
    tx: oneshot::Sender<Result<Value, String>>,
}

#[derive(Debug)]
pub enum BridgeError {
    ClientDisconnected,
    TimedOut,
    Rejected(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientDisconnected => f.write_str("local tool client disconnected"),
            Self::TimedOut => f.write_str("local tool timed out"),
            Self::Rejected(message) => f.write_str(message),
        }
    }
}

impl Error for BridgeError {}

impl ToolBridge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_stream(&self) -> StreamAuth {
        StreamAuth {
            stream_id: Uuid::new_v4().to_string(),
            stream_secret: Uuid::new_v4().to_string(),
        }
    }

    pub async fn request_local_tool(
        &self,
        auth: &StreamAuth,
        events: &mpsc::UnboundedSender<ChatStreamEvent>,
        name: String,
        input: Value,
        approval_required: bool,
        summary: String,
    ) -> Result<Value, BridgeError> {
        let invocation_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let key = InvocationKey {
            stream_id: auth.stream_id.clone(),
            invocation_id: invocation_id.clone(),
        };
        self.pending.lock().await.insert(
            key.clone(),
            PendingInvocation {
                stream_secret: auth.stream_secret.clone(),
                tx,
            },
        );

        if events
            .send(ChatStreamEvent::LocalToolRequest {
                invocation_id,
                name,
                input,
                approval_required,
                summary,
            })
            .is_err()
        {
            self.pending.lock().await.remove(&key);
            return Err(BridgeError::ClientDisconnected);
        }

        let result = tokio::time::timeout(Duration::from_secs(120), rx)
            .await
            .map_err(|_| BridgeError::TimedOut)?
            .map_err(|_| BridgeError::ClientDisconnected)?;

        result.map_err(BridgeError::Rejected)
    }

    pub async fn complete(&self, params: ToolResultParams) -> Result<ToolResultAck, String> {
        let key = InvocationKey {
            stream_id: params.stream_id,
            invocation_id: params.invocation_id,
        };
        let Some(pending) = self.pending.lock().await.remove(&key) else {
            return Err("unknown or expired tool invocation".to_owned());
        };
        if pending.stream_secret != params.stream_secret {
            return Err("invalid stream secret".to_owned());
        }
        let result = match (params.output, params.error) {
            (Some(output), None) => Ok(output),
            (_, Some(error)) => Err(error),
            (None, None) => Err("tool result did not include output or error".to_owned()),
        };
        let _ = pending.tx.send(result);
        Ok(ToolResultAck { accepted: true })
    }
}
