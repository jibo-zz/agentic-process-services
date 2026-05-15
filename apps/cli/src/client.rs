use agentic_config::DEFAULT_SERVER_ADDR;
use agentic_protocol::{
    ChatMessage, ChatStreamEvent, HealthResponse, LlmResponse, RPC_HEALTH_CHECK, RPC_LLM_GENERATE,
    RPC_PATH, RpcError, RpcRequest, RpcResponse,
};
use futures_util::{Stream, StreamExt, stream};
use serde::de::DeserializeOwned;
use std::{error::Error, fmt};

const RPC_HEALTH_ID: u64 = 1;
const RPC_LLM_ID: u64 = 2;

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
        Self::new(format!("http://{DEFAULT_SERVER_ADDR}"))
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn check_health(&self) -> Result<HealthResponse, FetchError> {
        self.rpc(RPC_HEALTH_ID, RPC_HEALTH_CHECK).await
    }

    pub async fn llm_generate(&self) -> Result<LlmResponse, FetchError> {
        self.rpc(RPC_LLM_ID, RPC_LLM_GENERATE).await
    }

    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> Result<impl Stream<Item = Result<ChatStreamEvent, FetchError>>, FetchError> {
        use agentic_protocol::{CHAT_STREAM_PATH, ChatRequest};

        let response = self
            .http
            .post(format!("{}{CHAT_STREAM_PATH}", self.base_url))
            .json(&ChatRequest {
                messages: messages.to_vec(),
            })
            .send()
            .await?
            .error_for_status()?;

        let bytes_stream = response.bytes_stream();

        Ok(stream::unfold(
            (bytes_stream, String::new()),
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
                        Some(Ok(bytes)) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                    }
                }
            },
        ))
    }

    async fn rpc<T>(&self, id: u64, method: &'static str) -> Result<T, FetchError>
    where
        T: DeserializeOwned,
    {
        let response: RpcResponse<T> = self
            .http
            .post(format!("{}{}", self.base_url, RPC_PATH))
            .json(&RpcRequest::method(id, method))
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
