use crate::auth::{classify_token, extract_token, forbidden, too_many, unauthorized, ClientIp, TokenKind};
use crate::state::{fingerprint, now, validate_url, AppState, Peer, PeerState};
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

const MAX_PROXY_BYTES: usize = 4 * 1024 * 1024;
// Must exceed pi-a2a replyTimeoutSec (default 300s) — agent tasks run long.
const PROXY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

fn err(status: StatusCode, msg: &str) -> Response {
    (status, msg.to_string()).into_response()
}

#[derive(Deserialize)]
pub struct RegBody {
    name: String,
    url: String,
    #[serde(default)]
    card: Option<serde_json::Value>,
    #[serde(default)]
    upstream_token: Option<String>,
}

/// POST /register
pub async fn register(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(reg): Json<RegBody>,
) -> Response {
    if !app.limiter.allow(&client_ip, 20) {
        return too_many();
    }
    let Some(token) = extract_token(&headers) else { return unauthorized() };
    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    let Some(kind) = classify_token(&token, &gateway, &bootstrap) else { return unauthorized() };

    let name = reg.name.trim().to_string();
    if name.is_empty() || name.len() > 64
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return err(StatusCode::UNPROCESSABLE_ENTITY, "name must be 1-64 chars of [a-zA-Z0-9._-]");
    }
    if let Err(e) = validate_url(&reg.url) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, &format!("url: {e}"));
    }
    if let Some(t) = reg.upstream_token.as_deref() {
        if t.len() > 512 {
            return err(StatusCode::UNPROCESSABLE_ENTITY, "upstream_token too long");
        }
    }
    if let Some(c) = reg.card.as_ref() {
        if serde_json::to_string(c).map(|s| s.len()).unwrap_or(usize::MAX) > 256 * 1024 {
            return err(StatusCode::PAYLOAD_TOO_LARGE, "card too large");
        }
    }

    let fp = fingerprint(&token);
    let mut inner = app.inner.write().await;
    if let Some(idx) = inner.peers.iter().position(|p| p.name == name) {
        let existing = &inner.peers[idx];
        if existing.fingerprint != fp {
            return err(StatusCode::CONFLICT, "peer name already registered by another identity");
        }
        // Same identity re-registering: refresh url/card/upstream token, keep admission state.
        let peer = Peer {
            url: reg.url.clone(),
            card: reg.card.clone().unwrap_or_else(|| existing.card.clone()),
            upstream_token: reg.upstream_token.clone().or_else(|| existing.upstream_token.clone()),
            ..existing.clone()
        };
        inner.peers[idx] = peer;
        drop(inner);
        app.persist().await;
        return (StatusCode::OK, Json(serde_json::json!({"status": "updated", "peer": name, "state": "accepted"}))).into_response();
    }

    let state = match kind {
        TokenKind::Bootstrap => PeerState::Accepted,
        TokenKind::Gateway => PeerState::Pending,
    };
    inner.peers.push(Peer {
        name: name.clone(),
        url: reg.url.clone(),
        card: reg.card.unwrap_or(serde_json::Value::Null),
        state,
        fingerprint: fp,
        upstream_token: reg.upstream_token,
        registered_at: now(),
        last_seen: None,
        healthy: None,
        last_error: None,
        auto_accepted: kind == TokenKind::Bootstrap,
    });
    drop(inner);
    app.persist().await;
    let s = if state == PeerState::Accepted { "accepted" } else { "pending" };
    (StatusCode::CREATED, Json(serde_json::json!({"status": "registered", "peer": name, "state": s}))).into_response()
}

/// DELETE /register — deregister the calling peer (matched by token fingerprint + name).
pub async fn deregister(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Query(q): Query<DeregQuery>,
) -> Response {
    if !app.limiter.allow(&client_ip, 20) {
        return too_many();
    }
    let Some(token) = extract_token(&headers) else { return unauthorized() };
    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    if classify_token(&token, &gateway, &bootstrap).is_none() {
        return unauthorized();
    }
    let fp = fingerprint(&token);
    let mut inner = app.inner.write().await;
    let before = inner.peers.len();
    inner.peers.retain(|p| !(p.name == q.name && p.fingerprint == fp));
    let removed = before - inner.peers.len();
    drop(inner);
    app.persist().await;
    if removed == 0 {
        return err(StatusCode::NOT_FOUND, "no such peer registered by this identity");
    }
    (StatusCode::OK, Json(serde_json::json!({"status": "deregistered", "peer": q.name}))).into_response()
}

#[derive(Deserialize)]
pub struct DeregQuery {
    pub name: String,
}

