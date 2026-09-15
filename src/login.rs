use crate::state::AppState;
use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::Deserialize;

pub const COOKIE: &str = "agw_session";

#[derive(Deserialize)]
pub struct LoginForm {
    pub password: String,
}

/// Legacy form sign-in (the SPA posts JSON to /api/login; kept for scripts).
pub async fn login(
    State(app): State<AppState>,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    Form(f): Form<LoginForm>,
) -> Response {
    if !app.limiter.allow(&format!("login-{ip}"), 5) {
        return Redirect::to("/login?error=rate").into_response();
    }
    if !app.verify_admin_password(&f.password).await {
        return Redirect::to("/login?error=wrong").into_response();
    }
    let token = app.create_session();
    session_response(Redirect::to("/"), token)
}

#[derive(Deserialize)]
pub struct JsonLogin {
    pub password: String,
}

/// GET /api/auth/ok — session probe for the SPA: 200 with instance info when
/// the session is valid (or no password is set), 401 otherwise.
pub async fn auth_ok(State(app): State<AppState>) -> Response {
    Json(serde_json::json!({
        "ok": true,
        "password_set": app.admin_set().await,
        "localhost": crate::admin::is_localhost(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .into_response()
}

/// POST /api/login {password} — JSON sign-in for the SPA (same rate limit
/// and argon2 verification as the form flow).
pub async fn api_login(
    State(app): State<AppState>,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    Json(f): Json<JsonLogin>,
) -> Response {
    if !app.limiter.allow(&format!("login-{ip}"), 5) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "too many attempts, wait a minute"})),
        )
            .into_response();
    }
    if !app.verify_admin_password(&f.password).await {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "wrong password"})),
        )
            .into_response();
    }
    let token = app.create_session();
    session_response(Json(serde_json::json!({"ok": true})), token)
}

/// POST /api/logout — drop the session (JSON flavor).
pub async fn api_logout(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(t) = session_token(&headers) {
        app.drop_session(&t);
    }
    clear_session(Json(serde_json::json!({"ok": true})))
}

pub async fn logout(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(t) = session_token(&headers) {
        app.drop_session(&t);
    }
    clear_session(Redirect::to("/login"))
}

pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|c| c.trim().strip_prefix(&format!("{COOKIE}=")))
        .next()
        .map(str::to_string)
}

pub fn session_response(resp: impl IntoResponse, token: String) -> Response {
    let resp = resp.into_response();
    let (mut parts, body) = resp.into_parts();
    let secure = if crate::admin::COOKIE_SECURE.load(std::sync::atomic::Ordering::Relaxed) {
        " Secure;"
    } else {
        ""
    };
    parts.headers.insert(
        header::SET_COOKIE,
        format!(
            "{COOKIE}={token}; HttpOnly; SameSite=Lax;{secure} Path=/; Max-Age={}",
            crate::state::SESSION_TTL
        )
        .parse()
        .unwrap(),
    );
    Response::from_parts(parts, body)
}

fn clear_session(resp: impl IntoResponse) -> Response {
    let resp = resp.into_response();
    let (mut parts, body) = resp.into_parts();
    parts.headers.insert(
        header::SET_COOKIE,
        format!("{COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
            .parse()
            .unwrap(),
    );
    Response::from_parts(parts, body)
}

/// Gate for admin pages: no-op until a password is set, then requires a session.
pub async fn require_admin(
    State(app): State<AppState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    // CSRF: state-changing requests must originate from our own origin.
    // SameSite=Lax already blocks cross-site POSTs in modern browsers; the
    // Origin check closes older browsers and exotic vectors (issue #5).
    if req.method() != axum::http::Method::GET
        && req.method() != axum::http::Method::HEAD
        && req.uri().path() != "/login"
    {
        let ok = match headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
            Some(origin) => {
                origin.starts_with('/')
                    || req
                        .headers()
                        .get(header::HOST)
                        .and_then(|h| h.to_str().ok())
                        .is_some_and(|host| origin.ends_with(&format!("://{host}")))
            }
            // No Origin header (curl, API clients) — SameSite plus session
            // auth cover these; do not break non-browser use.
            None => true,
        };
        if !ok {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    if !app.admin_set().await {
        return next.run(req).await;
    }
    let ok = headers
        .get(header::COOKIE)
        .and_then(|_| session_token(&headers))
        .map(|t| app.session_valid(&t))
        .unwrap_or(false);
    if ok {
        return next.run(req).await;
    }
    // HTML GETs get redirected to the login form; APIs/SSE get 401.
    let p = req.uri().path();
    let api = p.starts_with("/api/");
    if api {
        StatusCode::UNAUTHORIZED.into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}
