mod config;
mod http_client;
mod logs;
mod reload;
mod state;
mod stats;
mod twitch;
mod util;
mod web;

use std::sync::Arc;

use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::load();
    let port = config.port;
    let state = Arc::new(AppState::new(config));

    let app = web::build_router(state.clone());

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("[Website] Listening on {port}");

    // Background channel-list refresh loops start after we're already
    // accepting connections, same ordering as the original.
    reload::spawn_loops(state);

    axum::serve(
        listener,
        // The rate limiter needs the peer address, which is only available
        // to handlers when the service is made with connect info.
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}