/// Stable label for a caller in the routing log. With a shared token this is
/// token-class attribution only (see plan: per-peer tokens are the v2 upgrade).
pub fn caller_label(token: &str, gateway: &str, bootstrap: &str) -> String {
    match classify_token(token, gateway, bootstrap) {
        Some(TokenKind::Bootstrap) => "bootstrap".to_string(),
        _ => format!("client-{}", fingerprint(token).get(..8).unwrap_or("")),
    }
}

/// GET /.well-known/agent.json (+ v1.0 alias) — gateway card + accepted-peer directory.
/// Auth-aware: capabilities/skills only shown to token holders; pending/revoked never listed.
pub async fn agent_card(State(app): State<AppState>, headers: HeaderMap) -> Response {
    let inner = app.inner.read().await;
    let token = extract_token(&headers);
    let authed = token
        .as_deref()
        .and_then(|t| classify_token(t, &inner.gateway_token, &inner.bootstrap_token))
        .is_some();

    let peers: Vec<serde_json::Value> = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Accepted)
        .map(|p| {
            let mut v = serde_json::json!({
                "name": p.name,
                "url": format!("/peer/{}/", p.name),
                "healthy": p.healthy,
                "channel": app.channels.has(&p.name),
            });
            if authed {
                v["capabilities"] = p.card.get("capabilities").cloned().unwrap_or(serde_json::Value::Null);
                v["skills"] = p.card.get("skills").cloned().unwrap_or(serde_json::Value::Null);
            }
            v
        })
        .collect();

    Json(serde_json::json!({
        "name": "agent-gateway",
        "description": "Self-hosted A2A gateway: peer admission, directory and routing",
        "version": env!("CARGO_PKG_VERSION"),
        "protocolVersion": "0.1.0",
        "url": "/",
        "capabilities": { "streaming": true, "pushNotifications": false },
        "defaultInputModes": ["text"],
        "defaultOutputModes": ["text"],
        "securitySchemes": { "bearer": { "type": "http", "scheme": "bearer" } },
        "security": [{ "bearer": [] }],
        "peers": peers,
    }))
    .into_response()
}

/// ANY /peer/{name}/{*rest} — reverse proxy to the peer's pinned URL (deny-by-default egress).
#[allow(clippy::too_many_arguments)]
pub async fn proxy(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    uri: Uri,
    headers: HeaderMap,
    method: axum::http::Method,
    body: Bytes,
) -> Response {
    if !app.limiter.allow(&client_ip, 120) {
        return too_many();
    }
    // /peer/{name}[/rest…] — parse the name from the raw path; works for both
    // the single-segment and wildcard routes without a Path extractor.
    let name = uri
        .path()
        .strip_prefix("/peer/")
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let Some(token) = extract_token(&headers) else { return unauthorized() };
    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    if classify_token(&token, &gateway, &bootstrap).is_none() {
        return unauthorized();
    }

    let (url, upstream_token, state) = {
        let inner = app.inner.read().await;
        match inner.peers.iter().find(|p| p.name == name) {
            Some(p) => (p.url.clone(), p.upstream_token.clone(), p.state),
            None => return err(StatusCode::NOT_FOUND, "unknown peer"),
        }
    };
    if state != PeerState::Accepted {
        return forbidden();
    }
    if body.len() > MAX_PROXY_BYTES {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "body too large");
    }

    // Dual-mode: firewalled peers hold a reverse channel — deliver there.
    if app.channels.has(&name) {
        return channel_roundtrip(app, name, token, uri, method, headers, body).await;
    }

    // /peer/{name}/{rest} → {pinned-url}/{rest}; query string preserved.
    let rest_path = match uri.path().strip_prefix(&format!("/peer/{name}")) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => "/".to_string(),
    };
    let target = format!("{}{}{}", url.trim_end_matches('/'), rest_path, uri.query().map(|q| format!("?{q}")).unwrap_or_default());

    let src = caller_label(&token, &gateway, &bootstrap);
    let started = std::time::Instant::now();
    let method_s = method.to_string();

    let mut req = app
        .http
        .request(
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
            &target,
        )
        .timeout(PROXY_TIMEOUT);
    if let Some(ut) = upstream_token {
        req = req.bearer_auth(ut);
    }
    const SKIP: [&str; 6] = ["connection", "keep-alive", "transfer-encoding", "upgrade", "authorization", "x-gateway-token"];
    for (k, v) in headers.iter() {
        let lower = k.as_str().to_lowercase();
        if SKIP.contains(&lower.as_str()) || lower == "host" || lower == "content-length" {
            continue;
        }
        if let Ok(vs) = v.to_str() {
            req = req.header(&lower, vs);
        }
    }
    let body_len = body.len();
    if !body.is_empty() {
        req = req.body(body);
    }

    let resp = req.send().await;
    let status = match &resp {
        Ok(r) => r.status().as_u16(),
        Err(_) => 502,
    };
    app.log_route(crate::state::RouteEntry {
        ts: now(),
        src: src.clone(),
        dst: name.clone(),
        method: method_s.clone(),
        status,
        bytes: body_len as u64,
        latency_ms: started.elapsed().as_millis() as u64,
    })
    .await;

    match resp {
        Ok(upstream) => {
            let status = upstream.status();
            let mut headers = axum::http::HeaderMap::new();
            for (k, v) in upstream.headers().iter() {
                let lower = k.as_str().to_lowercase();
                if lower == "transfer-encoding" || lower == "content-length" || lower == "connection" {
                    continue;
                }
                if let (Ok(hn), Ok(Ok(hv))) = (
                    axum::http::HeaderName::from_bytes(lower.as_bytes()),
                    v.to_str().map(axum::http::HeaderValue::from_str),
                ) {
                    headers.insert(hn, hv);
                }
            }
            let bytes = upstream.bytes().await.unwrap_or_default();
            // Mark healthy + last_seen on any successful exchange.
            let mut inner = app.inner.write().await;
            if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
                p.last_seen = Some(now());
                if p.healthy != Some(true) {
                    p.healthy = Some(true);
                    p.last_error = None;
                }
            }
            drop(inner);
            (StatusCode::from_u16(status.as_u16()).unwrap(), headers, bytes).into_response()
        }
        Err(e) => {
            let mut inner = app.inner.write().await;
            if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
                p.healthy = Some(false);
                p.last_error = Some(format!("proxy: {e}"));
            }
            drop(inner);
            err(StatusCode::BAD_GATEWAY, &format!("upstream unreachable: {e}"))
        }
    }
}


