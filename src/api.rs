//! Admin JSON API behind the SPA console. Every endpoint returns JSON and
//! lives under /api (require_admin already serves 401 JSON for that prefix).
//! Reuses the same state as the page handlers: the routing ring + jsonl for
//! history, the peer registry, and the task inbox.

use crate::state::{now, AppState, Peer, PeerKind, PeerState, RouteEntry};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use std::collections::HashMap;

fn err_json(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({"error": msg}))).into_response()
}

// ---------- dashboard summary ----------

#[derive(Deserialize, Default)]
pub struct SummaryQuery {
    /// "1h" (default), "6h", "24h" — bucket size is always one minute.
    pub window: Option<String>,
}

pub async fn summary(State(app): State<AppState>, Query(q): Query<SummaryQuery>) -> Response {
    let window_sec = match q.window.as_deref() {
        Some("6h") => 6 * 3600,
        Some("24h") => 24 * 3600,
        _ => 3600,
    };
    let t = now();
    let cutoff = t - window_sec;
    let nbuckets = (window_sec / 60) as usize;
    // Anchor the last bucket to the current minute so live entries never
    // fall past the end of the window.
    let bstart = t - t % 60 - (nbuckets as i64 - 1) * 60;

    // 1h reads from the ring (hot path); longer windows replay routing.jsonl
    // off the async runtime.
    let entries: Vec<RouteEntry> = if window_sec <= 3600 {
        app.log_ring.read().await.iter().cloned().collect()
    } else {
        let dir = app.data_dir.clone();
        let c = cutoff - window_sec; // include the previous window for the delta
        tokio::task::spawn_blocking(move || {
            crate::admin::read_routing_log(&dir)
                .into_iter()
                .filter(|e| e.ts >= c)
                .collect()
        })
        .await
        .unwrap_or_default()
    };

    struct Agg {
        routed: u64,
        errors: u64,
        lat: Vec<u64>,
    }
    let mut aggs: Vec<Agg> = (0..nbuckets)
        .map(|_| Agg {
            routed: 0,
            errors: 0,
            lat: Vec::new(),
        })
        .collect();
    let mut routed = 0u64;
    let mut errors = 0u64;
    let mut prev_routed = 0u64;
    let mut latencies: Vec<u64> = Vec::new();
    for e in &entries {
        if e.ts < cutoff - window_sec {
            continue;
        }
        if e.ts < cutoff {
            prev_routed += 1;
            continue;
        }
        routed += 1;
        if e.status >= 400 {
            errors += 1;
        }
        latencies.push(e.latency_ms);
        let idx = ((e.ts - bstart) / 60) as usize;
        if idx < nbuckets {
            let a = &mut aggs[idx];
            a.routed += 1;
            if e.status >= 400 {
                a.errors += 1;
            }
            // capped per-bucket sample for percentile approximation
            if a.lat.len() < 128 {
                a.lat.push(e.latency_ms);
            }
        }
    }
    latencies.sort_unstable();
    let pct = |v: &[u64], p: usize| -> Option<u64> {
        if v.is_empty() {
            None
        } else {
            Some(crate::state::percentile(v, p))
        }
    };
    let buckets: Vec<serde_json::Value> = aggs
        .iter_mut()
        .enumerate()
        .map(|(i, a)| {
            let mut lat = std::mem::take(&mut a.lat);
            lat.sort_unstable();
            serde_json::json!({
                "ts": bstart + (i as i64) * 60,
                "routed": a.routed,
                "errors": a.errors,
                "p50": pct(&lat, 50),
                "p95": pct(&lat, 95),
            })
        })
        .collect();

    let (pending, peers_total, peers_up) = {
        let inner = app.inner.read().await;
        (
            inner
                .peers
                .iter()
                .filter(|p| p.state == PeerState::Pending)
                .count() as u64,
            inner
                .peers
                .iter()
                .filter(|p| p.state != PeerState::Revoked)
                .count(),
            inner
                .peers
                .iter()
                .filter(|p| p.state != PeerState::Revoked && p.healthy == Some(true))
                .count(),
        )
    };
    let input_required = app.tasks.input_required_count().await;
    let recent = app.recent_log(30).await;
    let peers_json = {
        let inner = app.inner.read().await;
        crate::admin::topology_peers(&app, &inner.peers).await
    };

    Json(serde_json::json!({
        "window_sec": window_sec,
        "routed": routed,
        "rate_per_min": (routed + window_sec as u64 / 120) / (window_sec as u64 / 60),
        "errors": errors,
        "err_pct": errors.checked_mul(100).map_or(0, |n| n / routed.max(1)),
        "p50": pct(&latencies, 50),
        "p95": pct(&latencies, 95),
        "pending": pending,
        "peers_up": peers_up,
        "peers_total": peers_total,
        "channels": app.channels.len(),
        "input_required": input_required,
        "prev_routed": prev_routed,
        "buckets": buckets,
        "recent": recent,
        "peers": peers_json,
        "generated_at": t,
    }))
    .into_response()
}

