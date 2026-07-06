//! OTel SDK setup + metric emitters. Reads OTEL_* env vars supplied by
//! kleos's docker-compose and pushes via OTLP/HTTP to Prometheus.
//! See ../../kleos/docs/observability.md for the contract.

mod breaker;
mod http;
mod llm;
mod metrics;
mod metrics_obs;
mod stt;

pub use breaker::BreakerMetrics;
pub use http::axum_metrics_layer;
#[cfg(test)]
pub(crate) use llm::test_support as llm_test_support;
pub use llm::LlmMetrics;
pub use metrics::{init_metrics, shutdown_metrics, MetricsHandle, MetricsInitError};
pub use metrics_obs::MetricsObs;
pub use stt::SttMetrics;
