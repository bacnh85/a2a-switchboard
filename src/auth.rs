use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::convert::Infallible;
use std::net::SocketAddr;
use subtle::ConstantTimeEq;

/// Infallible client-IP extractor: falls back to "unknown" when connect info
/// is absent (e.g. tower::ServiceExt::oneshot in tests).
pub struct ClientIp(pub String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|a| a.0.ip().to_string())
            .or_else(|| {
                parts
                    .extensions
                    .get::<SocketAddr>()
                    .map(|a| a.ip().to_string())
            })
            .unwrap_or_else(|| "unknown".into());
        Ok(ClientIp(ip))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Bootstrap token: registrant is auto-accepted.
    Bootstrap,
    /// Gateway token: registrant goes to the pending queue unless already accepted.
    Gateway,
    /// A peer's unique caller token (per-peer identity, issued at registration).
    Peer,
}

/// Extract bearer token from Authorization header or X-Gateway-Token.
pub fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        // RFC 7235: the auth-scheme is case-insensitive. A Bearer scheme with
        // an empty token yields None — a "" credential must never classify
        // (empty Persisted token fields would otherwise match trivially).
        let mut parts = v.split_whitespace();
        if parts.next().is_some_and(|s| s.eq_ignore_ascii_case("bearer")) {
            return parts
                .next()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string);
        }
    }
    headers
        .get("x-gateway-token")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

pub fn ct_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Constant-time token check against both tokens; both comparisons always run.
pub fn classify_token(token: &str, gateway: &str, bootstrap: &str) -> Option<TokenKind> {
    let is_boot = ct_eq(token, bootstrap);
    let is_gw = ct_eq(token, gateway);
    if is_boot {
        Some(TokenKind::Bootstrap)
    } else if is_gw {
        Some(TokenKind::Gateway)
    } else {
        None
    }
}

fn rpc_err(status: StatusCode, code: i64, msg: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "jsonrpc": "2.0", "id": null,
            "error": { "code": code, "message": msg }
        })),
    )
        .into_response()
}

pub fn unauthorized() -> Response {
    rpc_err(
        StatusCode::UNAUTHORIZED,
        -32001,
        "invalid or missing gateway token",
    )
}

pub fn forbidden() -> Response {
    rpc_err(StatusCode::FORBIDDEN, -32002, "peer is not accepted")
}

pub fn too_many() -> Response {
    let mut res = rpc_err(StatusCode::TOO_MANY_REQUESTS, -32003, "rate limit exceeded");
    // Fixed 60s window — tell well-behaved clients (pi-a2a, hermes) when to retry.
    res.headers_mut()
        .insert(header::RETRY_AFTER, axum::http::HeaderValue::from_static("60"));
    res
}
