use anyhow::{Context, Result};
use clap::Parser;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "meeting-companion-server")]
struct Args {
    #[arg(long, default_value_t = 7331)]
    port: u16,
    /// Port for the REST API server. Defaults to `port + 1`.
    /// Two separate listeners today; consolidating onto a single
    /// port would require migrating the WS layer to axum.
    #[arg(long)]
    api_port: Option<u16>,
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let token = std::env::var("MEETING_COMPANION_TOKEN")
        .context("MEETING_COMPANION_TOKEN env var must be set")?;
    if token.is_empty() {
        anyhow::bail!("MEETING_COMPANION_TOKEN must be non-empty");
    }
    let ws_addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    let api_port = args.api_port.unwrap_or(args.port + 1);
    let api_addr: SocketAddr = format!("{}:{}", args.bind, api_port).parse()?;
    info!(
        ?ws_addr,
        ?api_addr,
        version = env!("CARGO_PKG_VERSION"),
        "boot"
    );

    let llm = match meeting_companion_server::llm::LlmClient::from_env().await {
        Ok(c) => Arc::new(c),
        Err(e) => {
            tracing::error!(error = %e, "LLM client init failed");
            std::process::exit(3);
        }
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("sigterm");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
        let _ = shutdown_tx.send(());
    });

    meeting_companion_server::run_server(ws_addr, api_addr, token, llm, shutdown_rx).await
}
