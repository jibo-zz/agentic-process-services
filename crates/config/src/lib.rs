//! Shared configuration loading for workspace apps.

use std::{env, net::SocketAddr};

pub const SERVER_ADDR_ENV: &str = "APS_SERVER_ADDR";
pub const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:3000";

#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    pub addr: SocketAddr,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, std::net::AddrParseError> {
        let addr = env::var(SERVER_ADDR_ENV)
            .unwrap_or_else(|_| DEFAULT_SERVER_ADDR.to_owned())
            .parse()?;

        Ok(Self { addr })
    }
}
