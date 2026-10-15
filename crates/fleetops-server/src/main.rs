use clap::Parser;
use fleetops_server::config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    if config.openapi {
        println!("{}", fleetops_server::openapi_json());
        return Ok(());
    }
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    fleetops_server::run(config).await
}
