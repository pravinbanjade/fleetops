pub mod config;
pub mod maintenance;
pub mod static_files;
pub mod tls;

use axum::{Extension, Router};
use config::Config;
use fleetops_api::{envs::bootstrap_local, ApiConfig, AppState, IsTls};
use fleetops_docker::DockerEnvFactory;
use fleetops_store::Store;
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub async fn run(config: Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data)?;
    let store = Store::open(&config.data.join("fleetops.db")).await?;
    let state = build_state(
        store.clone(),
        ApiConfig {
            hsts: config.hsts,
            ..ApiConfig::default()
        },
    );
    state.envs.load(&store).await?;
    if let Some(env) = bootstrap_local(&store, &state.envs, &config.docker_host).await? {
        tracing::info!(url = %env.url, "registered local Docker environment");
    }
    let app = fleetops_api::app(state, static_files::router());
    tokio::spawn(maintenance::run(store));
    serve(&config, app).await
}

/// The API state with the env factory wired in: one agent registry, shared
/// by the factory (agent environments read sessions from it) and the
/// connect endpoint (which registers them).
pub fn build_state(store: Store, config: ApiConfig) -> AppState {
    let agents = Arc::new(fleetops_tunnel::registry::Registry::default());
    AppState::new(
        store,
        config,
        Arc::new(DockerEnvFactory::new(agents.clone())),
    )
    .with_agents(agents)
}

async fn serve(config: &Config, app: Router) -> anyhow::Result<()> {
    let handle = axum_server::Handle::new();
    tokio::spawn(shutdown_on_signal(handle.clone()));

    let http = axum_server::bind(config.bind).handle(handle.clone()).serve(
        app.clone()
            .layer(Extension(IsTls(false)))
            .into_make_service_with_connect_info::<SocketAddr>(),
    );
    tracing::info!(addr = %config.bind, "fleetops listening (http)");

    match config.tls_bind {
        None => http.await?,
        Some(addr) => {
            tls::install_crypto_provider();
            let rustls = tls::load_or_generate(config).await?;
            let https = axum_server::bind_rustls(addr, rustls).handle(handle).serve(
                app.layer(Extension(IsTls(true)))
                    .into_make_service_with_connect_info::<SocketAddr>(),
            );
            tracing::info!(%addr, "fleetops listening (https)");
            tokio::try_join!(http, https)?;
        }
    }
    Ok(())
}

async fn shutdown_on_signal(handle: axum_server::Handle) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("shutting down");
    handle.graceful_shutdown(Some(Duration::from_secs(10)));
}
