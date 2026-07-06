//! Builds the axum middleware layer that emits the OTel HTTP server
//! histogram (`http.server.request.duration`). Prometheus's OTLP
//! receiver translates the dotted OTel name to
//! `http_server_request_duration_seconds_{count,bucket,sum}` — the
//! metric names HighErrorRate + HighP99Latency query in
//! `kleos/prometheus/alert-rules.yml`.

use axum_otel_metrics::{HttpMetricsLayer, HttpMetricsLayerBuilder};

pub fn axum_metrics_layer() -> HttpMetricsLayer {
    HttpMetricsLayerBuilder::new().build()
}
