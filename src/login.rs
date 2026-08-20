use crate::state::AppState;
use askama::Template;
use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

pub const COOKIE: &str = "agw_session";

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTmpl {
    pub error: String,
}

fn login_error(msg: &str) -> Response {
    Html(
        LoginTmpl {
            error: msg.to_string(),
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}

pub async fn login_page() -> Response {
    Html(
        LoginTmpl {
            error: String::new(),
        }
        .render()
        .unwrap_or_default(),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub password: String,
}

pub async fn login(
    State(app): State<AppState>,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    Form(f): Form<LoginForm>,
) -> Response {
    if !app.limiter.allow(&format!("login-{ip}"), 5) {
        return login_error("too many attempts, wait a minute");
    }
    if !app.verify_admin_password(&f.password).await {
        return login_error("wrong password");
    }
    let token = app.create_session();
    session_response(Redirect::to("/"), token)
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
