use agentic_protocol::{
    HealthResponse, RPC_HEALTH_CHECK, RPC_PATH, RpcError, RpcRequest, RpcResponse,
};
use axum::{Json, Router, routing::get, routing::post};

pub type AppType = Router;

pub fn app() -> AppType {
    Router::new()
        .route("/health", get(health))
        .route(RPC_PATH, post(rpc))
}

async fn health() -> Json<HealthResponse> {
    Json(health_response())
}

async fn rpc(Json(request): Json<RpcRequest>) -> Json<RpcResponse<HealthResponse>> {
    if request.jsonrpc != "2.0" {
        return Json(RpcResponse::failure(
            request.id,
            RpcError::invalid_request(),
        ));
    }

    match request.method.as_str() {
        RPC_HEALTH_CHECK => Json(RpcResponse::success(request.id, health_response())),
        _ => Json(RpcResponse::failure(
            request.id,
            RpcError::method_not_found(request.method),
        )),
    }
}

fn health_response() -> HealthResponse {
    HealthResponse::ok(agentic_core::SERVICE_NAME)
}
