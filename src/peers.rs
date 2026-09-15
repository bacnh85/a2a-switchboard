use crate::auth::{
    classify_token, extract_token, forbidden, too_many, unauthorized, ClientIp, TokenKind,
};
use crate::state::{
    dm_conv, fingerprint, gen_token, now, validate_url, AppState, ChatMessage, Peer, PeerKind,
    PeerState,
};
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
    let Some(token) = extract_token(&headers) else {
        return unauthorized();
    };
    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    let Some(kind) = classify_token(&token, &gateway, &bootstrap) else {
        return unauthorized();
    };

    let name = reg.name.trim().to_string();
    if name == "gateway" {
        // Reserved: the built-in switchboard agent answers /peer/gateway/.
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "name 'gateway' is reserved for the switchboard agent",
        );
    }
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "name must be 1-64 chars of [a-zA-Z0-9._-]",
        );
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
        if serde_json::to_string(c)
            .map(|s| s.len())
            .unwrap_or(usize::MAX)
            > 256 * 1024
        {
            return err(StatusCode::PAYLOAD_TOO_LARGE, "card too large");
        }
    }

    let fp = fingerprint(&token);
    let mut inner = app.inner.write().await;
    if let Some(idx) = inner.peers.iter().position(|p| p.name == name) {
        let existing = &inner.peers[idx];
        // Re-registration is a management action: once a peer holds a per-peer
        // caller token, only that token may re-register it (issue #3).
        if !may_manage(&Caller::Shared(kind), existing, &fp) {
            return err(
                StatusCode::CONFLICT,
                "peer name already registered by another identity",
            );
        }
        // Same identity re-registering: refresh url/card/upstream token, keep admission state.
        // Mint the per-peer caller token if missing (peers registered before
        // the upgrade have none until their next heartbeat).
        let real_state = existing.state;
        let minted = existing.caller_token.is_none();
        let peer = Peer {
            url: reg.url.clone(),
            card: reg.card.clone().unwrap_or_else(|| existing.card.clone()),
            upstream_token: reg
                .upstream_token
                .clone()
                .or_else(|| existing.upstream_token.clone()),
            caller_token: existing.caller_token.clone().or_else(|| Some(gen_token())),
            ..existing.clone()
        };
        inner.peers[idx] = peer;
        drop(inner);
        app.persist().await;
        app.emit_peers("register", &name);
        let state_s = match real_state {
            PeerState::Pending => "pending",
            PeerState::Accepted => "accepted",
            PeerState::Revoked => "revoked",
        };
        let mut resp = serde_json::json!({"status": "updated", "peer": name, "state": state_s});
        // Only disclose the caller token when THIS call minted it (creation or
        // pre-upgrade mint); repeat heartbeats must not re-disclose the
        // per-peer credential to shared-token holders.
        if minted {
            resp["caller_token"] = serde_json::json!(inner2(&app, &name).await);
        }
        return (StatusCode::OK, Json(resp)).into_response();
    }

    let state = match kind {
        TokenKind::Bootstrap => PeerState::Accepted,
        TokenKind::Gateway => PeerState::Pending,
        // Peer caller tokens are for proxying, not registration.
        TokenKind::Peer => return unauthorized(),
    };
    inner.peers.push(Peer {
        name: name.clone(),
        kind: PeerKind::Agent,
        url: reg.url.clone(),
        card: reg.card.unwrap_or(serde_json::Value::Null),
        state,
        fingerprint: fp,
        upstream_token: reg.upstream_token,
        caller_token: Some(gen_token()),
        registered_at: now(),
        last_seen: Some(now()),
        last_ip: None,
        reg_ip: Some(client_ip),
        healthy: None,
        last_error: None,
        auto_accepted: kind == TokenKind::Bootstrap,
        last_probe_ts: None,
        last_ok_ts: None,
    });
    drop(inner);
    app.persist().await;
    app.emit_peers("register", &name);
    let s = if state == PeerState::Accepted {
        "accepted"
    } else {
        "pending"
    };
    let ct = inner2(&app, &name).await;
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"status": "registered", "peer": name, "state": s, "caller_token": ct})),
    )
        .into_response()
}

