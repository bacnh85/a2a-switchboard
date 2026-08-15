use crate::state::{now, AppState, Peer, PeerState, RouteEntry};

// set in main() — controls the non-localhost warning banner
pub static LOCALHOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn is_localhost() -> bool {
    LOCALHOST.load(std::sync::atomic::Ordering::Relaxed)
}
use askama::Template;
use axum::extract::{Path, State};
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
    pub accepted: usize,
    pub pending: usize,
    pub revoked: usize,
    pub healthy: usize,
    pub total_routes: u64,
    pub recent: Vec<RouteEntry>,
}

#[derive(Template)]
#[template(path = "peers.html")]
pub struct PeersTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub pending: Vec<Peer>,
    pub accepted: Vec<Peer>,
    pub revoked: Vec<Peer>,
}

#[derive(Template)]
#[template(path = "logs.html")]
pub struct LogsTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub entries: Vec<RouteEntry>,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub gateway_token: String,
    pub bootstrap_token: String,
}

pub async fn dashboard(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let t = DashboardTmpl {
        title: "Dashboard",
        active_nav: "dashboard",
        localhost: is_localhost(),
        accepted: inner
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Accepted)
            .count(),
        pending: inner
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Pending)
            .count(),
        revoked: inner
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Revoked)
            .count(),
        healthy: inner
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Accepted && p.healthy == Some(true))
            .count(),
        total_routes: app.log_ring.read().await.len() as u64,
        recent: app.recent_log(8).await,
    };
    Html(t.render().unwrap_or_default()).into_response()
}

pub async fn peers_page(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let t = PeersTmpl {
        title: "Peers",
        active_nav: "peers",
        localhost: is_localhost(),
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
        title: "Routing log",
        active_nav: "logs",
        localhost: is_localhost(),
        entries: app.recent_log(200).await,
    };
    Html(t.render().unwrap_or_default()).into_response()
}

#[derive(Template)]
#[template(path = "graph.html")]
pub struct GraphTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub nodes_json: String,
    pub edges_json: String,
}

pub async fn graph_page(State(app): State<AppState>) -> Response {
    // Nodes: gateway center + each accepted/pending peer. Edges: last N routed exchanges.
    let inner = app.inner.read().await;
    let mut nodes = vec![
        serde_json::json!({"id": "gateway", "label": "gateway", "shape": "star", "color": "#7c3aed", "title": "a2a-switchboard"}),
    ];
    for p in &inner.peers {
        if p.state == PeerState::Revoked {
            continue;
        }
        let color = match (p.state, p.healthy) {
            (PeerState::Accepted, Some(true)) => "#22c55e",
            (PeerState::Accepted, _) => "#ef4444",
            _ => "#eab308",
        };
        nodes.push(serde_json::json!({
            "id": p.name, "label": p.name, "color": color,
            "shape": "dot", "size": 14,
            "title": format!("{}\n{}", p.name, p.url),
        }));
    }
    drop(inner);
    let routes = app.recent_log(200).await;
    // Aggregate directed traffic counts src→dst.
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    for r in &routes {
        *counts.entry((r.src.clone(), r.dst.clone())).or_insert(0) += 1;
    }
    let edges: Vec<_> = counts
        .into_iter()
        .map(|((src, dst), n)| {
            serde_json::json!({
                "from": if src == "gateway" { "gateway" } else { &src },
                "to": dst,
                "value": n,
                "arrows": "to",
                "title": format!("{src} → {dst}: {n} calls"),
            })
        })
        .collect();
    let t = GraphTmpl {
        title: "Communication graph",
        active_nav: "graph",
        localhost: is_localhost(),
        nodes_json: serde_json::to_string(&nodes).unwrap(),
        edges_json: serde_json::to_string(&edges).unwrap(),
    };
    Html(t.render().unwrap_or_default()).into_response()
}

pub async fn settings_page(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let t = SettingsTmpl {
        title: "Settings",
        active_nav: "settings",
        localhost: is_localhost(),
        gateway_token: inner.gateway_token.clone(),
        bootstrap_token: inner.bootstrap_token.clone(),
    };
    Html(t.render().unwrap_or_default()).into_response()
}

// ----- actions (htmx form posts → redirect) -----

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

/// SSE stream of new routing entries (event: route) + pings.
pub async fn sse_events(
    State(app): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let rx = app.log_tx.subscribe();
    let stream = async_stream::try_stream! {
        let mut rx = rx;
        loop {
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

/// JSON graph data for the communication graph page (polled by htmx every 5s).
pub async fn graph_data(State(app): State<AppState>) -> Response {
    axum::Json(graph_json(&app).await).into_response()
}

async fn graph_json(app: &AppState) -> serde_json::Value {
    let inner = app.inner.read().await;
    let mut nodes = vec![
        serde_json::json!({"id": "gateway", "label": "gateway", "shape": "star", "color": "#7c3aed"}),
    ];
    for p in &inner.peers {
        if p.state == PeerState::Revoked {
            continue;
        }
        let color = match (p.state, p.healthy) {
            (PeerState::Accepted, Some(true)) => "#22c55e",
            (PeerState::Accepted, _) => "#ef4444",
            _ => "#eab308",
        };
        nodes.push(serde_json::json!({"id": p.name, "label": p.name, "color": color, "shape": "dot", "size": 14}));
    }
    drop(inner);
    let routes = app.recent_log(200).await;
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();
    for r in &routes {
        *counts.entry((r.src.clone(), r.dst.clone())).or_insert(0) += 1;
    }
    let edges: Vec<_> = counts
        .into_iter()
        .map(|((src, dst), n)| serde_json::json!({"from": src, "to": dst, "value": n, "arrows": "to"}))
        .collect();
    serde_json::json!({ "nodes": nodes, "edges": edges })
}
