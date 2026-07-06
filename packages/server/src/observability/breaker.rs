//! BreakerMetrics — adapts the util::circuit_breaker::BreakerObserver
//! trait to OTel via the global meter. One instance is safe to share
//! across many breakers; the `name` parameter on each Transition
//! identifies the source.
//!
//! Emits the two metrics required by kleos/docs/observability.md:
//!   - circuit_breaker_open                (Gauge<u64>, labels: name)
//!   - circuit_breaker_transitions_total   (Counter<u64>, labels: name, to_state)

use opentelemetry::global;
use opentelemetry::metrics::{Counter, Gauge};
use opentelemetry::KeyValue;

use crate::util::circuit_breaker::BreakerObserver;

pub struct BreakerMetrics {
    open_gauge: Gauge<u64>,
    transitions: Counter<u64>,
}

impl BreakerMetrics {
    pub fn new() -> Self {
        let meter = global::meter("auris/circuit_breaker");
        Self {
            open_gauge: meter
                .u64_gauge("circuit_breaker_open")
                .with_description("1 when the named breaker is open, 0 otherwise")
                .build(),
            transitions: meter
                .u64_counter("circuit_breaker_transitions_total")
                .with_description("Count of circuit-breaker state transitions")
                .build(),
        }
    }

    /// Seeds `circuit_breaker_open{name}=0` so the gauge appears in
    /// Prometheus from the first export interval even though the
    /// breaker hasn't actually transitioned. Without this, brand-new
    /// services don't show their breakers on dashboards until the
    /// first real failure or recovery. Does NOT bump the transitions
    /// counter — boot is not a transition.
    pub fn register(&self, name: &str) {
        self.open_gauge
            .record(0, &[KeyValue::new("name", name.to_string())]);
    }
}

impl Default for BreakerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl BreakerObserver for BreakerMetrics {
    fn transition(&self, name: &str, to_state: &str) {
        let name_kv = KeyValue::new("name", name.to_string());
        let open = u64::from(to_state == "open");
        self.open_gauge.record(open, std::slice::from_ref(&name_kv));
        self.transitions.add(
            1,
            &[name_kv, KeyValue::new("to_state", to_state.to_string())],
        );
    }
}
