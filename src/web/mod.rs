mod api;
mod meta;
mod ratelimit;
mod static_files;

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::routing::get;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;
use crate::web::ratelimit::RateLimiter;

pub fn build_router(state: Arc<AppState>) -> Router {
    // The endpoints that fan a request out to every alive instance, and so
    // are the expensive ones to have hammered. Recent-messages is
    // deliberately not among them: Chatterino-style clients open one request
    // per joined channel on connect, which is a legitimate burst well past
    // any per-IP budget worth setting for the log lookups.
    let logs = Router::new()
        .route("/api/{channel}", get(api::api_channel))
        .route("/api/{channel}/{user}", get(api::api_channel_user))
        .route("/rdr/{channel}", get(api::rdr_channel))
        .route("/rdr/{channel}/{user}", get(api::rdr_channel_user))
        .route("/list", get(api::mirror))
        .route("/channel/{*rest}", get(api::mirror))
        .route("/channelid/{*rest}", get(api::mirror))
        .route("/channels", get(api::channels))
        .route("/instances", get(api::instances))
        .route("/namehistory/{user}", get(api::namehistory));

    let logs = match RateLimiter::new(&state.config.rate_limit) {
        // `route_layer`, not `layer`: a request that matches none of these
        // routes should fall through to the rest of the router without
        // spending a token.
        Some(limiter) => {
            logs.route_layer(middleware::from_fn_with_state(limiter, ratelimit::limit))
        }
        None => logs,
    };

    Router::new()
        .route("/health", get(api::health))
        .route("/rm/{channel}", get(api::recent_messages))
        .route("/recent-messages/{channel}", get(api::recent_messages))
        .route(
            "/api/v2/recent-messages/{channel}",
            get(api::recent_messages),
        )
        .merge(logs)
        .route("/meta", get(meta::meta))
        .route("/meta/contact", get(meta::contact))
        .route("/meta/status", get(meta::status))
        .route("/meta/search", get(meta::search))
        .fallback(static_files::fallback)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .with_state(state)
}