/// DELETE /register — deregister the calling peer. Once a peer holds a
/// per-peer caller token, only that token may deregister it; shared operator
/// tokens are restricted to legacy entries without a caller token (issue #3).
pub async fn deregister(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Query(q): Query<DeregQuery>,
) -> Response {
    if !app.limiter.allow(&client_ip, 20) {
        return too_many();
    }
    let Some(token) = extract_token(&headers) else {
        return unauthorized();
    };
    // Resolve the identity BEFORE the name lookup so an unknown token 401s
    // without disclosing whether the name exists.
    let Some(caller) = resolve_caller(&app, &token).await else {
        return unauthorized();
    };
    let fp = fingerprint(&token);
    let mut inner = app.inner.write().await;
    let before = inner.peers.len();
    inner
        .peers
        .retain(|p| p.name != q.name || !may_manage(&caller, p, &fp));
    let removed = before - inner.peers.len();
    drop(inner);
    app.persist().await;
    if removed == 0 {
        // Registered but not manageable by this identity: report the same
        // "not yours" outcome without distinguishing it from unknown names.
        return err(
            StatusCode::NOT_FOUND,
            "no such peer registered by this identity",
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "deregistered", "peer": q.name})),
    )
        .into_response()
}

/// PATCH /register — partial self-service update (url, card, upstream_token).
/// Auth: the original registration token (gateway/bootstrap, fingerprint must
/// match the registrant) or the peer's own caller token. Admission state is
/// never changed by PATCH; a revoked peer may not update.
#[derive(Deserialize)]
pub struct PatchBody {
    name: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    card: Option<serde_json::Value>,
    #[serde(default)]
    upstream_token: Option<String>,
}

pub async fn update(
    State(app): State<AppState>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(p): Json<PatchBody>,
) -> Response {
    if !app.limiter.allow(&client_ip, 20) {
        return too_many();
    }
    let Some(token) = extract_token(&headers) else {
        return unauthorized();
    };
    let name = p.name.trim().to_string();
    if let Some(u) = p.url.as_deref() {
        if let Err(e) = validate_url(u) {
            return err(StatusCode::UNPROCESSABLE_ENTITY, &format!("url: {e}"));
        }
    }
    if let Some(t) = p.upstream_token.as_deref() {
        if t.len() > 512 {
            return err(StatusCode::UNPROCESSABLE_ENTITY, "upstream_token too long");
        }
    }
    if let Some(c) = p.card.as_ref() {
        if serde_json::to_string(c)
            .map(|s| s.len())
            .unwrap_or(usize::MAX)
            > 256 * 1024
        {
            return err(StatusCode::PAYLOAD_TOO_LARGE, "card too large");
        }
    }

    // Resolve the token to an identity BEFORE taking the write lock (the
    // resolver acquires its own read lock) and BEFORE the name lookup: a token
    // that is neither shared nor any peer's caller_token must 401 without
    // disclosing whether the name exists (pending/revoked peers are unlisted
    // by design).
    let caller = match resolve_caller(&app, &token).await {
        Some(c) => c,
        None => return unauthorized(),
    };
    let fp = fingerprint(&token);
    let mut inner = app.inner.write().await;
    let Some(idx) = inner.peers.iter().position(|x| x.name == name) else {
        return err(StatusCode::NOT_FOUND, "no such peer");
    };
    // Authorization: the peer's own caller token, or (legacy entries without
    // one only) the exact shared token that registered them. Any other shared
    // token is a different fleet identity → 409 (issue #3).
    if !may_manage(&caller, &inner.peers[idx], &fp) {
        return err(StatusCode::CONFLICT, "peer registered by another identity");
    }
    if inner.peers[idx].state == PeerState::Revoked {
        return forbidden();
    }
    {
        let peer = &mut inner.peers[idx];
        if let Some(u) = p.url {
            peer.url = u;
        }
        if let Some(c) = p.card {
            peer.card = c;
        }
        if let Some(t) = p.upstream_token {
            peer.upstream_token = Some(t);
        }
        peer.last_seen = Some(now());
        peer.last_ip = Some(client_ip);
    }
    let state = inner.peers[idx].state;
    drop(inner);
    app.persist().await;
    let state_s = match state {
        PeerState::Pending => "pending",
        PeerState::Accepted => "accepted",
        PeerState::Revoked => "revoked",
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({"status":"updated", "peer": name, "state": state_s})),
    )
        .into_response()
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

async fn inner2(app: &AppState, name: &str) -> Option<String> {
    let inner = app.inner.read().await;
    inner
        .peers
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.caller_token.clone())
}

