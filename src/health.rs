use crate::state::{now, AppState, PeerState};

/// Periodically probe each accepted peer's agent card; feed health + last_seen.
pub fn spawn(app: AppState, interval_sec: u64) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval_sec.max(1)));
        loop {
            tick.tick().await;
            let urls: Vec<(String, String)> = app
                .inner
                .read()
                .await
                .peers
                .iter()
                .filter(|p| p.state == PeerState::Accepted)
                .map(|p| (p.name.clone(), p.url.clone()))
                .collect();
            for (name, base) in urls {
                // Channel peers may be unreachable by URL by construction —
                // a live channel IS the health signal.
                if app.channels.has(&name) {
                    let mut inner = app.inner.write().await;
                    if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
                        p.healthy = Some(true);
                        p.last_seen = Some(now());
                        p.last_error = None;
                    }
                    drop(inner);
                    continue;
                }
                let target = format!("{}/.well-known/agent-card.json", base.trim_end_matches('/'));
                match app.http.get(&target).timeout(std::time::Duration::from_secs(5)).send().await {
                    Ok(r) if r.status().is_success() => {
                        let mut inner = app.inner.write().await;
                        if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
                            p.healthy = Some(true);
                            p.last_seen = Some(now());
                            p.last_error = None;
                        }
                    }
                    Ok(r) => {
                        let msg = format!("probe: HTTP {}", r.status());
                        set_unhealthy(&app, &name, &msg).await;
                    }
                    Err(e) => {
                        let msg = format!("probe: {e}");
                        set_unhealthy(&app, &name, &pe_msg_trunc(&msg)).await;
                    }
                }
            }
        }
    });
}

async fn set_unhealthy(app: &AppState, name: &str, msg: &str) {
    let mut inner = app.inner.write().await;
    if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
        p.healthy = Some(false);
        p.last_error = Some(msg.to_string());
    }
}

fn pe_msg_trunc(s: &str) -> String {
    s.chars().take(200).collect()
}