// ---------- peers ----------

fn peer_json(app: &AppState, p: &Peer) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "kind": match p.kind { PeerKind::Agent => "agent", PeerKind::Human => "human" },
        "state": p.state_str(),
        "url": if p.url.starts_with("local://") { None } else { Some(p.url.clone()) },
        "healthy": p.healthy,
        "channel": app.channels.has(&p.name),
        "registered_at": p.registered_at,
        "last_seen": p.last_seen,
        "last_ip": p.last_ip,
        "reg_ip": p.reg_ip,
        "auto_accepted": p.auto_accepted,
        "card": if p.card.is_null() { None } else { Some(p.card.clone()) },
    })
}

/// GET /api/peers — registry with ring-derived 1h activity + per-minute
/// series for the sparkline columns.
pub async fn peers_list(State(app): State<AppState>) -> Response {
    let series = peer_series(&app).await;
    let last_seen_ts = last_activity_map(&app).await;
    let inner = app.inner.read().await;
    let pending: Vec<serde_json::Value> = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Pending)
        .map(|p| peer_json(&app, p))
        .collect();
    let accepted: Vec<serde_json::Value> = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Accepted)
        .map(|p| {
            let name = &p.name;
            serde_json::json!({
                "peer": peer_json(&app, p),
                "reqs_1h": series.get(name).map(|v| v.iter().sum::<u64>()).unwrap_or(0),
                "last_activity": last_seen_ts.get(name).copied().flatten(),
                "series": series.get(name).cloned().unwrap_or_else(|| vec![0; 60]),
            })
        })
        .collect();
    let revoked: Vec<serde_json::Value> = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Revoked)
        .map(|p| peer_json(&app, p))
        .collect();
    drop(inner);
    Json(serde_json::json!({
        "pending": pending,
        "accepted": accepted,
        "revoked": revoked,
    }))
    .into_response()
}

/// Per-peer per-minute request counts for the last hour (60 buckets, the
/// last one being the current minute).
async fn peer_series(app: &AppState) -> HashMap<String, Vec<u64>> {
    let t = now();
    let cutoff = t - 3600;
    let start = t - t % 60 - 59 * 60;
    let mut out: HashMap<String, Vec<u64>> = HashMap::new();
    let ring = app.log_ring.read().await;
    for e in ring.iter() {
        if e.ts < cutoff {
            continue;
        }
        let idx = ((e.ts - start) / 60) as usize;
        // one entry counts once per peer (not twice when src == dst)
        let mut names: Vec<&String> = vec![&e.src];
        if e.dst != e.src {
            names.push(&e.dst);
        }
        for name in names {
            let v = out.entry(name.clone()).or_insert_with(|| vec![0; 60]);
            if idx < 60 {
                v[idx] += 1;
            }
        }
    }
    out
}

async fn last_activity_map(app: &AppState) -> HashMap<String, Option<i64>> {
    let mut out: HashMap<String, Option<i64>> = HashMap::new();
    let ring = app.log_ring.read().await;
    for e in ring.iter() {
        *out.entry(e.src.clone()).or_insert(None) = Some(e.ts);
        *out.entry(e.dst.clone()).or_insert(None) = Some(e.ts);
    }
    out
}

