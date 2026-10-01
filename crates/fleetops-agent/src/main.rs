use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = fleetops_agent::Config::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&config.log))
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();
    fleetops_agent::run(config).await
}
