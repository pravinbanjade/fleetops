use clap::Parser;
use fleetops_server::config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Helm 3's dry-run post-renderer: swallow the manifest, print nothing.
    if std::env::args().nth(1).as_deref() == Some(fleetops_kube::helm::render::DISCARD_ARG) {
        std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink())?;
        return Ok(());
    }
    let config = Config::parse();
    if config.openapi {
        println!("{}", fleetops_server::openapi_json());
        return Ok(());
    }
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    fleetops_server::run(config).await
}
