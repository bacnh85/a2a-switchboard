use crate::state::{now, AppState, PeerKind, PeerState};

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
                // Humans have no upstream endpoint to probe.
                .filter(|p| p.state == PeerState::Accepted && p.kind != PeerKind::Human)
                .map(|p| (p.name.clone(), p.url.clone()))
                .collect();
            for (name, base) in urls {
                // Channel peers may be unreachable by URL by construction —
                // a live channel IS the health signal.
                if app.channels.has(&name) {
                    apply_health(&app, &name, true, None).await;
                    continue;
                }
                let target = format!("{}/.well-known/agent-card.json", base.trim_end_matches('/'));
                match app
                    .http
                    .get(&target)
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => {
                        apply_health(&app, &name, true, None).await;
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

/// Set health + last_seen/last_error for one peer; emits a `peers` event only
/// when `healthy` actually flips (probe writes run every tick — per-tick
/// events would spam the SSE stream). pub for integration tests.
pub async fn apply_health(app: &AppState, name: &str, ok: bool, err: Option<String>) {
    // Probe writes run every heartbeat tick: emit a peers event only when the
    // healthy value actually FLIPS, or a stable fleet spams the SSE stream.
    let flipped = {
        let mut inner = app.inner.write().await;
        if let Some(p) = inner.peers.iter_mut().find(|p| p.name == name) {
            let was = p.healthy;
            p.healthy = Some(ok);
            p.last_error = err;
            p.last_probe_ts = Some(now());
            if ok {
                p.last_ok_ts = Some(now());
            }
            if ok {
                p.last_seen = Some(now());
            }
            was != Some(ok)
        } else {
            false
        }
    };
    if flipped {
        app.emit_peers("health", name);
    }
}

async fn set_unhealthy(app: &AppState, name: &str, msg: &str) {
    apply_health(app, name, false, Some(msg.to_string())).await;
}

fn pe_msg_trunc(s: &str) -> String {
    s.chars().take(200).collect()
}
