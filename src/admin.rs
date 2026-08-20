use crate::state::{now, AppState, Peer, PeerState, RouteEntry};

// set in main() — controls the non-localhost warning banner
pub static LOCALHOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
// set in main() — adds `Secure` to session cookies when TLS fronts the gateway
pub static COOKIE_SECURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_localhost() -> bool {
    LOCALHOST.load(std::sync::atomic::Ordering::Relaxed)
}
use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use std::convert::Infallible;
use std::time::Duration;

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
    pub errors: u64,
    pub avg_ms: u64,
    pub pending: u64,
    pub total_routes: u64,
    pub recent: Vec<RouteEntry>,
    pub peers_json: String,
}

#[derive(Template)]
#[template(path = "peers.html")]
pub struct PeersTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
    pub pending: Vec<Peer>,
    pub accepted: Vec<Peer>,
    pub revoked: Vec<Peer>,
}

#[derive(Template)]
#[template(path = "peer_detail.html")]
pub struct PeerDetailTmpl {
    pub title: String,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
    pub peer: Peer,
    /// reverse-channel active for this peer (Channels::has)
    pub channel: bool,
    pub registered_at: String,
    pub last_seen: String,
    /// capabilities array from the card (JSON stringified for display)
    pub capabilities: String,
    /// skills array from the card (JSON stringified for display)
    pub skills: String,
    pub card_pretty: String,
    pub traffic: Vec<RouteEntry>,
    pub traffic_total: u64,
    pub ok_count: u64,
    pub err_count: u64,
}

#[derive(Template)]
#[template(path = "logs.html")]
pub struct LogsTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
    pub entries: Vec<RouteEntry>,
    /// true when the ring is capped and routing.jsonl holds older history
    pub truncated: bool,
    /// query params echoed back into the filter form
    pub q_src: String,
    pub q_dst: String,
    pub q_status: String,
    pub q_method: String,
    /// true when ?errors=1 is active
    pub errors_only: bool,
    pub total: u64,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
    pub pw: String,
    pub gateway_token: String,
    pub bootstrap_token: String,
}

pub async fn dashboard(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let peers_json = topology_peers(&app, &inner.peers).to_string();
    let pending = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Pending)
        .count() as u64;
    drop(inner);
    // RED-style stats straight from the routing ring (last 1000 requests).
    let (total_routes, errors, latency_sum) = {
        let ring = app.log_ring.read().await;
        ring.iter().fold((0u64, 0u64, 0u64), |(t, e, l), x| {
            (t + 1, e + (x.status >= 400) as u64, l + x.latency_ms)
        })
    };
    let avg_ms = latency_sum.checked_div(total_routes).unwrap_or(0);
    let t = DashboardTmpl {
        title: "Dashboard",
        active_nav: "dashboard",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        errors,
        avg_ms,
        pending,
        total_routes,
        recent: app.recent_log(8).await,
        peers_json,
    };
    Html(t.render().unwrap_or_default()).into_response()
}

pub async fn peers_page(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let t = PeersTmpl {
        title: "Peers",
        active_nav: "peers",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        pending: inner
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Pending)
            .cloned()
            .collect(),
        accepted: inner
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Accepted)
            .cloned()
            .collect(),
        revoked: inner
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Revoked)
            .cloned()
            .collect(),
    };
    Html(t.render().unwrap_or_default()).into_response()
}

pub async fn logs_page(State(app): State<AppState>) -> Response {
    let t = LogsTmpl {
        title: "Communication log",
        active_nav: "logs",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        entries: app.recent_log(200).await,
        truncated: app.log_ring.read().await.len() >= crate::state::RING_CAP,
        q_src: String::new(),
        q_dst: String::new(),
        q_status: String::new(),
        q_method: String::new(),
        errors_only: false,
        total: app.log_ring.read().await.len() as u64,
    };
    Html(t.render().unwrap_or_default()).into_response()
}