#[derive(Deserialize, Default)]
pub struct PeerDetailQuery {
    pub dir: Option<String>,
    /// traffic rows (10..500, default 100)
    pub n: Option<usize>,
}

/// GET /api/peers/{name}?dir=&n= — identity, structured card, traffic.
pub async fn peer_detail(
    State(app): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<PeerDetailQuery>,
) -> Response {
    let peer = {
        let inner = app.inner.read().await;
        inner.peers.iter().find(|p| p.name == name).cloned()
    };
    let Some(peer) = peer else {
        return err_json(StatusCode::NOT_FOUND, "unknown peer");
    };
    let dir = match q.dir.as_deref() {
        Some("in") => "in",
        Some("out") => "out",
        _ => "",
    };
    let n = q.n.unwrap_or(100).clamp(10, 500);
    let peer_json = peer_json(&app, &peer);
    let card = crate::admin::card_info(&peer);

    let data_dir = app.data_dir.clone();
    let peer_name = name.clone();
    let dir_s = dir.to_string();
    let all = tokio::task::spawn_blocking(move || {
        crate::admin::read_routing_log(&data_dir)
            .into_iter()
            .filter(|e| match dir_s.as_str() {
                "in" => e.dst == peer_name,
                "out" => e.src == peer_name,
                _ => e.src == peer_name || e.dst == peer_name,
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let traffic: Vec<RouteEntry> = all.iter().rev().take(n).cloned().collect();
    let (ok_count, err_count) = all.iter().fold((0u64, 0u64), |(ok, err), e| {
        if e.status < 400 {
            (ok + 1, err)
        } else {
            (ok, err + 1)
        }
    });
    let (reqs_1h, last_activity) = {
        let ring = app.log_ring.read().await;
        crate::admin::peer_activity(&ring, &name, now())
    };
    Json(serde_json::json!({
        "peer": peer_json,
        "channel": app.channels.has(&name),
        "reqs_1h": reqs_1h,
        "last_activity": last_activity,
        "card": card,
        "traffic": traffic,
        "traffic_total": ok_count + err_count,
        "ok_count": ok_count,
        "err_count": err_count,
    }))
    .into_response()
}

// ---------- audit log ----------

#[derive(Deserialize, Default)]
pub struct LogsApiQuery {
    pub src: Option<String>,
    pub dst: Option<String>,
    pub status: Option<String>,
    pub method: Option<String>,
    pub errors: Option<String>,
    /// unix seconds — inclusive lower bound
    pub from: Option<String>,
    /// unix seconds — inclusive upper bound
    pub to: Option<String>,
    /// cursor: only entries strictly older than this ts
    pub before: Option<String>,
    pub n: Option<usize>,
}

/// GET /api/logs — filtered audit trail with cursor pagination. Prefers the
/// persistent routing.jsonl (full history); falls back to the ring when file
/// logging is disabled.
pub async fn logs(State(app): State<AppState>, Query(q): Query<LogsApiQuery>) -> Response {
    let src = q.src.unwrap_or_default();
    let dst = q.dst.unwrap_or_default();
    let method = q.method.unwrap_or_default();
    let status_i: Option<u16> = q.status.as_deref().and_then(|s| s.parse().ok());
    let errors_only = q.errors.as_deref().is_some_and(|v| v == "1" || v == "true");
    let from = q.from.as_deref().and_then(|s| s.parse::<i64>().ok());
    let to = q.to.as_deref().and_then(|s| s.parse::<i64>().ok());
    let before = q.before.as_deref().and_then(|s| s.parse::<i64>().ok());
    let n = q.n.unwrap_or(500).clamp(1, 5000);

    let data_dir = app.data_dir.clone();
    let mut filtered = tokio::task::spawn_blocking(move || {
        let mut file = crate::admin::read_routing_log(&data_dir);
        if file.is_empty() {
            // file logging disabled — the ring is all we have (sync snapshot)
            file = Vec::new();
        }
        file
    })
    .await
    .unwrap_or_default();
    if filtered.is_empty() {
        filtered = app.log_ring.read().await.iter().cloned().collect();
    }
    filtered.retain(|e| {
        (src.is_empty() || e.src.contains(&src))
            && (dst.is_empty() || e.dst.contains(&dst))
            && status_i.map(|s| e.status == s).unwrap_or(true)
            && (!errors_only || e.status >= 400)
            && (method.is_empty()
                || e.method.contains(&method)
                || e.rpc_method.as_deref().is_some_and(|m| m.contains(&method)))
            && from.map(|f| e.ts >= f).unwrap_or(true)
            && to.map(|t| e.ts <= t).unwrap_or(true)
    });
    // newest first
    filtered.reverse();
    let total_matched = filtered.len() as u64;
    if let Some(b) = before {
        filtered.retain(|e| e.ts < b);
    }
    let has_more = filtered.len() > n;
    filtered.truncate(n);
    let next_before_ts = if has_more {
        filtered.last().map(|e| e.ts)
    } else {
        None
    };
    Json(serde_json::json!({
        "entries": filtered,
        "total_matched": total_matched,
        "next_before_ts": next_before_ts,
    }))
    .into_response()
}

// ---------- notifications ----------

/// GET /api/notifications — operator attention feed: pending approvals,
/// unhealthy peers, tasks needing input, error spikes. Computed fresh on
/// each call (cheap over in-memory state); the bell polls + refetches on SSE.
pub async fn notifications(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let pending: Vec<String> = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Pending)
        .map(|p| p.name.clone())
        .collect();
    let unhealthy: Vec<String> = inner
        .peers
        .iter()
        .filter(|p| {
            p.kind == PeerKind::Agent && p.state == PeerState::Accepted && p.healthy == Some(false)
        })
        .map(|p| p.name.clone())
        .collect();
    drop(inner);

    let input_required = app.tasks.input_required_count().await;
    let cutoff = now() - 900;
    let recent_errors = {
        let ring = app.log_ring.read().await;
        ring.iter()
            .filter(|e| e.ts >= cutoff && e.status >= 400)
            .count()
    };

    let mut items: Vec<serde_json::Value> = Vec::new();
    if !pending.is_empty() {
        items.push(serde_json::json!({
            "kind": "pending", "severity": "warn",
            "title": format!("{} peer(s) awaiting approval", pending.len()),
            "detail": pending.join(", "),
            "href": "/peers",
            "count": pending.len(),
        }));
    }
    if !unhealthy.is_empty() {
        items.push(serde_json::json!({
            "kind": "unhealthy", "severity": "bad",
            "title": format!("{} agent(s) unreachable", unhealthy.len()),
            "detail": unhealthy.join(", "),
            "href": "/peers",
            "count": unhealthy.len(),
        }));
    }
    if input_required > 0 {
        items.push(serde_json::json!({
            "kind": "input_required", "severity": "warn",
            "title": format!("{input_required} task(s) need your input"),
            "detail": "agents are waiting for an operator answer",
            "href": "/tasks?state=input-required",
            "count": input_required,
        }));
    }
    if recent_errors >= 5 {
        items.push(serde_json::json!({
            "kind": "error_spike", "severity": "bad",
            "title": format!("{recent_errors} errors in the last 15 min"),
            "detail": "recent routed traffic is failing — check the audit log",
            "href": "/logs?errors=1",
            "count": recent_errors,
        }));
    }
    let total: u64 = items.iter().map(|i| i["count"].as_u64().unwrap_or(1)).sum();
    Json(serde_json::json!({ "items": items, "total": total })).into_response()
}

// ---------- settings ----------

/// GET /api/settings — tokens + human operators (admin-only surface, same
/// trust level as the settings page it replaces).
pub async fn settings_json(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let humans: Vec<serde_json::Value> = inner
        .peers
        .iter()
        .filter(|p| p.kind == PeerKind::Human)
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "token": p.caller_token.clone().unwrap_or_default(),
                "registered_at": p.registered_at,
            })
        })
        .collect();
    let password_set = app.admin_set().await;
    Json(serde_json::json!({
        "gateway_token": inner.gateway_token,
        "bootstrap_token": inner.bootstrap_token,
        "humans": humans,
        "password_set": password_set,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct PasswordBody {
    pub current: Option<String>,
    pub new: String,
    pub confirm: String,
}

/// POST /api/settings/password {current?, new, confirm}
pub async fn set_password_json(
    State(app): State<AppState>,
    Json(f): Json<PasswordBody>,
) -> Response {
    if !app.admin_set().await {
        return err_json(StatusCode::CONFLICT, "no admin password exists yet");
    }
    if f.new != f.confirm {
        return err_json(StatusCode::UNPROCESSABLE_ENTITY, "passwords don't match");
    }
    if f.new.chars().count() < 8 {
        return err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "password must be at least 8 characters",
        );
    }
    let cur = f.current.as_deref().filter(|s| !s.is_empty());
    match app.set_admin_password(cur, &f.new).await {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(_) => err_json(StatusCode::UNAUTHORIZED, "current password is wrong"),
    }
}

#[derive(Deserialize)]
pub struct HumanBody {
    pub name: String,
}

/// POST /api/settings/humans {name} → {ok, token} — token shown once.
pub async fn create_human_json(State(app): State<AppState>, Json(b): Json<HumanBody>) -> Response {
    match crate::chat::create_human_core(&app, &b.name).await {
        Ok(token) => Json(serde_json::json!({"ok": true, "token": token})).into_response(),
        Err("duplicate") => err_json(StatusCode::CONFLICT, "that name is already taken"),
        Err(_) => err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            "name must be 1-64 chars: letters, digits, - _ . (not 'gateway')",
        ),
    }
}

/// POST /api/settings/humans/{name}/delete
pub async fn delete_human_json(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    let removed = {
        let mut inner = app.inner.write().await;
        let idx = inner
            .peers
            .iter()
            .position(|p| p.name == name && p.kind == PeerKind::Human);
        idx.map(|i| inner.peers.remove(i))
    };
    match removed {
        Some(p) => {
            // also drop them from every room roster
            {
                let mut inner = app.inner.write().await;
                for r in &mut inner.rooms {
                    r.members.retain(|m| m != &p.name);
                }
            }
            app.persist().await;
            app.emit_peers("delete", &p.name);
            Json(serde_json::json!({"ok": true})).into_response()
        }
        None => err_json(StatusCode::NOT_FOUND, "no such operator"),
    }
}

/// POST /api/settings/bootstrap/regenerate → {token}
pub async fn regenerate_bootstrap_json(State(app): State<AppState>) -> Response {
    let token = app.regenerate_bootstrap().await;
    Json(serde_json::json!({"ok": true, "token": token})).into_response()
}

// ---------- peer admission actions ----------

fn action_guard(name: &str) -> Response {
    Json(serde_json::json!({"ok": true, "peer": name})).into_response()
}

/// POST /api/peers/{name}/accept|reject|revoke|delete — JSON admission actions.
pub async fn accept_peer_json(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    crate::admin::set_state(&app, &name, PeerState::Accepted).await;
    app.emit_peers("accept", &name);
    action_guard(&name)
}

pub async fn reject_peer_json(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    {
        let mut inner = app.inner.write().await;
        inner
            .peers
            .retain(|p| !(p.name == name && p.state == PeerState::Pending));
    }
    app.persist().await;
    app.emit_peers("reject", &name);
    action_guard(&name)
}

pub async fn revoke_peer_json(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    crate::admin::set_state(&app, &name, PeerState::Revoked).await;
    app.emit_peers("revoke", &name);
    action_guard(&name)
}

pub async fn delete_peer_json(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    {
        let mut inner = app.inner.write().await;
        inner.peers.retain(|p| p.name != name);
        for r in &mut inner.rooms {
            r.members.retain(|m| m != &name);
        }
    }
    app.persist().await;
    app.emit_peers("delete", &name);
    action_guard(&name)
}
