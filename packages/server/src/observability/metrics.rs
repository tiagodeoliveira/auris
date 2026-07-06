//! Builds the OTLP/HTTP MeterProvider and registers it as the global.
//!
//! Reads three env vars (all set by kleos's docker-compose):
//!   - OTEL_SERVICE_NAME           (`auris`)
//!   - OTEL_EXPORTER_OTLP_PROTOCOL (`http/protobuf`)
//!   - OTEL_EXPORTER_OTLP_ENDPOINT (`http://prometheus:9090/api/v1/otlp`)
//!
//! Contract (improvement #8 — "the monitoring of the monitoring"):
//!   - Endpoint unset or empty  → soft disable: local `cargo run`
//!     without prometheus keeps working, no metrics. (Empty counts as
//!     unset because docker-compose `${VAR:-}` passthrough produces
//!     `KEY=""`; see config.rs.)
//!   - Endpoint set but invalid → Err(MetricsInitError); main exits
//!     with code 4. opentelemetry-otlp 0.28 swallows endpoint parse
//!     errors inside build() and silently falls back to
//!     http://localhost:4318, so without our own validation a typo'd
//!     compose edit means the server runs healthy-looking while every
//!     metric is a no-op.
//!   - Endpoint set and valid   → exporter installed AND the
//!     `auris_build_info{version,sha}=1` heartbeat gauge registered.
//!     Kleos alerts on absence, which also catches the well-formed-but-
//!     unreachable endpoint that boot validation cannot.

use std::time::Duration;

use opentelemetry::global;
use opentelemetry::metrics::{Meter, MeterProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{MetricExporter, WithExportConfig};
use opentelemetry_sdk::metrics::periodic_reader_with_async_runtime::PeriodicReader;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::runtime;
use opentelemetry_sdk::Resource;
use tracing::{info, warn};

/// Matches Prometheus's `evaluation_interval` (15s). Tightening
/// wastes bandwidth without helping alert latency; loosening delays
/// detection.
const EXPORT_INTERVAL: Duration = Duration::from_secs(15);

/// Handle held by main; calling `shutdown_metrics()` consumes it
/// and flushes the final batch.
#[derive(Debug)]
pub struct MetricsHandle(Option<SdkMeterProvider>);

impl MetricsHandle {
    pub fn disabled() -> Self {
        Self(None)
    }

    /// True when a real MeterProvider was installed (endpoint set and
    /// valid); false on the soft-disable path. Exists so callers and
    /// tests can distinguish the two Ok states of `init_metrics`.
    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }
}

/// Why metrics init can fail. Endpoint-unset/empty is NOT an error
/// (soft disable for local dev); these variants are reserved for
/// "the operator clearly wanted metrics and they cannot work" —
/// main treats them as fatal (exit 4).
#[derive(Debug, thiserror::Error)]
pub enum MetricsInitError {
    #[error("OTEL_EXPORTER_OTLP_ENDPOINT is set but not a valid http(s) URI: {0:?}")]
    BadEndpoint(String),
    #[error("OTLP metric exporter build failed: {0}")]
    Exporter(#[from] opentelemetry_sdk::metrics::MetricError),
}

/// Registers the `auris_build_info{version,sha} = 1` heartbeat gauge.
///
/// An ObservableGauge callback runs on EVERY collection (the 15s
/// EXPORT_INTERVAL), producing a fresh data point per export by
/// construction — unlike a regular Gauge seeded once at boot, which
/// leans on the SDK's last-value retention. Kleos alerts on
/// `absent(auris_build_info)`: that fires for process-down,
/// exporter-down, AND a well-formed-but-unreachable endpoint, the
/// case boot-time validation cannot catch.
///
/// The returned ObservableGauge handle is deliberately dropped: the
/// callback is retained by the provider pipeline, not the handle
/// (proven by the in-memory-exporter test below).
fn register_build_info(meter: &Meter, version: String, sha: String) {
    let attrs = [KeyValue::new("version", version), KeyValue::new("sha", sha)];
    meter
        .u64_observable_gauge("auris_build_info")
        .with_description(
            "Constant 1; absence in Prometheus means the auris metrics pipeline is down",
        )
        .with_callback(move |obs| obs.observe(1, &attrs))
        .build();
}

/// Builds + installs the global MeterProvider. On success returns a
/// handle callers must pass to `shutdown_metrics()` on graceful
/// shutdown, or drop on crash (which still flushes via the periodic
/// reader best-effort). Returns Ok(disabled) when the endpoint is
/// unset/empty, Err when it is set but unusable — main treats Err as
/// fatal (exit 4).
pub fn init_metrics() -> Result<MetricsHandle, MetricsInitError> {
    // var_opt: empty counts as unset (docker-compose `${VAR:-}`
    // passthrough lands as KEY="" and means "operator chose no
    // metrics", not "operator typo'd the endpoint").
    let endpoint = match crate::config::var_opt("OTEL_EXPORTER_OTLP_ENDPOINT") {
        None => {
            info!("OTEL_EXPORTER_OTLP_ENDPOINT unset — metrics disabled");
            return Ok(MetricsHandle::disabled());
        }
        Some(e) => e,
    };

    // Validate the endpoint ourselves: opentelemetry-otlp 0.28's
    // resolve_http_endpoint() does env::var(..).ok().and_then(|s|
    // build_endpoint_uri(..).ok()) — a malformed endpoint silently
    // falls back to http://localhost:4318, so build() "succeeds" and
    // every export fails forever against a port nothing listens on.
    // http::Uri is the same type the crate parses with (semantics
    // aligned); the scheme allowlist additionally rejects
    // "prometheus:9090/..." (missing http://), which Uri happily
    // parses as scheme="prometheus".
    let uri: http::Uri = endpoint
        .parse()
        .map_err(|_| MetricsInitError::BadEndpoint(endpoint.clone()))?;
    if !matches!(uri.scheme_str(), Some("http") | Some("https")) {
        return Err(MetricsInitError::BadEndpoint(endpoint));
    }

    let exporter = MetricExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .build()?;

    // Use the Tokio-runtime variant of PeriodicReader because the
    // OTLP exporter uses async reqwest, which needs a Tokio reactor
    // in context. The plain (std::thread-backed) PeriodicReader
    // panics with "no reactor running" when the export thread tries
    // to drive the reqwest future. The `rt-tokio` feature on
    // opentelemetry_sdk gates this type.
    let reader = PeriodicReader::builder(exporter, runtime::Tokio)
        .with_interval(EXPORT_INTERVAL)
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(
            Resource::builder()
                .with_service_name(
                    std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "auris".to_string()),
                )
                .build(),
        )
        .build();

