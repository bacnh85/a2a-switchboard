use crate::state::{now, AppState, Peer, PeerState, RouteEntry};

// set in main() — controls the non-localhost warning banner
pub static LOCALHOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn is_localhost() -> bool {
    LOCALHOST.load(std::sync::atomic::Ordering::Relaxed)
}
use askama::Template;
use axum::extract::{Form, Path, State};
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
    pub accepted: usize,
    pub pending: usize,
    pub revoked: usize,
    pub healthy: usize,
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
#[template(path = "logs.html")]
pub struct LogsTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
    pub entries: Vec<RouteEntry>,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTmpl {
    pub title: &'static str,
    pub active_nav: &'static str,
    pub localhost: bool,
    pub authed: bool,
    pub password_set: bool,
    pub pw: String,
    pub gateway_token: String,
    pub bootstrap_token: String,
}

pub async fn dashboard(State(app): State<AppState>) -> Response {
    let inner = app.inner.read().await;
    let accepted = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Accepted)
        .count();
    let healthy = inner
        .peers
        .iter()
        .filter(|p| p.state == PeerState::Accepted && p.healthy == Some(true))
        .count();
    let peers_json = topology_peers(&app, &inner.peers).to_string();
    let t = DashboardTmpl {
        title: "Dashboard",
        active_nav: "dashboard",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        accepted,
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
        healthy,
        total_routes: app.log_ring.read().await.len() as u64,
        recent: app.recent_log(8).await,
        peers_json,
    };
    drop(inner);
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
        title: "Routing log",
        active_nav: "logs",
        localhost: is_localhost(),
        authed: app.admin_set().await,
        entries: app.recent_log(200).await,
    };
    Html(t.render().unwrap_or_default()).into_response()
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
        password_set: inner.admin.is_some(),
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

/// Is the source IP a local/private address? Gates the first-time admin
/// password set. Behind podman/docker port publishing the socket source IP is
/// the container-bridge gateway (e.g. 10.88.0.35), never 127.0.0.1 — so treat
/// loopback plus the RFC1918 private ranges (10/8, 172.16/12, 192.168/16,
/// which cover podman's 10.88/16 and docker's 172.17/16 bridges) as local.
fn is_local_source(ip: &str) -> bool {
    let Ok(ip) = ip.parse::<std::net::IpAddr>() else {
        return false;
    };
    let v4 = match ip {
        std::net::IpAddr::V4(v4) => Some(v4),
        // Unmap IPv4-mapped IPv6 (::ffff:a.b.c.d) — podman may present the
        // peer as a mapped address even for IPv4 bridges.
        std::net::IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .or_else(|| v6.is_loopback().then_some(std::net::Ipv4Addr::LOCALHOST)),
    };
    v4.map(|v4| v4.is_loopback() || v4.is_private())
        .unwrap_or(false)
}

/// Set (first time, local/private source only) or change the admin password.
pub async fn set_password(
    State(app): State<AppState>,
    crate::auth::ClientIp(ip): crate::auth::ClientIp,
    Form(f): Form<PasswordForm>,
) -> Response {
    let localhost = is_local_source(&ip);
    let is_change = app.admin_set().await;
    if !is_change && !localhost {
        return Redirect::to("/settings").into_response();
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

#[cfg(test)]
mod tests {
    use super::is_local_source;

    #[test]
    fn local_source_detection() {
        // loopback v4/v6 + IPv4-mapped loopback
        assert!(is_local_source("127.0.0.1"));
        assert!(is_local_source("127.8.9.10"));
        assert!(is_local_source("::1"));
        assert!(is_local_source("::ffff:127.0.0.1"));
        // podman/docker bridge sources
        assert!(is_local_source("10.88.0.35"));
        assert!(is_local_source("10.0.0.1"));
        assert!(is_local_source("172.17.0.1"));
        assert!(is_local_source("172.31.255.254"));
        assert!(is_local_source("192.168.1.42"));
        assert!(is_local_source("::ffff:10.88.0.35"));
        // public / non-local sources must NOT pass the gate
        assert!(!is_local_source("8.8.8.8"));
        assert!(!is_local_source("172.32.0.1"));
        assert!(!is_local_source("192.169.0.1"));
        assert!(!is_local_source("2001:db8::1"));
        assert!(!is_local_source("not-an-ip"));
    }
}
