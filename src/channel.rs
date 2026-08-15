use crate::state::{fingerprint, AppState, PeerState};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};

/// Per-peer broadcast depth. Lagged senders now yield 503 (retryable), not a
/// dropped envelope.
const CHANNEL_CAP: usize = 256;

/// Max decoded channel body (matches MAX_PROXY_BYTES on the direct path).
pub const MAX_CHANNEL_BODY: usize = 4 * 1024 * 1024;
/// base64 size that can decode to at most MAX_CHANNEL_BODY bytes.
const MAX_B64: usize = MAX_CHANNEL_BODY * 4 / 3 + 4;

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
    /// Per-connection secret proving this envelope went down OUR channel.
    pub chan_secret: String,
}

/// One response posted back by the peer.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RespEnvelope {
    pub id: u64,
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body_b64: String,
    /// Must match the secret we embedded in the envelope (channel binding).
    pub chan_secret: String,
}

/// Correlation slot: (peer name the id was issued to, channel secret, completion sender).
struct Pending {
    peer: String,
    chan_secret: String,
    tx: oneshot::Sender<RespEnvelope>,
}

/// A live channel: bounded mpsc sender (try_send: Full = lag 503, Closed = 502)
/// + per-connection secret.
struct ChannelSlot {
    tx: mpsc::Sender<Envelope>,
    secret: String,
}

#[derive(Default)]
pub struct Channels {
    /// peer name → live channel (bound by channel_open)
    peers: RwLock<HashMap<String, ChannelSlot>>,
    /// correlation id → pending slot (single-use, removed on resolve)
    pending: Mutex<HashMap<u64, Pending>>,
    next_id: AtomicU64,
}

impl Channels {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has(&self, name: &str) -> bool {
        self.peers.read().unwrap().contains_key(name)
    }

    /// Bind (or replace) the channel for a peer. Returns the receiver, a
    /// sender clone for disconnect detection, and the per-connection secret
    /// the peer must echo on every response POST.
    pub fn bind(&self, name: &str) -> (mpsc::Receiver<Envelope>, mpsc::Sender<Envelope>, String) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAP);
        let mut secret = [0u8; 32];
        rand::rng().fill_bytes(&mut secret);
        let secret = hex(&secret);
        self.peers.write().unwrap().insert(
            name.to_string(),
            ChannelSlot {
                tx: tx.clone(),
                secret: secret.clone(),
            },
        );
        (rx, tx, secret)
    }

    /// Drop the channel; fail its pending requests immediately (502).
    pub fn drop(&self, name: &str) {
        let mut peers = self.peers.write().unwrap();
        let secret = peers.get(name).map(|s| s.secret.clone());
        peers.remove(name);
        drop(peers);
        if let Some(secret) = secret {
            self.fail_peer(name, &secret);
        }
    }

    /// Fail all pending requests for (peer, secret) — resolve NOW with 502
    /// instead of letting callers hang for the full 600s.
    fn fail_peer(&self, peer: &str, secret: &str) {
        let mut pending = self.pending.lock().unwrap();
        let doomed: Vec<u64> = pending
            .iter()
            .filter(|(_, p)| p.peer == peer && p.chan_secret == secret)
            .map(|(id, _)| *id)
            .collect();
        for id in doomed {
            if let Some(p) = pending.remove(&id) {
                let _ = p.tx.send(RespEnvelope {
                    id,
                    status: 502,
                    headers: HashMap::new(),
                    body_b64: base64_encode(b"{\"error\":\"peer channel closed\"}"),
                    chan_secret: p.chan_secret.clone(),
                });
            }
        }
    }

    /// Push a request down the channel. Returns the completion receiver.
    pub fn deliver(
        &self,
        name: &str,
        env_head: EnvelopeHead,
    ) -> Option<oneshot::Receiver<RespEnvelope>> {
        let peers = self.peers.read().unwrap();
        let slot = peers.get(name)?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (otx, orx) = oneshot::channel();
        let env = Envelope {
            id,
            method: env_head.method,
            path: env_head.path,
            query: env_head.query,
            headers: env_head.headers,
            body_b64: env_head.body_b64,
            chan_secret: slot.secret.clone(),
        };
        match slot.tx.try_send(env) {
            Ok(_) => {
                self.pending.lock().unwrap().insert(
                    id,
                    Pending {
                        peer: name.to_string(),
                        chan_secret: slot.secret.clone(),
                        tx: otx,
                    },
                );
                Some(orx)
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Slow consumer: queue full. Retryable 503, resolved now.
                self.pending.lock().unwrap().insert(
                    id,
                    Pending {
                        peer: name.to_string(),
                        chan_secret: slot.secret.clone(),
                        tx: otx,
                    },
                );
                let _ = self.resolve_err(&id, 503, "peer channel lagging");
                Some(orx)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => None,
        }
    }

    /// Resolve a pending id immediately with an error status (used for lag).
    fn resolve_err(&self, id: &u64, status: u16, msg: &str) -> bool {
        let mut pending = self.pending.lock().unwrap();
        if let Some(p) = pending.remove(id) {
            let body = format!("{{\"error\":\"{msg}\"}}");
            let _ = p.tx.send(RespEnvelope {
                id: *id,
                status,
                headers: HashMap::new(),
                body_b64: base64_encode(body.as_bytes()),
                chan_secret: p.chan_secret.clone(),
            });
            true
        } else {
            false
        }
    }

    /// Resolve a pending id. The responder must present the channel secret of
    /// the channel the request was delivered on — a shared-token peer cannot
    /// answer another peer's requests.
    pub fn respond(&self, name: &str, resp: RespEnvelope) -> bool {
        let mut pending = self.pending.lock().unwrap();
        match pending.get(&resp.id) {
            Some(p) if p.peer == name && p.chan_secret == resp.chan_secret => {
                let p = pending.remove(&resp.id).unwrap();
                p.tx.send(resp).is_ok()
            }
            _ => false,
        }
    }
}

