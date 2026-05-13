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

/// Print the Auris boot banner — ear-arcs ASCII mark + build info.
///
/// Goes to stderr so it shares a stream with the tracing logs that
/// follow, but is emitted *before* `tracing_subscriber::init()` so
/// the banner isn't decorated with a timestamp/level prefix.
///
/// Respects `NO_COLOR` (and skips coloring when stderr isn't a TTY)
/// so piped output (`auris-server | tee log`) stays plain.
fn print_banner(addr: &SocketAddr, auth: &str) {
    let coral_on: &str;
    let coral_off: &str;
    let dim_on: &str;
    let dim_off: &str;
    let use_color = std::env::var_os("NO_COLOR").is_none()
        && std::io::IsTerminal::is_terminal(&std::io::stderr());
    if use_color {
        // 24-bit truecolor: Auris coral (#d97757) for the mark/dot,
        // dim grey for the secondary build-info line.
        coral_on = "\x1b[38;2;217;119;87m";
        coral_off = "\x1b[0m";
        dim_on = "\x1b[2m";
        dim_off = "\x1b[0m";
    } else {
        coral_on = "";
        coral_off = "";
        dim_on = "";
        dim_off = "";
    }
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let version = env!("CARGO_PKG_VERSION");
    // Git SHA: CI can pass `AURIS_BUILD_SHA=$(git rev-parse --short HEAD)`
    // at compile time. Local `cargo run` builds will have no SHA — we
    // render a short placeholder instead of dropping the field entirely
    // so the field width stays predictable.
    let sha = option_env!("AURIS_BUILD_SHA").unwrap_or("dev");
    let banner = format!(
        "
{c}   ⌒⌒{r}
{c}  ⌒  •{r}    auris-server  v{version}  ({profile}, {sha})
{c}  ⌒  ⌒{r}    {d}listening on {addr} · auth={auth}{dr}
{c}   ⌒⌒{r}
",
        c = coral_on,
        r = coral_off,
        d = dim_on,
        dr = dim_off,
    );
    eprintln!("{banner}");
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
    let auth_label = if auth_disabled { "disabled" } else { "auth0" };
    print_banner(&addr, auth_label);
    info!(
        ?addr,
        version = env!("CARGO_PKG_VERSION"),
        auth = auth_label,
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
