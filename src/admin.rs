use crate::state::{now, AppState, Peer, PeerState, RouteEntry};

// set in main() — controls the non-localhost warning banner
pub static LOCALHOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
// set in main() — adds `Secure` to session cookies when TLS fronts the gateway
pub static COOKIE_SECURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_localhost() -> bool {
    LOCALHOST.load(std::sync::atomic::Ordering::Relaxed)
}

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use std::convert::Infallible;
use std::time::Duration;

/// One skill from the peer's agent card, structured for display.
#[derive(Clone, serde::Serialize)]
pub struct CardSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: String,
}

/// Structured agent-card view shared by the JSON API.
#[derive(Clone, Default, serde::Serialize)]
#[serde(default)]
pub(crate) struct CardInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub provider: String,
    pub streaming: Option<bool>,
    pub push: Option<bool>,
    pub sth: Option<bool>,
    pub skills: Vec<CardSkill>,
}

/// Parse a peer's registered card into display fields. None when the peer
/// registered cardless (null or empty card object).
pub(crate) fn card_info(peer: &Peer) -> Option<CardInfo> {
    let empty = serde_json::Map::new();
    let obj = peer.card.as_object().unwrap_or(&empty);
    if obj.is_empty() {
        return None;
    }
    let gstr = |k: &str| {
        obj.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let provider = obj
        .get("provider")
        .and_then(|v| v.as_object())
        .map(|p| {
            let org = p
                .get("organization")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let url = p.get("url").and_then(|v| v.as_str()).unwrap_or_default();
            match (org.is_empty(), url.is_empty()) {
                (true, true) => String::new(),
                (false, true) => org.to_string(),
                (true, false) => url.to_string(),
                (false, false) => format!("{org} · {url}"),
            }
        })
        .unwrap_or_default();
    let cap = |k: &str| {
        obj.get("capabilities")
            .and_then(|c| c.get(k))
            .and_then(|v| v.as_bool())
    };
    let skills = obj
        .get("skills")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    // A2A skills are objects {id,name,description,tags}; some
                    // agents post plain strings — accept both.
                    if let Some(txt) = s.as_str() {
                        return Some(CardSkill {
                            id: String::new(),
                            name: txt.to_string(),
                            description: String::new(),
                            tags: String::new(),
                        });
                    }
                    let o = s.as_object()?;
                    let gs = |k: &str| {
                        o.get(k)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string()
                    };
                    Some(CardSkill {
                        id: gs("id"),
                        name: gs("name"),
                        description: gs("description"),
                        tags: o
                            .get("tags")
                            .and_then(|v| v.as_array())
                            .map(|t| {
                                t.iter()
                                    .filter_map(|x| x.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(CardInfo {
        name: gstr("name"),
        description: gstr("description"),
        version: gstr("version"),
        provider,
        streaming: cap("streaming"),
        push: cap("pushNotifications"),
        sth: cap("stateTransitionHistory"),
        skills,
    })
}

/// Ring-derived per-peer activity: requests in the last hour + most recent ts.
pub(crate) fn peer_activity(
    ring: &std::collections::VecDeque<RouteEntry>,
    name: &str,
    now: i64,
) -> (u64, Option<i64>) {
    let mut reqs = 0u64;
    let mut last = None;
    for e in ring {
        if e.src == name || e.dst == name {
            if e.ts >= now - 3600 {
                reqs += 1;
            }
            last = Some(e.ts);
        }
    }
    (reqs, last)
}

#[derive(serde::Deserialize, Default)]
pub struct LogsQuery {
    pub src: Option<String>,
    pub dst: Option<String>,
    pub status: Option<String>,
    pub method: Option<String>,
    pub errors: Option<String>,
    pub n: Option<usize>,
}

/// GET /logs/export?<filters> — the filtered audit trail as routing.jsonl
/// lines (newest first). Machine-readable for offline retention.
pub async fn logs_export(State(app): State<AppState>, Query(q): Query<LogsQuery>) -> Response {
    let status_i: Option<u16> = q.status.as_deref().and_then(|s| s.parse().ok());
    let errors_only = q.errors.as_deref().is_some_and(|v| v == "1" || v == "true");
    let n = q.n.unwrap_or(500).clamp(1, 5000);
    let src = q.src.unwrap_or_default();
    let dst = q.dst.unwrap_or_default();
    let method = q.method.unwrap_or_default();
    let data_dir = app.data_dir.clone();
    let entries = tokio::task::spawn_blocking(move || {
        let mut out: Vec<RouteEntry> = read_routing_log(&data_dir)
            .into_iter()
            .filter(|e| {
                (src.is_empty() || e.src.contains(&src))
                    && (dst.is_empty() || e.dst.contains(&dst))
                    && status_i.map(|s| e.status == s).unwrap_or(true)
                    && (!errors_only || e.status >= 400)
                    && (method.is_empty()
                        || e.method.contains(&method)
                        || e.rpc_method.as_deref().is_some_and(|m| m.contains(&method)))
            })
            .collect();
        out.reverse();
        out.truncate(n);
        out
    })
    .await
    .unwrap_or_default();
    let mut body = String::new();
    for e in &entries {
        if let Ok(json) = serde_json::to_string(e) {
            body.push_str(&json);
            body.push('\n');
        }
    }
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/x-ndjson"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"routing.jsonl\"",
            ),
        ],
        body,
    )
        .into_response()
}

/// GET /metrics — Prometheus text format (v0.0.4). Token-gated: localhost or
/// any valid token (gateway/bootstrap, or a peer's per-peer caller token).
/// Counters are bumped in log_route; gauges read live state.
pub async fn metrics(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if !is_localhost() {
        let Some(token) = crate::auth::extract_token(&headers) else {
            return crate::auth::unauthorized();
        };
        let (gateway, bootstrap) = {
            let inner = app.inner.read().await;
            (inner.gateway_token.clone(), inner.bootstrap_token.clone())
        };
        if !crate::peers::authorized_token(&app, &token, &gateway, &bootstrap).await {
            return crate::auth::forbidden();
        }
    }
    let mut counters: Vec<(String, u64)> = {
        let m = app.metrics.lock().unwrap();
        m.iter().map(|(k, v)| (k.clone(), *v)).collect()
    };
    counters.sort();
    let (peers_by_state, channel_count) = {
        let inner = app.inner.read().await;
        let mut by_state: std::collections::BTreeMap<String, u64> = Default::default();
        for p in &inner.peers {
            *by_state.entry(p.state_str().to_string()).or_insert(0) += 1;
        }
        (by_state, app.channels.len())
    };
    // Label values come from peer names + HTTP methods — escape backslash,
    // quote, and newline per the Prometheus text exposition format.
    let esc = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    };
    let mut body = String::new();
    body.push_str("# TYPE a2a_switchboard_requests_total counter\n");
    for (key, v) in &counters {
        let parts: Vec<&str> = key.split('\t').collect();
        let (src, dst, method, status) = match parts.as_slice() {
            [s, d, m, st] => (s, d, m, st),
            _ => continue,
        };
        body.push_str(&format!(
            "a2a_switchboard_requests_total{{src=\"{}\",dst=\"{}\",method=\"{}\",status=\"{}\"}} {v}\n",
            esc(src),
            esc(dst),
            esc(method),
            status
        ));
    }
    body.push_str("# TYPE a2a_switchboard_peers gauge\n");
    for (state, n) in &peers_by_state {
        body.push_str(&format!("a2a_switchboard_peers{{state=\"{state}\"}} {n}\n"));
    }
    body.push_str("# TYPE a2a_switchboard_channels gauge\n");
    body.push_str(&format!("a2a_switchboard_channels {channel_count}\n"));
    body.push_str("# TYPE a2a_switchboard_uptime_seconds gauge\n");
    body.push_str(&format!(
        "a2a_switchboard_uptime_seconds {}\n",
        now().saturating_sub(app.started_at)
    ));
    body.push_str("# TYPE a2a_switchboard_build_info gauge\n");
    body.push_str(&format!(
        "a2a_switchboard_build_info{{version=\"{}\"}} 1\n",
        env!("CARGO_PKG_VERSION")
    ));
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

// ----- admission actions (form posts → redirect; the SPA uses /api/*) -----

pub(crate) async fn set_state(app: &AppState, name: &str, state: PeerState) {
    {
        let mut inner = app.inner.write().await;
        if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
            p.state = state;
        }
    }
    app.persist().await;
}

pub async fn accept_peer(State(app): State<AppState>, Path(name): Path<String>) -> Redirect {
    set_state(&app, &name, PeerState::Accepted).await;
    app.emit_peers("accept", &name);
    Redirect::to("/peers")
}

pub async fn reject_peer(State(app): State<AppState>, Path(name): Path<String>) -> Redirect {
    // Rejected pending peers are removed entirely (they may re-register later).
    {
        let mut inner = app.inner.write().await;
        inner
            .peers
            .retain(|p| !(p.name == name && p.state == PeerState::Pending));
    }
    app.persist().await;
    app.emit_peers("reject", &name);
    Redirect::to("/peers")
}

pub async fn revoke_peer(State(app): State<AppState>, Path(name): Path<String>) -> Redirect {
    set_state(&app, &name, PeerState::Revoked).await;
    app.emit_peers("revoke", &name);
    Redirect::to("/peers")
}

pub async fn delete_peer(State(app): State<AppState>, Path(name): Path<String>) -> Redirect {
    {
        let mut inner = app.inner.write().await;
        inner.peers.retain(|p| p.name != name);
        for r in &mut inner.rooms {
            r.members.retain(|m| m != &name);
        }
    }
    app.persist().await;
    app.emit_peers("delete", &name);
    Redirect::to("/peers")
}

#[derive(serde::Deserialize)]
pub struct PasswordForm {
    pub current: Option<String>,
    pub new: String,
    pub confirm: String,
}

/// Set or change the admin password (legacy form flavor; the SPA posts JSON
/// to /api/settings/password). A random password is generated on first run;
/// changing it always requires the current password.
pub async fn set_password(
    State(app): State<AppState>,
    axum::extract::Form(f): axum::extract::Form<PasswordForm>,
) -> Response {
    if !app.admin_set().await {
        return Redirect::to("/settings?pw=error").into_response();
    }
    if f.new != f.confirm {
        return Redirect::to("/settings?pw=mismatch").into_response();
    }
    let cur = f.current.as_deref().filter(|s| !s.is_empty());
    match app.set_admin_password(cur, &f.new).await {
        Ok(()) => Redirect::to("/settings?pw=ok"),
        Err(_) => Redirect::to("/settings?pw=error"),
    }
    .into_response()
}

pub async fn regenerate_bootstrap(State(app): State<AppState>) -> Redirect {
    app.regenerate_bootstrap().await;
    Redirect::to("/settings")
}

// ----- SSE + JSON feeds -----

/// JSON feed for the live topology: peers with health/state flags.
pub async fn topology_data(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let peers = topology_peers(&app, &inner.peers).await;
    drop(inner);
    axum::Json(serde_json::json!({
        "peers": peers,
        "total_routes": app.log_ring.read().await.len() as u64,
    }))
    .into_response()
}

pub(crate) async fn topology_peers(app: &AppState, peers: &[Peer]) -> serde_json::Value {
    // per-peer 1h request counts for the topology node labels
    let cutoff = now() - 3600;
    let mut reqs: std::collections::HashMap<String, u64> = Default::default();
    {
        let ring = app.log_ring.read().await;
        for e in ring.iter() {
            if e.ts < cutoff {
                continue;
            }
            *reqs.entry(e.src.clone()).or_insert(0) += 1;
            if e.dst != e.src {
                *reqs.entry(e.dst.clone()).or_insert(0) += 1;
            }
        }
    }
    serde_json::json!(peers
        .iter()
        .filter(|p| p.state != PeerState::Revoked)
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "state": match p.state { PeerState::Pending => "pending", PeerState::Accepted => "accepted", PeerState::Revoked => "revoked" },
                "healthy": p.healthy,
                "channel": app.channels.has(&p.name),
                "reqs_1h": reqs.get(&p.name).copied().unwrap_or(0),
            })
        })
        .collect::<Vec<_>>())
}

