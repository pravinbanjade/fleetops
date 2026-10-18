pub mod config;
pub mod factory;
pub mod maintenance;
pub mod static_files;
pub mod tls;

use axum::{Extension, Router};
use config::Config;
use fleetops_api::{envs::bootstrap_local, ApiConfig, AppState, IsTls};
use fleetops_store::Store;
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub async fn run(config: Config) -> anyhow::Result<()> {
    let store = open_store(&config).await?;
    let kube_exec = fleetops_kube::exec_auth::ExecPolicy::parse(
        config.kube_exec_plugins.as_deref().unwrap_or(""),
    )
    .map_err(|e| anyhow::anyhow!("--kube-exec-plugins: {e}"))?;
    let state = build_state(
        store.clone(),
        ApiConfig {
            hsts: config.hsts,
            public_url: config
                .public_url
                .as_deref()
                .map(|u| u.trim_end_matches('/').to_string()),
            kube_exec,
            helm: config.helm_binary.clone().map(|binary| {
                Arc::new(fleetops_kube::helm::render::HelmCli {
                    binary,
                    insecure_repos: config.helm_insecure_repos,
                    timeout: fleetops_kube::helm::render::RENDER_TIMEOUT,
                    // This binary, run with DISCARD_ARG (see main.rs).
                    discard: std::env::current_exe().ok().map(|exe| {
                        fleetops_kube::helm::render::Discard {
                            program: exe.display().to_string(),
                            args: vec![fleetops_kube::helm::render::DISCARD_ARG.into()],
                        }
                    }),
                }) as Arc<dyn fleetops_kube::helm::render::ChartRenderer>
            }),
            data_dir: Some(config.data.clone()),
            ..ApiConfig::default()
        },
    )
    .with_restart(Arc::new(ReExec));
    state.envs.load(&store).await?;
    if let Some(env) = bootstrap_local(&store, &state.envs, &config.docker_host).await? {
        tracing::info!(url = %env.url, "registered local Docker environment");
    }
    #[cfg(feature = "enterprise")]
    let app = {
        let ee = fleetops_ee::init(
            &store,
            fleetops_ee::EeConfig {
                license_file: config.license_file.clone(),
                sso_endpoints: Default::default(),
            },
        )
        .await?;
        tokio::spawn(ee.background);
        fleetops_api::app_with(
            state
                .with_entitlements(ee.entitlements)
                .with_extra_grants(ee.grants)
                .with_custom_roles(ee.roles)
                .with_external_auth(ee.external_auth)
                .with_backup_hooks(ee.backup_hooks),
            static_files::router(),
            ee.routes,
        )
    };
    #[cfg(not(feature = "enterprise"))]
    let app = fleetops_api::app(state, static_files::router());
    tokio::spawn(maintenance::run(store));
    serve(&config, app).await
}

/// The OpenAPI document this binary serves (`fleetops --openapi`).
pub fn openapi_json() -> String {
    #[cfg(feature = "enterprise")]
    return fleetops_api::openapi::json_with(fleetops_ee::api_routes());
    #[cfg(not(feature = "enterprise"))]
    fleetops_api::openapi::json().to_string()
}

/// The API state with the env factory wired in: one agent registry, shared
/// by the factory (agent environments read sessions from it) and the
/// connect endpoint (which registers them).
pub fn build_state(store: Store, config: ApiConfig) -> AppState {
    let agents = Arc::new(fleetops_tunnel::registry::Registry::default());
    let exec = config.kube_exec.clone();
    AppState::new(
        store,
        config,
        Arc::new(factory::Factory::new(agents.clone(), exec)),
    )
    .with_agents(agents)
}

/// Opens the database with the secret key from FLEETOPS_SECRET_KEY, or
/// from <data>/secret.key (created on first start).
pub async fn open_store(config: &Config) -> anyhow::Result<Store> {
    std::fs::create_dir_all(&config.data)?;
    match fleetops_store::backup::apply_pending(&config.data).await? {
        fleetops_store::backup::Pending::None => {}
        fleetops_store::backup::Pending::Applied(dir) => {
            tracing::info!(previous = %dir.display(), "restored from backup; previous data kept")
        }
        fleetops_store::backup::Pending::Rejected(dir) => tracing::error!(
            moved_to = %dir.display(),
            "the pending restore was damaged; kept the current data"
        ),
    }
    let key = match &config.secret_key {
        Some(text) => fleetops_store::secrets::SecretKey::from_base64(text)?,
        None => {
            let path = config.data.join("secret.key");
            let (key, created) = fleetops_store::secrets::SecretKey::load_or_create(&path)?;
            if created {
                tracing::info!(path = %path.display(), "created the secret key; back it up from Admin → Backup");
            }
            key
        }
    };
    Ok(Store::open(&config.data.join("fleetops.db"))
        .await?
        .with_secret_key(&key))
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

/// Restarts by replacing this process with a fresh copy of itself, same
/// arguments and environment, so a pending restore is swapped in.
pub struct ReExec;

impl fleetops_api::backup::Restart for ReExec {
    fn restart(&self) {
        tracing::info!("restarting to finish the restore");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            if let Ok(exe) = std::env::current_exe() {
                let err = std::process::Command::new(exe)
                    .args(std::env::args_os().skip(1))
                    .exec();
                tracing::error!(error = %err, "restart failed; exiting so a supervisor restarts FleetOps");
            }
        }
        std::process::exit(0);
    }
}
