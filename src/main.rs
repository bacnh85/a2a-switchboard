use a2a_switchboard::{admin, config, health, state, state::App};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,hyper_util=warn".into()),
        )
        .init();

    let cfg = config::Config::load()?;
    let first_run = !cfg.data_dir.join("state.json").exists();

    let app = std::sync::Arc::new(App::load(cfg.data_dir.clone()).await?);
    if first_run {
        let (gw, boot) = {
            let inner = app.inner.read().await;
            (inner.gateway_token.clone(), inner.bootstrap_token.clone())
        };
        tracing::info!("gateway API token : {gw}");
        tracing::info!("bootstrap token   : {boot}");
        app.persist().await;
    }
    // Issue #4: never an unauthenticated admin window. A random password is
    // generated on first run (or upgrade from a passwordless state.json) and
    // logged exactly once; change it in Settings.
    if let Some(pw) = app.ensure_admin_password().await {
        tracing::info!("admin password : {pw}   (change it in Settings)");
    }

    // Runtime knobs from config (issue #5).
    *state::ROUTING_LOG_MAX_BYTES.write().unwrap() = cfg.routing_log_max_mb * 1024 * 1024;
    *state::PREVIEW_ENABLED.write().unwrap() = cfg.audit_previews;

    if cfg.cookie_secure {
        tracing::info!("session cookies marked Secure (TLS front assumed)");
    }
    admin::COOKIE_SECURE.store(cfg.cookie_secure, std::sync::atomic::Ordering::Relaxed);

    if !cfg.binds_localhost() {
        admin::LOCALHOST.store(false, std::sync::atomic::Ordering::Relaxed);
        tracing::warn!(
            "bound to non-localhost interface {}: serving bearer tokens over plaintext HTTP — put a TLS terminator in front (see docs/DEPLOYMENT.md)",
            cfg.bind
        );
    }

    health::spawn(app.clone(), cfg.heartbeat_sec);

    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(
        "a2a-switchboard {} listening on http://{}",
        env!("CARGO_PKG_VERSION"),
        cfg.bind
    );
    axum::serve(
        listener,
        a2a_switchboard::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
