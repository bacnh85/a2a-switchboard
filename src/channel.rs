use crate::state::{fingerprint, AppState, PeerState};
use axum::extract::Path;
use axum::response::IntoResponse;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use tokio::sync::{broadcast, oneshot};

const CHANNEL_CAP: usize = 16; // per-peer broadcast depth; slow consumer lags → dropped envelope → caller 502

/// One request pushed down a peer's channel.
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    pub id: u64,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    /// Hop-by-hop + auth headers already stripped by the proxy.
    pub headers: HashMap<String, String>,
    /// Base64 body (binary-safe over SSE text frames).
    pub body_b64: String,
}

/// One response posted back by the peer.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RespEnvelope {
    pub id: u64,
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body_b64: String,
}

/// Correlation slot: (peer name the id was issued to, completion sender).
struct Pending {
    peer: String,
    tx: oneshot::Sender<RespEnvelope>,
}

#[derive(Default)]
pub struct Channels {
    /// peer name → broadcast sender (live SSE channels)
    peers: RwLock<HashMap<String, broadcast::Sender<Envelope>>>,
    /// correlation id → pending slot (single-use, removed on resolve)
    pending: Mutex<HashMap<u64, Pending>>,
    next_id: AtomicU64,
}

impl Channels {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when the peer holds a live channel.
    pub fn has(&self, name: &str) -> bool {
        self.peers.read().unwrap().contains_key(name)
    }

    /// Bind (or replace) the channel for a peer. Returns the receiver plus a
    /// sender clone the stream uses to detect client disconnect
    /// (receiver_count()==0 → the SSE stream was dropped).
    pub fn bind(&self, name: &str) -> (broadcast::Receiver<Envelope>, broadcast::Sender<Envelope>) {
        let (tx, rx) = broadcast::channel(CHANNEL_CAP);
        self.peers.write().unwrap().insert(name.to_string(), tx.clone());
        (rx, tx)
    }

    /// Drop the channel; fail its pending requests (502 upstream closed).
    pub fn drop(&self, name: &str) {
        self.peers.write().unwrap().remove(name);
        self.pending
            .lock()
            .unwrap()
            .retain(|_, p| p.peer != name);
    }

    /// Push a request down the channel; returns the completion receiver.
    pub fn deliver(&self, name: &str, env_head: EnvelopeHead) -> Option<oneshot::Receiver<RespEnvelope>> {
        let peers = self.peers.read().unwrap();
        let tx = peers.get(name)?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (otx, orx) = oneshot::channel();
        let env = Envelope {
            id,
            method: env_head.method,
            path: env_head.path,
            query: env_head.query,
            headers: env_head.headers,
            body_b64: env_head.body_b64,
        };
        // Lagged/failed send = slow or vanished consumer → caller gets 502.
        if tx.send(env).is_err() {
            return None;
        }
        self.pending
            .lock()
            .unwrap()
            .insert(id, Pending { peer: name.to_string(), tx: otx });
        Some(orx)
    }

    /// Resolve a pending id. Only the peer the id was issued to may respond.
    pub fn respond(&self, name: &str, resp: RespEnvelope) -> bool {
        let mut pending = self.pending.lock().unwrap();
        match pending.remove(&resp.id) {
            Some(p) if p.peer == name => p.tx.send(resp).is_ok(),
            _ => false,
        }
    }
}

/// Envelope minus the correlation id (id is allocated at deliver-time).
pub struct EnvelopeHead {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: HashMap<String, String>,
    pub body_b64: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /channel — SSE: peer holds this open to receive proxied requests.
pub async fn channel_open(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> axum::response::Response {
    let Some(token) = crate::auth::extract_token(&headers) else {
        return crate::auth::unauthorized();
    };
    let fp = fingerprint(&token);
    // Resolve the peer by token fingerprint; must be Accepted to receive traffic.
    let (name, state) = {
        let inner = app.inner.read().await;
        match inner.peers.iter().find(|p| p.fingerprint == fp) {
            Some(p) => (p.name.clone(), p.state),
            None => return (StatusCode::NOT_FOUND, "no peer registered for this token").into_response(),
        }
    };
    if state != PeerState::Accepted {
        return (StatusCode::FORBIDDEN, "peer is not accepted").into_response();
    }

    let (rx, tx) = app.channels.bind(&name);
    tracing::info!("channel open: {name}");
    let app_c = app.clone();
    let name_c = name.clone();
    let stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(Event::default().event("hello").data(name_c.clone()));
        let mut rx = rx;
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(15), rx.recv()).await {
                Ok(Ok(env)) => {
                    let data = serde_json::to_string(&env).unwrap_or_default();
                    yield Ok(Event::default().event("request").data(data));
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Err(_) => {
                    // keepalive + disconnect detection: if the SSE stream is
                    // gone, our receiver dropped → receiver_count()==0.
                    if tx.receiver_count() == 0 {
                        break;
                    }
                    yield Ok(Event::default().event("ping").data("keepalive"));
                }
            }
        }
        app_c.channels.drop(&name_c);
    };
    let sse = Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)));
    // Cleanup when the stream is dropped (client disconnect included).
    sse.into_response()
}

/// POST /channel/response/{id} — peer posts the answer for a delivered request.
pub async fn channel_response(
    State(app): State<AppState>,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    Path(id): Path<u64>,
    headers: HeaderMap,
    axum::Json(resp): axum::Json<RespEnvelope>,
) -> axum::response::Response {
    if !app.limiter.allow(&ip, 120) {
        return crate::auth::too_many();
    }
    let Some(token) = crate::auth::extract_token(&headers) else {
        return crate::auth::unauthorized();
    };
    let fp = fingerprint(&token);
    let name = {
        let inner = app.inner.read().await;
        match inner.peers.iter().find(|p| p.fingerprint == fp) {
            Some(p) => p.name.clone(),
            None => return (StatusCode::NOT_FOUND, "no peer registered for this token").into_response(),
        }
    };
    if resp.id != id {
        return (StatusCode::UNPROCESSABLE_ENTITY, "id mismatch").into_response();
    }
    // respond() checks the id→peer binding internally.
    let ok = app.channels.respond(&name, resp);
    if ok {
        StatusCode::OK.into_response()
    } else {
        (StatusCode::NOT_FOUND, "unknown or expired request id").into_response()
    }
}
