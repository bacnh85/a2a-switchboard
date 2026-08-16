pub mod admin;
pub mod auth;
pub mod channel;
pub mod config;
pub mod health;
pub mod login;
pub mod peers;
pub mod state;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use state::App;
use std::sync::Arc;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets/"]
struct Assets;

async fn asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
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
    let admin_ui = Router::new()
        .route("/api/events", get(admin::sse_events))
        .route("/api/topology", get(admin::topology_data))
        .route("/", get(admin::dashboard))
        .route("/peers", get(admin::peers_page))
        .route("/logs", get(admin::logs_page))
        .route("/settings", get(admin::settings_page))
        .route("/settings/password", post(admin::set_password))
        .route("/peers/{name}/accept", post(admin::accept_peer))
        .route("/peers/{name}/reject", post(admin::reject_peer))
        .route("/peers/{name}/revoke", post(admin::revoke_peer))
        .route("/peers/{name}/delete", post(admin::delete_peer))
        .route(
            "/settings/bootstrap/regenerate",
            post(admin::regenerate_bootstrap),
        )
        .layer(middleware::from_fn_with_state(
            app.clone(),
            login::require_admin,
        ));

    Router::new()
        .route("/register", post(peers::register).delete(peers::deregister))
        .route("/.well-known/agent.json", get(peers::agent_card))
        .route("/.well-known/agent-card.json", get(peers::agent_card))
        .route("/peer/{name}", axum::routing::any(peers::proxy))
        // ponytail: empty-rest match; axum 0.8 wildcard does not match "" so
        // `/peer/name/` (agent-card URLs end in `/`) fell through to admin 303
        .route("/peer/{name}/", axum::routing::any(peers::proxy))
        .route("/peer/{name}/{*rest}", axum::routing::any(peers::proxy))
        .route("/channel", get(channel::channel_open))
        .route("/channel/response/{id}", post(channel::channel_response))
        .route("/login", get(login::login_page).post(login::login))
        .route("/logout", post(login::logout))
        .route("/assets/{*path}", get(asset))
        .merge(admin_ui)
        .with_state(app)
}