    global::set_meter_provider(provider.clone());

    // Same compile-time sources as the boot banner in main.rs:
    // CARGO_PKG_VERSION for the version, AURIS_BUILD_SHA (CI
    // --build-arg, long-form github.sha) truncated to 7 chars for the
    // sha, "dev" for local builds.
    let sha: String = option_env!("AURIS_BUILD_SHA")
        .unwrap_or("dev")
        .chars()
        .take(7)
        .collect();
    register_build_info(
        &provider.meter("auris/build"),
        env!("CARGO_PKG_VERSION").to_string(),
        sha,
    );

    info!(
        interval_secs = EXPORT_INTERVAL.as_secs(),
        "metrics SDK initialised"
    );
    Ok(MetricsHandle(Some(provider)))
}

/// Flushes the final batch and shuts the provider down. Called from
/// the signal handler in main. Safe to call on a disabled handle
/// (no-op).
pub fn shutdown_metrics(handle: MetricsHandle) {
    let Some(provider) = handle.0 else {
        return;
    };
    if let Err(err) = provider.shutdown() {
        warn!(error = %err, "metrics shutdown returned error");
    } else {
        info!("metrics flushed and shut down");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::metrics::{data, InMemoryMetricExporter, PeriodicReader};

    /// The heartbeat must export a fresh `auris_build_info` gauge data
    /// point with value 1 and version/sha attributes on every flush —
    /// this is the series kleos alerts on with absent(auris_build_info).
    #[test]
    fn build_info_gauge_exports_constant_one_with_version_and_sha() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();

        register_build_info(
            &provider.meter("auris/build"),
            "1.2.3".to_string(),
            "abc1234".to_string(),
        );
        provider.force_flush().expect("force_flush");

        let finished = exporter.get_finished_metrics().expect("finished metrics");
        let metric = finished
            .iter()
            .flat_map(|rm| rm.scope_metrics.iter())
            .flat_map(|sm| sm.metrics.iter())
            .find(|m| m.name == "auris_build_info")
            .expect("auris_build_info metric was exported");
        let gauge = metric
            .data
            .as_any()
            .downcast_ref::<data::Gauge<u64>>()
            .expect("auris_build_info is a u64 gauge");
        assert_eq!(gauge.data_points.len(), 1, "exactly one heartbeat series");
        let dp = &gauge.data_points[0];
        assert_eq!(dp.value, 1, "heartbeat value is the constant 1");

        let attr = |key: &str| {
            dp.attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.to_string())
        };
        assert_eq!(attr("version").as_deref(), Some("1.2.3"));
        assert_eq!(attr("sha").as_deref(), Some("abc1234"));

        provider.shutdown().expect("provider shutdown");
    }

    /// All three env states in sequence (parallel-safe: no other test
    /// touches OTEL_EXPORTER_OTLP_ENDPOINT; saved/restored for the
    /// developer's shell). The happy path is NOT asserted here — it
    /// installs a real global meter provider + Tokio PeriodicReader
    /// outside a reactor.
    #[test]
    fn init_metrics_endpoint_handling() {
        const KEY: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
        let saved = std::env::var(KEY).ok();

        std::env::remove_var(KEY);
        let handle = init_metrics().expect("unset endpoint must soft-disable, not error");
        assert!(!handle.is_enabled());

        std::env::set_var(KEY, "");
        let handle = init_metrics().expect("empty endpoint must soft-disable, not error");
        assert!(!handle.is_enabled());

        std::env::set_var(KEY, "http://prom host/api/v1/otlp");
        let err = init_metrics().expect_err("malformed endpoint must fail init");
        assert!(matches!(err, MetricsInitError::BadEndpoint(_)));

        std::env::set_var(KEY, "prometheus:9090/api/v1/otlp");
        let err = init_metrics().expect_err("scheme-less endpoint must fail init");
        assert!(matches!(err, MetricsInitError::BadEndpoint(_)));

        match saved {
            Some(v) => std::env::set_var(KEY, v),
            None => std::env::remove_var(KEY),
        }
    }
}
