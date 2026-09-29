pub mod config;
pub mod maintenance;
pub mod static_files;

use axum::{Extension, Router};
use config::Config;
use dockerer_api::{ApiConfig, AppState, IsTls};
use dockerer_store::Store;
use std::{net::SocketAddr, time::Duration};

pub async fn run(config: Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data)?;
    let store = Store::open(&config.data.join("dockerer.db")).await?;
    let app = dockerer_api::app(
        AppState::new(store.clone(), ApiConfig::default()),
        static_files::router(),
    );
    tokio::spawn(maintenance::run(store));
    serve(&config, app).await
}

async fn serve(config: &Config, app: Router) -> anyhow::Result<()> {
    let handle = axum_server::Handle::new();
    tokio::spawn(shutdown_on_signal(handle.clone()));
    tracing::info!(addr = %config.bind, "dockerer listening (http)");
    axum_server::bind(config.bind)
        .handle(handle)
        .serve(
            app.layer(Extension(IsTls(false)))
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
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
