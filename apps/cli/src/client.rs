use agentic_config::DEFAULT_SERVER_ADDR;
use agentic_protocol::{
    AUTHOR_STREAM_PATH, AgentMode, AuthorRequest, CHAT_STREAM_PATH, ChatRequest, ChatStreamEvent,
    DeleteAck, DeleteToolParams, DeleteVersionParams, HealthResponse, RPC_HEALTH_CHECK, RPC_PATH,
    RPC_SESSIONS_GET, RPC_SESSIONS_LIST, RPC_TOOLS_DELETE_TOOL, RPC_TOOLS_DELETE_VERSION,
    RPC_TOOLS_LIST, RPC_TOOLS_MANAGEMENT, RPC_TOOLS_REGISTER, RPC_TOOLS_RESULT,
    RPC_TOOLS_SAVE_DRAFT, RegisterParams, RpcError, RpcRequest, RpcResponse, SaveDraftParams,
    SessionSummary, ToolResultAck, ToolResultParams, ToolVersionRow, ToolsListResponse,
    ToolsManagementResponse, UiMessage,
};
use futures_util::{Stream, StreamExt, stream};
use serde::de::DeserializeOwned;
use std::{error::Error, fmt};

const RPC_HEALTH_ID: u64 = 1;
const RPC_SESSIONS_LIST_ID: u64 = 2;
const RPC_SESSIONS_GET_ID: u64 = 3;
const RPC_TOOLS_RESULT_ID: u64 = 4;
const RPC_TOOLS_LIST_ID: u64 = 5;
const RPC_TOOLS_MANAGEMENT_ID: u64 = 6;
const RPC_TOOLS_SAVE_DRAFT_ID: u64 = 7;
const RPC_TOOLS_REGISTER_ID: u64 = 8;
const RPC_TOOLS_DELETE_VERSION_ID: u64 = 9;
const RPC_TOOLS_DELETE_TOOL_ID: u64 = 10;
const AGENTS_SERVER_URL_ENV: &str = "AGENTS_SERVER_URL";

#[derive(Debug)]
pub enum FetchError {
    Http(reqwest::Error),
    Rpc(RpcError),
    MissingResult,
    Decode(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "HTTP request failed: {error}"),
            Self::Rpc(error) => write!(f, "RPC failed: {} ({})", error.message, error.code),
            Self::MissingResult => f.write_str("RPC response did not include a result"),
            Self::Decode(error) => write!(f, "response decode failed: {error}"),
        }
    }
}

impl Error for FetchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Rpc(_) | Self::MissingResult | Self::Decode(_) => None,
        }
    }
}

impl From<reqwest::Error> for FetchError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

#[derive(Clone)]
pub struct Fetcher {
    base_url: String,
    http: reqwest::Client,
}

impl Fetcher {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    pub fn local() -> Self {
        let base_url = std::env::var(AGENTS_SERVER_URL_ENV)
            .unwrap_or_else(|_| format!("http://{DEFAULT_SERVER_ADDR}"));
        Self::new(base_url)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn check_health(&self) -> Result<HealthResponse, FetchError> {
        self.rpc_call(RpcRequest::method(RPC_HEALTH_ID, RPC_HEALTH_CHECK))
            .await
    }

    pub async fn sessions_list(&self) -> Result<Vec<SessionSummary>, FetchError> {
        self.rpc_call(RpcRequest::method(RPC_SESSIONS_LIST_ID, RPC_SESSIONS_LIST))
            .await
    }

    pub async fn sessions_get(&self, id: &str) -> Result<Vec<UiMessage>, FetchError> {
        self.rpc_call(
            RpcRequest::method(RPC_SESSIONS_GET_ID, RPC_SESSIONS_GET)
                .with_params(serde_json::json!({ "id": id })),
        )
        .await
    }

    pub async fn tools_result(
        &self,
        params: ToolResultParams,
    ) -> Result<ToolResultAck, FetchError> {
        self.rpc_call(
            RpcRequest::method(RPC_TOOLS_RESULT_ID, RPC_TOOLS_RESULT).with_params(
                serde_json::to_value(params).map_err(|e| FetchError::Decode(e.to_string()))?,
            ),
        )
        .await
    }

    pub async fn tools_list(&self) -> Result<ToolsListResponse, FetchError> {
        self.rpc_call(RpcRequest::method(RPC_TOOLS_LIST_ID, RPC_TOOLS_LIST))
            .await
    }

    pub async fn tools_management(&self) -> Result<ToolsManagementResponse, FetchError> {
        self.rpc_call(RpcRequest::method(
            RPC_TOOLS_MANAGEMENT_ID,
            RPC_TOOLS_MANAGEMENT,
        ))
        .await
    }

