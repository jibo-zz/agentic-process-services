use agentic_config::ServerConfig;
use agentic_protocol::HealthResponse;
use axum::{Json, Router, routing::get};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging once at startup; RUST_LOG can override the default.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "server=info".into()),
        )
        .init();

    // Keep the HTTP surface minimal until shared app logic is extracted.
    let app = Router::new().route("/health", get(health));
    let config = ServerConfig::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.addr).await?;

    info!(addr = %config.addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse::ok(agentic_core::SERVICE_NAME))
}

async fn shutdown_signal() {
    // Let Axum drain in-flight requests when the process is stopped.
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
