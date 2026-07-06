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
    // Git SHA: CI passes `--build-arg AURIS_BUILD_SHA=${{ github.sha }}`
    // which is the long-form commit hash. Local `cargo run` builds have
    // no SHA — render "dev" as the placeholder. Truncate to 7 chars
    // (matching `git rev-parse --short HEAD`) so the field width stays
    // predictable regardless of where the value came from.
    let sha_full = option_env!("AURIS_BUILD_SHA").unwrap_or("dev");
    let sha: String = sha_full.chars().take(7).collect();
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

    // Subcommands handled before clap parses the main arg surface. The
    // runtime image is debian-slim today, but `auris-server healthz`
    // matches the mnemo contract (mnemo@647fb82) — the binary is its
    // own probe so the Dockerfile HEALTHCHECK works regardless of
    // whether the base ever shrinks to distroless. Tracing isn't init'd
    // yet at this point; subcommands write to stderr directly.
    if let Some(sub) = std::env::args().nth(1) {
        if sub == "healthz" {
            healthz_probe().await;
            // healthz_probe always exits; unreachable.
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Exit codes: 2 = auth init, 3 = LLM init, 4 = metrics init (the
    // healthz subcommand uses 0/1). Metrics init only fails when
    // OTEL_EXPORTER_OTLP_ENDPOINT is set-but-broken — i.e. explicit
    // operator intent to have metrics — so running blind is not an
    // acceptable fallback; compose restart makes the failure loud
    // within seconds instead of invisible for weeks.
    let metrics_handle = match auris_server::observability::init_metrics() {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(
                error = %e,
                "metrics init failed; fix OTEL_EXPORTER_OTLP_ENDPOINT or unset it to run without metrics"
            );
            std::process::exit(4);
        }
    };

    let args = Args::parse();
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    let auth_disabled = auris_server::config::flag("AURIS_AUTH_DISABLED");
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
        auris_server::auth::AuthMode::Disabled
    } else {
        let auth0 = match auris_server::auth::AuthValidator::from_env() {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "Auth validator init failed");
                std::process::exit(2);
            }
        };
        // Auris's own HS256 issuer for paired-device tokens. Required
        // in Live mode — without it the /pair/redeem endpoint can't
        // mint anything. Operators set AURIS_JWT_HS256_SECRET to at
        // least 32 random bytes (e.g. `openssl rand -hex 32`).
        let secret = match auris_server::config::var_opt("AURIS_JWT_HS256_SECRET") {
            Some(s) => s,
            None => {
                tracing::error!(
                    "AURIS_JWT_HS256_SECRET is required in Live mode \
                     (generate with: openssl rand -hex 32)"
                );
                std::process::exit(2);
            }
        };
        let auris = match auris_server::auth::pairing::AurisJwtIssuer::new(secret.as_bytes()) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "Auris JWT issuer init failed");
                std::process::exit(2);
            }
        };
        auris_server::auth::AuthMode::Live { auth0, auris }
    };

    // Single shared Arc<BreakerMetrics> for all circuit breakers
    // (LLM pools + mnemo push/recall). Constructed here once and
    // passed into run_server so that run_server_with_listener can
    // reuse the same Arc instead of building a second one.
    let breaker_metrics = Arc::new(auris_server::observability::BreakerMetrics::new());
    breaker_metrics.register("llm.chat");
    breaker_metrics.register("llm.background");
    let chat_breaker = Arc::new(auris_server::util::circuit_breaker::CircuitBreaker::new(
        "llm.chat",
        5,
        std::time::Duration::from_secs(60),
        Some(Box::new(auris_server::observability::MetricsObs(
            breaker_metrics.clone(),
        ))),
    ));
    let background_breaker = Arc::new(auris_server::util::circuit_breaker::CircuitBreaker::new(
        "llm.background",
        5,
        std::time::Duration::from_secs(60),
        Some(Box::new(auris_server::observability::MetricsObs(
            breaker_metrics.clone(),
        ))),
    ));
    let chat_llm = match auris_server::llm::LlmClient::from_env(
        auris_server::llm::LlmPool::Chat,
        Some(chat_breaker),
    )
    .await
    {
        Ok(c) => Arc::new(c),
        Err(e) => {
            tracing::error!(error = %e, pool = "chat", "LLM client init failed");
            std::process::exit(3);
        }
    };
    let background_llm = match auris_server::llm::LlmClient::from_env(
        auris_server::llm::LlmPool::Background,
        Some(background_breaker),
    )
    .await
    {
        Ok(c) => Arc::new(c),
        Err(e) => {
            tracing::error!(error = %e, pool = "background", "LLM client init failed");
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

    let result = auris_server::run_server(
        addr,
        auth,
        chat_llm,
        background_llm,
        breaker_metrics,
        shutdown_rx,
    )
    .await;
    auris_server::observability::shutdown_metrics(metrics_handle);
    result
}

/// Probe the locally-listening `/healthz` and exit 0 on HTTP 200, 1
/// otherwise. Invoked via `auris-server healthz` from the container
/// healthcheck (Dockerfile HEALTHCHECK + kleos compose). Reads
/// `AURIS_PORT` for the target port; falls back to 7331 (the binary's
/// `--port` default, also the EXPOSE/CMD in the Dockerfile). Uses
/// reqwest because it's already a dependency — no new linkage.
async fn healthz_probe() -> ! {
    let port = std::env::var("AURIS_PORT")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "7331".to_string());
    let url = format!("http://127.0.0.1:{port}/healthz");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("healthz: client build failed: {e}");
            std::process::exit(1);
        }
    };
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => std::process::exit(0),
        Ok(resp) => {
            eprintln!("healthz: HTTP {} from {url}", resp.status());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("healthz: GET {url} failed: {e}");
            std::process::exit(1);
        }
    }
}