/// Envelope minus correlation id + channel secret (allocated at deliver-time).
pub struct EnvelopeHead {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: HashMap<String, String>,
    pub body_b64: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[derive(Deserialize)]
pub struct ChannelQuery {
    pub name: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /channel?name=<peer> — SSE: peer holds this open to receive proxied
/// requests. The name is REQUIRED: with a shared token, several peers share
/// one fingerprint, so fingerprint-only lookup would bind the wrong peer.
/// The `hello` event carries the per-connection secret the peer must echo.
pub async fn channel_open(
    State(app): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ChannelQuery>,
    headers: HeaderMap,
) -> axum::response::Response {
    let Some(token) = crate::auth::extract_token(&headers) else {
        return crate::auth::unauthorized();
    };
    let fp = fingerprint(&token);
    let (name, state) = {
        let inner = app.inner.read().await;
        match inner.peers.iter().find(|p| p.name == q.name) {
            Some(p) if p.fingerprint == fp => (p.name.clone(), p.state),
            Some(_) => {
                return (StatusCode::FORBIDDEN, "peer name does not match this token")
                    .into_response()
            }
            None => return (StatusCode::NOT_FOUND, "unknown peer").into_response(),
        }
    };
    if state != PeerState::Accepted {
        return (StatusCode::FORBIDDEN, "peer is not accepted").into_response();
    }

    let (rx, tx, secret) = app.channels.bind(&name);
    tracing::info!("channel open: {name}");
    let stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(Event::default().event("hello").data(secret.clone()));
        let mut rx = rx;
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(15), rx.recv()).await {
                Ok(Some(env)) => {
                    let data = serde_json::to_string(&env).unwrap_or_default();
                    yield Ok(Event::default().event("request").data(data));
                }
                Ok(None) => break,
                Err(_) => {
                    // keepalive; the mpsc sender's is_closed() tells us the
                    // peer's SSE stream vanished (receiver dropped).
                    if tx.is_closed() {
                        break;
                    }
                    yield Ok(Event::default().event("ping").data("keepalive"));
                }
            }
        }
    };
    let cleaned = CleanupStream {
        inner: Box::pin(stream),
        app: app.clone(),
        name,
    };
    Sse::new(cleaned)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

/// Wraps the SSE stream so that ANY termination — normal end, client
/// disconnect, axum body drop — runs channel cleanup + fails pending
/// requests immediately instead of leaving callers to the 600s timeout.
struct CleanupStream<S> {
    inner: Pin<Box<S>>,
    app: AppState,
    name: String,
}

impl<S: futures_util::Stream<Item = Result<Event, Infallible>>> futures_util::Stream
    for CleanupStream<S>
{
    type Item = Result<Event, Infallible>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<S> Drop for CleanupStream<S> {
    fn drop(&mut self) {
        tracing::info!("channel drop: {}", self.name);
        self.app.channels.drop(&self.name);
    }
}

/// POST /channel/response/{id}?name=<peer> — peer posts the answer for a
/// delivered request. `name` is required and the body must carry the
/// channel secret from the request's envelope (per-connection binding).
pub async fn channel_response(
    State(app): State<AppState>,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    axum::extract::Query(q): axum::extract::Query<ChannelQuery>,
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
    let name_ok = {
        let inner = app.inner.read().await;
        match inner.peers.iter().find(|p| p.name == q.name) {
            Some(p) => p.fingerprint == fp,
            None => false,
        }
    };
    if !name_ok {
        return (StatusCode::FORBIDDEN, "peer name does not match this token").into_response();
    }
    if resp.id != id {
        return (StatusCode::UNPROCESSABLE_ENTITY, "id mismatch").into_response();
    }
    // Size guard BEFORE any decode/allocation downstream.
    if resp.body_b64.len() > MAX_B64 {
        return (StatusCode::PAYLOAD_TOO_LARGE, "channel response too large").into_response();
    }
    let ok = app.channels.respond(&q.name, resp);
    if ok {
        StatusCode::OK.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            "unknown, expired, or foreign request id",
        )
            .into_response()
    }
}