/// True when the token is the gateway/bootstrap token OR a peer's caller token.
pub async fn authorized_token(app: &AppState, token: &str, gateway: &str, bootstrap: &str) -> bool {
    if classify_token(token, gateway, bootstrap).is_some() {
        return true;
    }
    peer_from_token(app, token).await.is_some()
}

/// Resolve a presented token to a registered peer name via its per-peer
/// caller token (constant-time compare). None = not a peer token.
async fn peer_from_token(app: &AppState, token: &str) -> Option<String> {
    let inner = app.inner.read().await;
    for p in &inner.peers {
        if let Some(ct) = &p.caller_token {
            if crate::auth::ct_eq(ct, token) {
                return Some(p.name.clone());
            }
        }
    }
    None
}

/// Who a presented token resolves to.
pub enum Caller {
    /// A shared operator token (gateway/bootstrap) — fleet-wide, identifies
    /// NO individual peer.
    Shared(TokenKind),
    /// A specific peer's per-peer caller token.
    Peer(String),
}

/// Resolve a bearer token to a caller identity: shared operator tokens
/// (gateway/bootstrap) first, then per-peer caller tokens (constant-time).
/// None = unknown token.
pub async fn resolve_caller(app: &AppState, token: &str) -> Option<Caller> {
    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    if let Some(kind) = classify_token(token, &gateway, &bootstrap) {
        return Some(Caller::Shared(kind));
    }
    peer_from_token(app, token).await.map(Caller::Peer)
}

/// May this caller manage the peer (PATCH / DELETE / channel-open /
/// re-register)? Once a peer holds a per-peer caller token, that token is
/// the ONLY management credential: shared operator tokens are restricted to
/// legacy entries registered before caller tokens existed (and then only the
/// exact shared token that registered them). Without this restriction every
/// shared-token holder could act as any peer — the cross-peer takeover of
/// issue #3.
pub fn may_manage(caller: &Caller, peer: &Peer, fp: &str) -> bool {
    match caller {
        Caller::Peer(owner) => owner == &peer.name,
        Caller::Shared(_) => peer.caller_token.is_none() && peer.fingerprint == fp,
    }
}

/// Display-name attribution for the routing log: an `X-Gateway-Caller` header
/// (e.g. pi-a2a's selfIdentity) when the caller provides one, else the stable
/// fingerprint label. Advisory only — same trust level as the shared token.
async fn caller_display(
    app: &AppState,
    token: &str,
    gateway: &str,
    bootstrap: &str,
    header: Option<&str>,
    channel_fallback: bool,
) -> String {
    // Highest confidence: a per-peer caller token identifies the peer exactly.
    if let Some(name) = peer_from_token(app, token).await {
        return name;
    }
    match header
        .map(str::trim)
        .filter(|h| !h.is_empty() && h.len() <= 64)
    {
        Some(name) => name.to_string(),
        // Channel-delivered calls from unattributed callers keep the
        // `channel-` marker so they stay visible in the comm log.
        None if channel_fallback => format!("channel-{}", caller_label(token, gateway, bootstrap)),
        None => caller_label(token, gateway, bootstrap),
    }
}

