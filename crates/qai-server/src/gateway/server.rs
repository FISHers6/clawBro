use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub async fn start(state: AppState, host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(super::ws_handler::ws_upgrade))
        .route(
            "/runtime/team-tools",
            post(super::team_tools_handler::invoke_team_tool),
        )
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    tracing::info!("Gateway listening on {}", bound_addr);

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("Gateway server failed");
    });

    Ok(bound_addr)
}

async fn health() -> &'static str {
    "ok"
}
