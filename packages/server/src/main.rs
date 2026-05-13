use anyhow::Result;
use clap::Parser;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "auris-server")]
struct Args {
    /// Single port serving both WebSocket (control + /audio) and
    /// REST (/meetings…) over axum.
    #[arg(long, default_value_t = 7331)]
    port: u16,
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
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    let auth_disabled = auris_server::env::flag("AURIS_AUTH_DISABLED");
    info!(
        ?addr,
        version = env!("CARGO_PKG_VERSION"),
        auth = if auth_disabled { "disabled" } else { "auth0" },
        "boot"
    );

    let auth = if auth_disabled {
        tracing::warn!(
            "AURIS_AUTH_DISABLED=1: bypass mode, every request maps to a synthetic dev user"
        );
        auris_server::ws::AuthMode::Disabled
    } else {
        match auris_server::auth::AuthValidator::from_env() {
            Ok(v) => auris_server::ws::AuthMode::Live(v),
            Err(e) => {
                tracing::error!(error = %e, "Auth validator init failed");
                std::process::exit(2);
            }
        }
    };

    let llm = match auris_server::llm::LlmClient::from_env().await {
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

    auris_server::run_server(addr, auth, llm, shutdown_rx).await
}