/// SSE stream of new routing entries (event: route) + registry/health flips
/// (event: peers) + chat (event: chat) + typing (event: chat_typing) + pings.
/// The session is re-validated every 15s regardless of event frequency, so
/// logout/expiry closes the stream (instead of streaming forever on a busy
/// gateway).
pub async fn sse_events(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let session = crate::login::session_token(&headers);
    let rx = app.log_tx.subscribe();
    let mut prx = app.peers_tx.subscribe();
    let mut crx = app.chat_tx.subscribe();
    let mut ctlrx = app.chat_ctl_tx.subscribe();
    let stream = async_stream::try_stream! {
        let mut rx = rx;
        let mut last_check = std::time::Instant::now();
        loop {
            // Revalidate the session every 15s on EVERY iteration — busy
            // event streams never hit the idle timeout, so the check must
            // not live only in the timeout branch.
            if last_check.elapsed() >= Duration::from_secs(15) {
                last_check = std::time::Instant::now();
                if app.admin_set().await
                    && session.as_deref().map(|t| !app.session_valid(t)).unwrap_or(true)
                {
                    break;
                }
            }
            let ev = tokio::select! {
                entry = rx.recv() => match entry {
                    Ok(entry) => Event::default().event("route").data(serde_json::to_string(&entry).unwrap_or_default()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                pe = prx.recv() => match pe {
                    Ok(pe) => Event::default().event("peers").data(serde_json::to_string(&pe).unwrap_or_default()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                cm = crx.recv() => match cm {
                    Ok(cm) => Event::default().event("chat").data(serde_json::to_string(&cm).unwrap_or_default()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                ct = ctlrx.recv() => match ct {
                    Ok(ct) => Event::default().event("chat_typing").data(serde_json::to_string(&ct).unwrap_or_default()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = tokio::time::sleep(Duration::from_secs(15)) => {
                    Event::default().event("ping").data(now().to_string())
                }
            };
            yield ev;
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// Read the persistent routing log (routing.jsonl) — the full audit trail,
/// not just the in-memory ring. Missing/corrupt tail lines are skipped.
pub fn read_routing_log(data_dir: &std::path::Path) -> Vec<RouteEntry> {
    let path = data_dir.join("routing.jsonl");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|l| serde_json::from_str::<RouteEntry>(l).ok())
        .collect()
}