/// GET /logs?src=&dst=&status=&method=&errors=1&n= — full audit view over
/// routing.jsonl (not just the in-memory ring). Filters are substring matches
/// on the caller/destination/method names and exact match on HTTP status.
/// `errors=1` keeps status>=400 only. `n` caps rows (default 500, max 5000).
pub async fn logs_full(State(app): State<AppState>, Query(q): Query<LogsQuery>) -> Response {
    let filters = LogFilters::from_query(q);
    let entries = filter_routing_log_spawn(&app, &filters).await;
    let total = entries.len() as u64;
    let t = LogsTmpl {
        title: "Communication log",
        active_nav: "logs",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        entries,
        truncated: false,
        q_src: filters.src.clone(),
        q_dst: filters.dst.clone(),
        q_status: filters.status.clone().unwrap_or_default(),
        q_method: filters.method.clone(),
        errors_only: filters.errors_only,
        total,
    };
    Html(t.render().unwrap_or_default()).into_response()
}

/// GET /logs/export?<filters> — the filtered audit trail as routing.jsonl
/// lines (newest first). Machine-readable for offline retention.
pub async fn logs_export(State(app): State<AppState>, Query(q): Query<LogsQuery>) -> Response {
    let filters = LogFilters::from_query(q);
    let entries = filter_routing_log_spawn(&app, &filters).await;
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

#[derive(Default, Clone)]
struct LogFilters {
    src: String,
    dst: String,
    status: Option<String>,
    method: String,
    errors_only: bool,
    n: usize,
}

impl LogFilters {
    fn from_query(q: LogsQuery) -> Self {
        Self {
            src: q.src.unwrap_or_default(),
            dst: q.dst.unwrap_or_default(),
            status: q.status.filter(|s| !s.is_empty()),
            method: q.method.unwrap_or_default(),
            errors_only: q.errors.as_deref().is_some_and(|v| v == "1" || v == "true"),
            n: q.n.unwrap_or(500).clamp(1, 5000),
        }
    }
}

/// Read routing.jsonl, apply filters, newest first, capped at `n`.
/// Off the async runtime — routing.jsonl grows unboundedly (append-only).
async fn filter_routing_log_spawn(app: &AppState, f: &LogFilters) -> Vec<RouteEntry> {
    let dir = app.data_dir.clone();
    let f = f.clone();
    tokio::task::spawn_blocking(move || filter_routing_log(&dir, &f))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("routing log filter task failed: {e}");
            Vec::new()
        })
}

fn filter_routing_log(data_dir: &std::path::Path, f: &LogFilters) -> Vec<RouteEntry> {
    let status_i: Option<u16> = f.status.as_deref().and_then(|s| s.parse().ok());
    let mut out: Vec<RouteEntry> = read_routing_log(data_dir)
        .into_iter()
        .filter(|e| {
            (f.src.is_empty() || e.src.contains(&f.src))
                && (f.dst.is_empty() || e.dst.contains(&f.dst))
                && status_i.map(|s| e.status == s).unwrap_or(true)
                && (!f.errors_only || e.status >= 400)
                && (f.method.is_empty()
                    || e.method.contains(&f.method)
                    || e.rpc_method
                        .as_deref()
                        .is_some_and(|m| m.contains(&f.method)))
        })
        .collect();
    out.reverse();
    out.truncate(f.n);
    out
}

