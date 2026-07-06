//! Per-call LLM metrics. Optional under the kleos contract, but
//! cheap to emit and useful for per-provider dashboards.

use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::KeyValue;

pub struct LlmMetrics {
    duration: Histogram<f64>,
    tokens: Counter<u64>,
}

impl LlmMetrics {
    pub fn new() -> Self {
        Self::with_meter(global::meter("auris/llm"))
    }

    /// Build against an explicit meter. Production uses [`Self::new`]
    /// (global meter, shared instruments across clones); tests pass a
    /// meter backed by an in-memory exporter to assert on emitted
    /// points without touching global state.
    pub fn with_meter(meter: Meter) -> Self {
        Self {
            duration: meter
                .f64_histogram("llm_request_duration_seconds")
                .with_description("Wall-clock time for an LLM call by provider/model/status")
                .with_unit("s")
                .build(),
            tokens: meter
                .u64_counter("llm_tokens_used_total")
                .with_description("LLM tokens consumed by provider/model/direction")
                .build(),
        }
    }

    pub fn record_call(
        &self,
        provider: &str,
        model: &str,
        status: &str,
        latency_seconds: f64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let common = [
            KeyValue::new("provider", provider.to_string()),
            KeyValue::new("model", model.to_string()),
        ];
        let mut with_status: Vec<KeyValue> = common.to_vec();
        with_status.push(KeyValue::new("status", status.to_string()));
        self.duration.record(latency_seconds, &with_status);

        let mut input_attrs: Vec<KeyValue> = common.to_vec();
        input_attrs.push(KeyValue::new("direction", "input"));
        let mut output_attrs: Vec<KeyValue> = common.to_vec();
        output_attrs.push(KeyValue::new("direction", "output"));
        self.tokens.add(input_tokens, &input_attrs);
        self.tokens.add(output_tokens, &output_attrs);
    }
}

impl Default for LlmMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Test-only helpers for asserting on emitted LLM metric points.
/// Shared by the baseline test below and the `timed_recorded` tests
/// in `llm/client.rs` (re-exported from `observability/mod.rs` as
/// `llm_test_support`).
#[cfg(test)]
pub(crate) mod test_support {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::Histogram;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::LlmMetrics;

    /// Build an `LlmMetrics` wired to an in-memory exporter. Keep the
    /// returned `SdkMeterProvider` alive for the duration of the test
    /// and call `.force_flush()` on it before reading the exporter.
    pub(crate) fn in_memory_metrics() -> (LlmMetrics, SdkMeterProvider, InMemoryMetricExporter) {
        let exporter = InMemoryMetricExporter::default();
        let meter_provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let metrics = LlmMetrics::with_meter(meter_provider.meter("auris/llm-test"));
        (metrics, meter_provider, exporter)
    }

    /// All `(status, count)` pairs recorded on the
    /// `llm_request_duration_seconds` histogram, across every flush.
    pub(crate) fn duration_statuses(exporter: &InMemoryMetricExporter) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        for rm in exporter.get_finished_metrics().unwrap() {
            for scope in &rm.scope_metrics {
                for metric in &scope.metrics {
                    if metric.name != "llm_request_duration_seconds" {
                        continue;
                    }
                    let hist = metric
                        .data
                        .as_any()
                        .downcast_ref::<Histogram<f64>>()
                        .expect("llm_request_duration_seconds must be an f64 histogram");
                    for dp in &hist.data_points {
                        let status = dp
                            .attributes
                            .iter()
                            .find(|kv| kv.key.as_str() == "status")
                            .map(|kv| kv.value.to_string())
                            .unwrap_or_default();
                        out.push((status, dp.count));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{duration_statuses, in_memory_metrics};

    /// Baseline: `record_call` emits exactly one duration point carrying
    /// the status label it was given. Tasks 3-5 build on this plumbing
    /// to assert failure-side emission.
    #[test]
    fn record_call_emits_one_duration_point_with_status_label() {
        let (metrics, meter_provider, exporter) = in_memory_metrics();
        metrics.record_call("bedrock", "test-model", "ok", 1.25, 100, 50);
        meter_provider.force_flush().unwrap();
        assert_eq!(duration_statuses(&exporter), vec![("ok".to_string(), 1)]);
    }
}
