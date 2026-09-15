pub mod admin;
pub mod api;
pub mod auth;
pub mod channel;
pub mod chat;
pub mod config;
pub mod health;
pub mod login;
pub mod peers;
pub mod state;
pub mod tasks;

use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use state::App;
use std::sync::Arc;

#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
struct Assets;

async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> impl IntoResponse {
    match Assets::get(&path) {
        Some(f) => {
            let mime = match path.rsplit('.').next() {
                Some("css") => "text/css",
                Some("js") | Some("mjs") => "application/javascript",
                Some("html") => "text/html",
                Some("svg") => "image/svg+xml",
                Some("json") | Some("map") => "application/json",
                Some("woff2") => "font/woff2",
                Some("png") => "image/png",
                Some("ico") => "image/x-icon",
                Some("txt") => "text/plain",
                _ => "application/octet-stream",
            };
            // no-cache: embedded assets change on upgrade — never let the
            // browser serve a stale console JS/CSS across deploys.
            (
                [
                    (axum::http::header::CONTENT_TYPE, mime),
                    (axum::http::header::CACHE_CONTROL, "no-cache"),
                ],
                f.data,
            )
        }
        None => (
            [
                (axum::http::header::CONTENT_TYPE, "text/plain"),
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            std::borrow::Cow::Borrowed(&b"not found"[..]),
        ),
    }
}

/// Serve the SPA shell for every console route. The client router decides
/// what to render; data comes from /api.
async fn spa() -> Response {
    match Assets::get("index.html") {
        Some(f) => (
            [
                (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (axum::http::header::CACHE_CONTROL, "no-cache"),
            ],
            f.data,
        )
            .into_response(),
        None => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "console UI not built — run `npm --prefix ui ci && npm --prefix ui run build`",
        )
            .into_response(),
    }
}

pub fn router(app: Arc<App>) -> axum::Router {
    let admin_ui = Router::new()
        // live feeds
        .route("/api/events", get(admin::sse_events))
        .route("/api/topology", get(admin::topology_data))
        // auth + instance info
        .route("/api/auth/ok", get(login::auth_ok))
        // SPA console API
        .route("/api/summary", get(api::summary))
        .route("/api/peers", get(api::peers_list))
        .route("/api/peers/{name}", get(api::peer_detail))
        .route("/api/peers/{name}/accept", post(api::accept_peer_json))
        .route("/api/peers/{name}/reject", post(api::reject_peer_json))
        .route("/api/peers/{name}/revoke", post(api::revoke_peer_json))
        .route("/api/peers/{name}/delete", post(api::delete_peer_json))
        .route("/api/logs", get(api::logs))
        .route("/api/notifications", get(api::notifications))
        .route("/api/settings", get(api::settings_json))
        .route("/api/settings/password", post(api::set_password_json))
        .route("/api/settings/humans", post(api::create_human_json))
        .route(
            "/api/settings/humans/{name}/delete",
            post(api::delete_human_json),
        )
        .route(
            "/api/settings/bootstrap/regenerate",
            post(api::regenerate_bootstrap_json),
        )
        .route("/api/tasks", get(tasks::list))
        .route("/api/tasks/{id}", get(tasks::detail))
        .route("/api/tasks/{id}/reply", post(tasks::reply))
        .route("/api/tasks/{id}/cancel", post(tasks::cancel))
        .route("/api/chat/state", get(chat::api_state))
        .route("/api/chat/messages", get(chat::api_messages))
        .route("/api/chat/history", get(chat::api_history))
        .route("/api/chat/send", post(chat::api_send))
        .route("/api/chat/typing", post(chat::api_typing))
        .route("/api/chat/rooms", post(chat::api_rooms_create))
        .route(
            "/api/chat/rooms/{id}/members",
            post(chat::api_rooms_members),
        )
        .route("/api/chat/rooms/{id}/delete", post(chat::api_room_delete))
        // SPA shell for all console routes
        .route("/", get(spa))
        .route("/peers", get(spa))
        .route("/peers/{name}", get(spa))
        .route("/tasks", get(spa))
        .route("/logs", get(spa))
        .route("/logs/full", get(spa))
        .route("/chat", get(spa))
        .route("/settings", get(spa))
        // still server-rendered: the JSONL export download
        .route("/logs/export", get(admin::logs_export))
        // legacy form actions (pre-SPA, kept for scripts/tests until removal)
        .route("/settings/password", post(admin::set_password))
        .route("/settings/humans", post(chat::create_human))
        .route("/settings/humans/{name}/delete", post(chat::delete_human))
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
        .route(
            "/register",
            post(peers::register)
                .patch(peers::update)
                .delete(peers::deregister),
        )
        .route("/.well-known/agent.json", get(peers::agent_card))
        .route("/.well-known/agent-card.json", get(peers::agent_card))
        .route("/peer/{name}", axum::routing::any(peers::proxy))
        // ponytail: empty-rest match; axum 0.8 wildcard does not match "" so
        // `/peer/name/` (agent-card URLs end in `/`) fell through to admin 303
        .route("/peer/{name}/", axum::routing::any(peers::proxy))
        .route("/peer/{name}/{*rest}", axum::routing::any(peers::proxy))
        .route("/channel", get(channel::channel_open))
        .route("/channel/response/{id}", post(channel::channel_response))
        // token-gated, NOT behind require_admin (metrics does its own auth)
        .route("/metrics", get(admin::metrics))
        .route("/login", get(spa).post(login::login))
        // JSON logout: dropping an invalid session is harmless, so no gate here
        .route("/api/logout", post(login::api_logout))
        // login lives OUTSIDE the gated router: it *creates* the session
        .route("/api/login", post(login::api_login))
        .route("/logout", post(login::logout))
        .route("/assets/{*path}", get(asset))
        .merge(admin_ui)
        .with_state(app)
}