    pub async fn tools_save_draft(
        &self,
        params: SaveDraftParams,
    ) -> Result<ToolVersionRow, FetchError> {
        self.rpc_call(
            RpcRequest::method(RPC_TOOLS_SAVE_DRAFT_ID, RPC_TOOLS_SAVE_DRAFT).with_params(
                serde_json::to_value(params).map_err(|e| FetchError::Decode(e.to_string()))?,
            ),
        )
        .await
    }

    pub async fn tools_register(&self, version_id: String) -> Result<ToolVersionRow, FetchError> {
        self.rpc_call(
            RpcRequest::method(RPC_TOOLS_REGISTER_ID, RPC_TOOLS_REGISTER).with_params(
                serde_json::to_value(RegisterParams { version_id })
                    .map_err(|e| FetchError::Decode(e.to_string()))?,
            ),
        )
        .await
    }

    pub async fn tools_delete_version(&self, version_id: String) -> Result<DeleteAck, FetchError> {
        self.rpc_call(
            RpcRequest::method(RPC_TOOLS_DELETE_VERSION_ID, RPC_TOOLS_DELETE_VERSION).with_params(
                serde_json::to_value(DeleteVersionParams { version_id })
                    .map_err(|e| FetchError::Decode(e.to_string()))?,
            ),
        )
        .await
    }

    pub async fn tools_delete_tool(&self, tool_id: String) -> Result<DeleteAck, FetchError> {
        self.rpc_call(
            RpcRequest::method(RPC_TOOLS_DELETE_TOOL_ID, RPC_TOOLS_DELETE_TOOL).with_params(
                serde_json::to_value(DeleteToolParams { tool_id })
                    .map_err(|e| FetchError::Decode(e.to_string()))?,
            ),
        )
        .await
    }

    pub async fn author_stream(
        &self,
        req: AuthorRequest,
    ) -> Result<impl Stream<Item = Result<ChatStreamEvent, FetchError>>, FetchError> {
        let response = self
            .http
            .post(format!("{}{AUTHOR_STREAM_PATH}", self.base_url))
            .json(&req)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(FetchError::Decode(format!("HTTP {status}: {body}")));
        }
        Ok(sse_event_stream(response.bytes_stream()))
    }

    pub async fn chat_stream(
        &self,
        session_id: &str,
        message: &str,
        mode: AgentMode,
    ) -> Result<impl Stream<Item = Result<ChatStreamEvent, FetchError>>, FetchError> {
        let response = self
            .http
            .post(format!("{}{CHAT_STREAM_PATH}", self.base_url))
            .json(&ChatRequest {
                session_id: session_id.to_owned(),
                message: message.to_owned(),
                mode,
            })
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(FetchError::Decode(format!("HTTP {status}: {body}")));
        }

        Ok(sse_event_stream(response.bytes_stream()))
    }

    async fn rpc_call<T>(&self, req: RpcRequest) -> Result<T, FetchError>
    where
        T: DeserializeOwned,
    {
        let response: RpcResponse<T> = self
            .http
            .post(format!("{}{}", self.base_url, RPC_PATH))
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if let Some(error) = response.error {
            return Err(FetchError::Rpc(error));
        }

        response.result.ok_or(FetchError::MissingResult)
    }
}

fn sse_event_stream<S, B>(bytes: S) -> impl Stream<Item = Result<ChatStreamEvent, FetchError>>
where
    S: Stream<Item = reqwest::Result<B>> + Unpin,
    B: AsRef<[u8]>,
{
    stream::unfold(
        (bytes, String::new()),
        |(mut bytes_stream, mut buf)| async move {
            loop {
                if let Some(pos) = buf.find("\n\n") {
                    let event = buf[..pos].to_string();
                    buf.drain(..pos + 2);
                    for line in event.lines() {
                        if let Some(json) = line.strip_prefix("data: ") {
                            match serde_json::from_str::<ChatStreamEvent>(json) {
                                Ok(event) => return Some((Ok(event), (bytes_stream, buf))),
                                Err(e) => {
                                    return Some((
                                        Err(FetchError::Decode(e.to_string())),
                                        (bytes_stream, buf),
                                    ));
                                }
                            }
                        }
                    }
                    continue;
                }
                match bytes_stream.next().await {
                    None => return None,
                    Some(Err(e)) => {
                        return Some((Err(FetchError::Http(e)), (bytes_stream, buf)));
                    }
                    Some(Ok(b)) => buf.push_str(&String::from_utf8_lossy(b.as_ref())),
                }
            }
        },
    )
}