/// GET /.well-known/agent.json (+ v1.0 alias) — gateway card + accepted-peer
/// directory. Any valid token (gateway, bootstrap, or a peer caller_token)
/// sees the full directory; unauthenticated callers get the gateway card only
/// (issue #5 — no fleet names/health/channel disclosure without a token).
pub async fn agent_card(
    State(app): State<AppState>,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    headers: HeaderMap,
) -> Response {
    // Rate-limited like every other peer-facing endpoint (issue #5).
    if !app.limiter.allow(&ip, 120) {
        return too_many();
    }
    let inner = app.inner.read().await;
    let token = extract_token(&headers);
    let (gateway, bootstrap) = (inner.gateway_token.clone(), inner.bootstrap_token.clone());
    drop(inner);
    let authed = match token.as_deref() {
        Some(t) => authorized_token(&app, t, &gateway, &bootstrap).await,
        None => false,
    };

    let inner = app.inner.read().await;
    // The fleet directory is the sensitive part (names/health/channel are
    // admission state); unauthenticated callers get an empty list (issue #5).
    let mut peers: Vec<serde_json::Value> = Vec::new();
    if authed {
        // The built-in switchboard agent answers at /peer/gateway/.
        peers.push(serde_json::json!({
            "name": "gateway",
            "url": "/peer/gateway/",
            "healthy": true,
            "channel": false,
            "capabilities": { "streaming": false },
            "skills": [],
        }));
        // Humans have no callable upstream endpoint — directory lists agents.
        peers.extend(
            inner
                .peers
                .iter()
                .filter(|p| p.state == PeerState::Accepted && p.kind == PeerKind::Agent)
                .map(|p| {
                    let mut v = serde_json::json!({
                        "name": p.name,
                        "url": format!("/peer/{}/", p.name),
                        "healthy": p.healthy,
                        "channel": app.channels.has(&p.name),
                    });
                    v["capabilities"] = p
                        .card
                        .get("capabilities")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    v["skills"] = p
                        .card
                        .get("skills")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    v
                }),
        );
    }

    Json(serde_json::json!({
        "name": "a2a-switchboard",
        "description": "Self-hosted A2A switchboard: peer admission, directory and routing",
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

/// Delivered response plus the buffered body (mirrored inside the response)
/// so internal callers (chat fanout) can extract reply text.
pub(crate) struct Delivered {
    pub resp: Response,
    pub body: Option<Vec<u8>>,
}

/// Core dual-mode delivery: the reverse-channel envelope when the peer holds
/// one, direct pinned-URL HTTP otherwise. The single RouteEntry choke point
/// for both; when `record_chat` is set and the call is an A2A message/send,
/// the exchange is mirrored into the messenger store as DM bubbles.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn deliver(
    app: &AppState,
    name: &str,
    token: &str,
    client_ip: &str,
    path: &str,
    query: Option<String>,
    method: &str,
    headers: &HeaderMap,
    body: Bytes,
    record_chat: bool,
) -> Delivered {
    let (url, upstream_token) = {
        let inner = app.inner.read().await;
        match inner.peers.iter().find(|p| p.name == name) {
            Some(p) => (p.url.clone(), p.upstream_token.clone()),
            None => {
                return Delivered {
                    resp: err(StatusCode::NOT_FOUND, "unknown peer"),
                    body: None,
                }
            }
        }
    };
    // Admission applies to internal sends exactly like proxy traffic.
    let accepted = {
        let inner = app.inner.read().await;
        inner
            .peers
            .iter()
            .find(|p| p.name == name)
            .is_some_and(|p| p.state == PeerState::Accepted)
    };
    if !accepted {
        return Delivered {
            resp: forbidden(),
            body: None,
        };
    }

    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    // Dual-mode: firewalled peers hold a reverse channel — deliver there.
    let via_channel = app.channels.has(name);
    let src = caller_display(
        app,
        token,
        &gateway,
        &bootstrap,
        headers
            .get("x-gateway-caller")
            .and_then(|v| v.to_str().ok()),
        via_channel,
    )
    .await;
    let audit = crate::state::audit_extract(&body);
    let started = std::time::Instant::now();

    let (status, resp, resp_bytes) = if via_channel {
        channel_exchange(app, name, path, query, method, headers, body.clone()).await
    } else {
        direct_exchange(
            app,
            name,
            &url,
            upstream_token,
            client_ip,
            path,
            query,
            method,
            headers,
            body.clone(),
        )
        .await
    };

    let (task_state, task_id, resp_preview) = match &resp_bytes {
        Some(b) => crate::state::audit_extract_response(b),
        None => (None, None, None),
    };
    app.log_route(crate::state::RouteEntry {
        ts: now(),
        src: src.clone(),
        dst: name.to_string(),
        method: method.to_string(),
        status,
        bytes: body.len() as u64,
        latency_ms: started.elapsed().as_millis() as u64,
        rpc_method: audit.rpc_method.clone(),
        rpc_id: audit.rpc_id,
        preview: audit.preview,
        resp_preview,
        task_state,
        task_id,
        context_id: audit.context_id,
    })
    .await;
    record_chat_roundtrip(
        app,
        record_chat,
        &src,
        name,
        audit.rpc_method.as_deref(),
        &body,
        resp_bytes.as_deref(),
        status,
    )
    .await;
    Delivered {
        resp,
        body: resp_bytes,
    }
}

/// Direct pinned-URL HTTP exchange. Marks peer health/last_seen like the old
/// inline proxy path. Returns (http status, response, buffered body).
#[allow(clippy::too_many_arguments)]
async fn direct_exchange(
    app: &AppState,
    name: &str,
    url: &str,
    upstream_token: Option<String>,
    client_ip: &str,
    path: &str,
    query: Option<String>,
    method: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> (u16, Response, Option<Vec<u8>>) {
    // /peer/{name}/{rest} → {pinned-url}/{rest}; query string preserved.
    let target = format!(
        "{}{}{}",
        url.trim_end_matches('/'),
        path,
        query.map(|q| format!("?{q}")).unwrap_or_default()
    );
    let mut req = app
        .http
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            &target,
        )
        .timeout(PROXY_TIMEOUT);
    if let Some(ut) = upstream_token {
        req = req.bearer_auth(ut);
    }
    const SKIP: [&str; 7] = [
        "connection",
        "keep-alive",
        "transfer-encoding",
        "upgrade",
        "authorization",
        "x-gateway-token",
        "x-gateway-caller",
    ];
    for (k, v) in headers.iter() {
        let lower = k.as_str().to_lowercase();
        if SKIP.contains(&lower.as_str()) || lower == "host" || lower == "content-length" {
            continue;
        }
        if let Ok(vs) = v.to_str() {
            req = req.header(&lower, vs);
        }
    }
    if !body.is_empty() {
        req = req.body(body);
    }

    match req.send().await {
        Ok(upstream) => {
            let status = upstream.status();
            let mut resp_headers = axum::http::HeaderMap::new();
            for (k, v) in upstream.headers().iter() {
                let lower = k.as_str().to_lowercase();
                if lower == "transfer-encoding"
                    || lower == "content-length"
                    || lower == "connection"
                    || lower == "set-cookie"
                    || lower == "www-authenticate"
                {
                    continue;
                }
                if let (Ok(hn), Ok(Ok(hv))) = (
                    axum::http::HeaderName::from_bytes(lower.as_bytes()),
                    v.to_str().map(axum::http::HeaderValue::from_str),
                ) {
                    resp_headers.insert(hn, hv);
                }
            }
            let bytes = upstream.bytes().await.unwrap_or_default();
            // Mark healthy + last_seen + last_ip on any successful exchange.
            let mut inner = app.inner.write().await;
            if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
                p.last_seen = Some(now());
                p.last_ip = Some(client_ip.to_string());
                if p.healthy != Some(true) {
                    p.healthy = Some(true);
                    p.last_error = None;
                }
            }
            drop(inner);
            let code = StatusCode::from_u16(status.as_u16()).unwrap();
            (
                status.as_u16(),
                (code, resp_headers, bytes.clone()).into_response(),
                Some(bytes.to_vec()),
            )
        }
        Err(e) => {
            let mut inner = app.inner.write().await;
            if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
                p.healthy = Some(false);
                p.last_error = Some(format!("proxy: {e}"));
            }
            drop(inner);
            (
                502,
                err(
                    StatusCode::BAD_GATEWAY,
                    &format!("upstream unreachable: {e}"),
                ),
                None,
            )
        }
    }
}

/// Channel-mode exchange: wrap the request as an envelope, push it down the
/// peer's own outbound SSE stream, await the correlated response POST.
#[allow(clippy::too_many_arguments)]
async fn channel_exchange(
    app: &AppState,
    name: &str,
    path: &str,
    query: Option<String>,
    method: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> (u16, Response, Option<Vec<u8>>) {
    use base64::Engine as _;
    const SKIP: [&str; 7] = [
        "connection",
        "keep-alive",
        "transfer-encoding",
        "upgrade",
        "authorization",
        "x-gateway-token",
        "x-gateway-caller",
    ];
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
        path: path.to_string(),
        query,
        headers: fwd,
        body_b64: base64::engine::general_purpose::STANDARD.encode(&body),
    };
    if let Some(rx) = app.channels.deliver(name, head) {
        match tokio::time::timeout(PROXY_TIMEOUT, rx).await {
            Ok(Ok(resp)) => {
                let status = resp.status;
                let (r, b) = decode_channel_resp(resp);
                (status, r, Some(b))
            }
            Ok(Err(_)) => (
                502,
                err(StatusCode::BAD_GATEWAY, "peer channel closed"),
                None,
            ),
            Err(_) => (
                504,
                err(StatusCode::GATEWAY_TIMEOUT, "peer channel timeout"),
                None,
            ),
        }
    } else {
        (
            502,
            err(StatusCode::BAD_GATEWAY, "peer channel send failed"),
            None,
        )
    }
}

/// Mirror an A2A message/send exchange into the messenger store as DM
/// bubbles (request from src, reply from dst). Captured traffic obeys
/// PREVIEW_ENABLED like the audit previews; failures keep the request
/// bubble with an error status and no reply.
#[allow(clippy::too_many_arguments)]
async fn record_chat_roundtrip(
    app: &AppState,
    enabled: bool,
    src: &str,
    dst: &str,
    rpc_method: Option<&str>,
    req_body: &[u8],
    resp_body: Option<&[u8]>,
    http_status: u16,
) {
    if !enabled
        || !crate::chat::is_message_send(rpc_method)
        || !*crate::state::PREVIEW_ENABLED.read().unwrap()
    {
        return;
    }
    let Some(text) = crate::chat::chat_text_request(req_body) else {
        return;
    };
    let conv = dm_conv(src, dst);
    let ok = http_status < 400;
    app.log_chat(ChatMessage {
        id: 0,
        ts: 0,
        conv: conv.clone(),
        src: src.to_string(),
        text,
        kind: "chat".into(),
        status: if ok { "ok" } else { "err" }.into(),
        error: (!ok).then(|| format!("HTTP {http_status}")),
    })
    .await;
    if let Some(rb) = resp_body {
        if let Some(reply) = crate::chat::chat_text_response(rb) {
            app.log_chat(ChatMessage {
                id: 0,
                ts: 0,
                conv,
                src: dst.to_string(),
                text: reply,
                kind: "chat".into(),
                status: "ok".into(),
                error: None,
            })
            .await;
        }
    }
}

/// ANY /peer/{name}/{*rest} — reverse proxy to the peer's pinned URL
/// (deny-by-default egress). The reserved name `gateway` is answered by the
/// built-in switchboard agent instead of being proxied.
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
    let Some(token) = extract_token(&headers) else {
        return unauthorized();
    };
    let (gateway, bootstrap) = {
        let inner = app.inner.read().await;
        (inner.gateway_token.clone(), inner.bootstrap_token.clone())
    };
    if !authorized_token(&app, &token, &gateway, &bootstrap).await {
        return unauthorized();
    }
    // Built-in switchboard agent: talk to the gateway itself.
    if name == "gateway" {
        return crate::chat::gateway_agent(State(app), headers, body).await;
    }
    if body.len() > MAX_PROXY_BYTES {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "body too large");
    }
    let rest_path = match uri.path().strip_prefix(&format!("/peer/{name}")) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => "/".to_string(),
    };
    deliver(
        &app,
        &name,
        &token,
        &client_ip,
        &rest_path,
        uri.query().map(|q| q.to_string()),
        method.as_str(),
        &headers,
        body,
        true,
    )
    .await
    .resp
}

