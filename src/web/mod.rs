mod api;
mod meta;
mod static_files;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(api::health))
        .route("/instances", get(api::instances))
        .route("/channels", get(api::channels))
        .route("/api/{channel}", get(api::api_channel))
        .route("/api/{channel}/{user}", get(api::api_channel_user))
        .route("/rdr/{channel}", get(api::rdr_channel))
        .route("/rdr/{channel}/{user}", get(api::rdr_channel_user))
        .route("/namehistory/{user}", get(api::namehistory))
        .route("/rm/{channel}", get(api::recent_messages))
        .route("/recent-messages/{channel}", get(api::recent_messages))
        .route(
            "/api/v2/recent-messages/{channel}",
            get(api::recent_messages),
        )
        .route("/list", get(api::mirror))
        .route("/channel/{*rest}", get(api::mirror))
        .route("/channelid/{*rest}", get(api::mirror))
        .route("/meta", get(meta::meta))
        .route("/meta/contact", get(meta::contact))
        .route("/meta/status", get(meta::status))
        .fallback(static_files::fallback)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .with_state(state)
}
