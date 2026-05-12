use agentic_config::DEFAULT_SERVER_ADDR;
use agentic_protocol::{
    HealthResponse, RPC_HEALTH_CHECK, RPC_PATH, RpcError, RpcRequest, RpcResponse,
};
use std::{error::Error, fmt};

#[derive(Debug)]
pub enum FetchError {
    Http(reqwest::Error),
    Rpc(RpcError),
    MissingResult,
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "HTTP request failed: {error}"),
            Self::Rpc(error) => write!(f, "RPC failed: {} ({})", error.message, error.code),
            Self::MissingResult => f.write_str("RPC response did not include a result"),
        }
    }
}

impl Error for FetchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Rpc(_) | Self::MissingResult => None,
        }
    }
}

impl From<reqwest::Error> for FetchError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

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
        let response: RpcResponse<HealthResponse> = self
            .http
            .post(format!("{}{}", self.base_url, RPC_PATH))
            .json(&RpcRequest::method(1, RPC_HEALTH_CHECK))
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