/// Decode a channel response into an axum Response with the same header
/// filtering as the direct path; also returns the decoded body bytes for
/// response-side audit. Size-capped before decode (OOM guard).
fn decode_channel_resp(resp: crate::channel::RespEnvelope) -> (Response, Vec<u8>) {
    use base64::Engine as _;
    if resp.body_b64.len() > crate::channel::MAX_CHANNEL_BODY * 4 / 3 + 4 {
        return (
            err(StatusCode::PAYLOAD_TOO_LARGE, "channel response too large"),
            Vec::new(),
        );
    }
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&resp.body_b64) {
        Ok(b) if b.len() <= crate::channel::MAX_CHANNEL_BODY => b,
        _ => {
            return (
                err(StatusCode::PAYLOAD_TOO_LARGE, "channel response too large"),
                Vec::new(),
            )
        }
    };
    // RFC 9110 hop-by-hop + auth-challenge headers never reach the caller.
    const RESPONSE_SKIP: [&str; 8] = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
    ];
    let mut headers = axum::http::HeaderMap::new();
    for (k, v) in &resp.headers {
        let lower = k.to_lowercase();
        if RESPONSE_SKIP.contains(&lower.as_str())
            || lower == "content-length"
            || lower == "set-cookie"
            || lower == "www-authenticate"
        {
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
    ((status, headers, bytes.clone()).into_response(), bytes)
}