/// GET /peers/{name} — detail page: agent card (capabilities/skills),
/// registration/liveness metadata, and per-peer traffic history.
pub async fn peer_detail(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    let peer = {
        let inner = app.inner.read().await;
        inner.peers.iter().find(|p| p.name == name).cloned()
    };
    let Some(peer) = peer else {
        return (axum::http::StatusCode::NOT_FOUND, "unknown peer").into_response();
    };
    let registered_at = crate::state::fmt_dt(peer.registered_at);
    let last_seen = peer.last_seen.map(crate::state::fmt_dt).unwrap_or_default();

    let (capabilities, skills) = match &peer.card {
        serde_json::Value::Object(m) => (
            serde_json::to_string_pretty(&m.get("capabilities")).unwrap_or_else(|_| "null".into()),
            serde_json::to_string_pretty(&m.get("skills")).unwrap_or_else(|_| "null".into()),
        ),
        _ => ("null".into(), "null".into()),
    };
    let card_pretty = serde_json::to_string_pretty(&peer.card).unwrap_or_else(|_| "{}".into());

    // per-peer traffic from routing.jsonl (peer appears as src or dst) —
    // filtered inside spawn_blocking so the async worker never blocks on I/O
    let dir = app.data_dir.clone();
    let peer_name = name.clone();
    let all = tokio::task::spawn_blocking(move || {
        read_routing_log(&dir)
            .into_iter()
            .filter(|e| e.src == peer_name || e.dst == peer_name)
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("routing log read task failed: {e}");
        Vec::new()
    });
    let traffic: Vec<RouteEntry> = all.iter().rev().take(100).cloned().collect();
    let (ok_count, err_count) = all.iter().fold((0u64, 0u64), |(ok, err), e| {
        if e.status < 400 {
            (ok + 1, err)
        } else {
            (ok, err + 1)
        }
    });
    let traffic_total = ok_count + err_count;

    let t = PeerDetailTmpl {
        title: format!("Peer · {name}"),
        active_nav: "peers",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        peer,
        channel: app.channels.has(&name),
        registered_at,
        last_seen,
        capabilities,
        skills,
        card_pretty,
        traffic,
        traffic_total,
        ok_count,
        err_count,
    };
    Html(t.render().unwrap_or_default()).into_response()
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

pub async fn settings_page(
    State(app): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let inner = app.inner.read().await;
    let t = SettingsTmpl {
        title: "Settings",
        active_nav: "settings",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        pw: q.get("pw").cloned().unwrap_or_default(),
        gateway_token: inner.gateway_token.clone(),
        bootstrap_token: inner.bootstrap_token.clone(),
    };
    drop(inner);
    Html(t.render().unwrap_or_default()).into_response()
}

#[derive(serde::Deserialize)]
pub struct PasswordForm {
    pub current: Option<String>,
    pub new: String,
    pub confirm: String,
}

/// Set or change the admin password. The first-time set-from-LAN path is
/// gone (issue #4): a random password is generated on first run; changing
/// it always requires the current password.
pub async fn set_password(State(app): State<AppState>, Form(f): Form<PasswordForm>) -> Response {
    // There is no first-set-over-HTTP path (issue #4): the initial password
    // is minted at startup and logged once. Before that exists, refuse.
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

// ----- actions (form posts → redirect) -----

async fn set_state(app: &AppState, name: &str, state: PeerState) {
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
    Redirect::to("/peers")
}

pub async fn revoke_peer(State(app): State<AppState>, Path(name): Path<String>) -> Redirect {
    set_state(&app, &name, PeerState::Revoked).await;
    Redirect::to("/peers")
}

pub async fn delete_peer(State(app): State<AppState>, Path(name): Path<String>) -> Redirect {
    {
        let mut inner = app.inner.write().await;
        inner.peers.retain(|p| p.name != name);
    }
    app.persist().await;
    Redirect::to("/peers")
}

pub async fn regenerate_bootstrap(State(app): State<AppState>) -> Redirect {
    app.regenerate_bootstrap().await;
    Redirect::to("/settings")
}

// ----- SSE + JSON feeds -----

/// JSON feed for the live topology: peers with health/state flags.
pub async fn topology_data(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    axum::Json(serde_json::json!({
        "peers": topology_peers(&app, &inner.peers),
        "total_routes": app.log_ring.read().await.len() as u64,
    }))
    .into_response()
}

fn topology_peers(app: &AppState, peers: &[Peer]) -> serde_json::Value {
    serde_json::json!(peers
        .iter()
        .filter(|p| p.state != PeerState::Revoked)
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "state": match p.state { PeerState::Pending => "pending", PeerState::Accepted => "accepted", PeerState::Revoked => "revoked" },
                "healthy": p.healthy,
                "channel": app.channels.has(&p.name),
            })
        })
        .collect::<Vec<_>>())
}

/// SSE stream of new routing entries (event: route) + pings. The session is
/// re-validated every 15s regardless of event frequency, so logout/expiry
/// closes the stream (instead of streaming forever on a busy gateway).
pub async fn sse_events(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let session = crate::login::session_token(&headers);
    let rx = app.log_tx.subscribe();
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
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
                Ok(Ok(entry)) => {
                    yield Event::default().event("route").data(serde_json::to_string(&entry).unwrap_or_default());
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
                Err(_) => yield Event::default().event("ping").data(now().to_string()),
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