/// Channel-mode proxying: wrap the request as an envelope, push it down the
/// peer's own outbound SSE stream, await the correlated response POST.
async fn channel_roundtrip(
    app: AppState,
    name: String,
    token: String,
    uri: Uri,
    method: axum::http::Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    use base64::Engine as _;
    let rest = match uri.path().strip_prefix(&format!("/peer/{name}")) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => "/".to_string(),
    };
    const SKIP: [&str; 6] = ["connection", "keep-alive", "transfer-encoding", "upgrade", "authorization", "x-gateway-token"];
    let mut fwd = std::collections::HashMap::new();
    for (k, v) in headers.iter() {
        let lower = k.as_str().to_lowercase();
        if SKIP.contains(&lower.as_str()) || lower == "host" || lower == "content-length" {
            continue;
        }
        if let Ok(vs) = v.to_str() {
            fwd.insert(lower, vs.to_string());
        }
    }
    let head = crate::channel::EnvelopeHead {
        method: method.to_string(),
        path: rest,
        query: uri.query().map(|q| q.to_string()),
        headers: fwd,
        body_b64: base64::engine::general_purpose::STANDARD.encode(&body),
    };
    let (gateway_t, bootstrap_t) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    let src = format!("channel-{}", caller_label(&token, &gateway_t, &bootstrap_t));
    let started = std::time::Instant::now();
    let method_s = method.to_string();
    let status;
    let out = if let Some(rx) = app.channels.deliver(&name, head) {
        match tokio::time::timeout(PROXY_TIMEOUT, rx).await {
            Ok(Ok(resp)) => {
                status = resp.status;
                decode_channel_resp(resp)
            }
            Ok(Err(_)) => { status = 502; err(StatusCode::BAD_GATEWAY, "peer channel closed") }
            Err(_) => { status = 504; err(StatusCode::GATEWAY_TIMEOUT, "peer channel timeout") }
        }
    } else {
        status = 502;
        err(StatusCode::BAD_GATEWAY, "peer channel send failed")
    };
    app.log_route(crate::state::RouteEntry {
        ts: now(),
        src,
        dst: name,
        method: method_s,
        status,
        bytes: body.len() as u64,
        latency_ms: started.elapsed().as_millis() as u64,
    })
    .await;
    out
}

/// Decode a channel response into an axum Response with the same header
/// filtering as the direct path.
fn decode_channel_resp(resp: crate::channel::RespEnvelope) -> Response {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&resp.body_b64)
        .unwrap_or_default();
    let mut headers = axum::http::HeaderMap::new();
    for (k, v) in &resp.headers {
        let lower = k.to_lowercase();
        if lower == "transfer-encoding" || lower == "content-length" || lower == "connection" {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            axum::http::HeaderName::from_bytes(lower.as_bytes()),
            axum::http::HeaderValue::from_str(v),
        ) {
            headers.insert(hn, hv);
        }
    }
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::BAD_GATEWAY);
    (status, headers, bytes).into_response()
}
