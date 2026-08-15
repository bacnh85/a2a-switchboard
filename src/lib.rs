pub mod admin;
pub mod auth;
pub mod config;
pub mod health;
pub mod peers;
pub mod state;

use axum::routing::{get, post};
use axum::Router;
use state::App;
use std::sync::Arc;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets/"]
struct Assets;

async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> impl axum::response::IntoResponse {
    match Assets::get(&path) {
        Some(f) => {
            let mime = match path.rsplit('.').next() {
                Some("css") => "text/css",
                Some("js") => "application/javascript",
                _ => "application/octet-stream",
            };
            ([(axum::http::header::CONTENT_TYPE, mime)], f.data)
        }
        None => (
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            std::borrow::Cow::Borrowed(&b"not found"[..]),
        ),
    }
}

pub fn router(app: Arc<App>) -> axum::Router {
    Router::new()
        .route("/register", post(peers::register))
        .route("/.well-known/agent.json", get(peers::agent_card))
        .route("/.well-known/agent-card.json", get(peers::agent_card))
        .route("/peer/{name}", axum::routing::any(peers::proxy))
        .route("/peer/{name}/{*rest}", axum::routing::any(peers::proxy))
        .route("/api/events", get(admin::sse_events))
        .route("/api/graph", get(admin::graph_data))
        .route("/", get(admin::dashboard))
        .route("/peers", get(admin::peers_page))
        .route("/logs", get(admin::logs_page))
        .route("/graph", get(admin::graph_page))
        .route("/settings", get(admin::settings_page))
        .route("/peers/{name}/accept", post(admin::accept_peer))
        .route("/peers/{name}/reject", post(admin::reject_peer))
        .route("/peers/{name}/revoke", post(admin::revoke_peer))
        .route("/peers/{name}/delete", post(admin::delete_peer))
        .route("/settings/bootstrap/regenerate", post(admin::regenerate_bootstrap))
        .route("/assets/{*path}", get(asset))
        .with_state(app)
}
