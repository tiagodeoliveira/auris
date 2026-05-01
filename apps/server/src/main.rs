use anyhow::{Context, Result};
use clap::Parser;
use std::net::SocketAddr;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "meeting-companion-server")]
#[command(about = "Meeting Companion stub WebSocket server")]
struct Args {
    /// TCP port to bind
    #[arg(long, default_value_t = 7331)]
    port: u16,

    /// Bind address
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let token = std::env::var("MEETING_COMPANION_TOKEN")
        .context("MEETING_COMPANION_TOKEN env var must be set and non-empty")?;
    if token.is_empty() {
        anyhow::bail!("MEETING_COMPANION_TOKEN must be non-empty");
    }

    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    info!(?addr, version = env!("CARGO_PKG_VERSION"), "boot");

    info!("server scaffold complete; WS handling lands in Task 10");
    Ok(())
}
