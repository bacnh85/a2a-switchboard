use agent_gateway::{admin, config, health, state::App};

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
        tracing::info!("first run: generated tokens (also shown in admin UI → Settings)");
        tracing::info!("gateway API token : {gw}");
        tracing::info!("bootstrap token   : {boot}");
        app.persist().await;
    }
    if !cfg.binds_localhost() {
        admin::LOCALHOST.store(false, std::sync::atomic::Ordering::Relaxed);
        tracing::warn!("bound to non-localhost interface {}: admin UI is UNAUTHENTICATED", cfg.bind);
    }

    health::spawn(app.clone(), cfg.heartbeat_sec);

    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!("agent-gateway {} listening on http://{}", env!("CARGO_PKG_VERSION"), cfg.bind);
    axum::serve(
        listener,
        agent_gateway::router(app)
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
