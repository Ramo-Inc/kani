use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "kani", about = "Cross-platform KVM — share keyboard & mouse")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "kani.toml")]
    config: PathBuf,

    /// Run in dry-run mode (no platform input capture/injection)
    #[arg(long)]
    dry_run: bool,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cli.log_level)),
        )
        .init();

    tracing::info!("Kani starting...");

    let config = kani_proto::config::KaniConfig::load(&cli.config)
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() })?;
    tracing::info!(
        server_id = %config.server.host_id,
        port = config.server.bind_port,
        hosts = config.hosts.len(),
        border_links = config.border_links.len(),
        "Config loaded"
    );

    kani_server::server::run(config, cli.dry_run).await
}
