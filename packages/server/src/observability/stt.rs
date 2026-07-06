//! Soniox STT reconnect metric — already tracked locally in
//! stt/soniox.rs as `consecutive_failures`; we just expose it.

use opentelemetry::global;
use opentelemetry::metrics::Gauge;

pub struct SttMetrics {
    reconnect_failures: Gauge<u64>,
}

impl SttMetrics {
    pub fn new() -> Self {
        let meter = global::meter("auris/stt");
        Self {
            reconnect_failures: meter
                .u64_gauge("stt_soniox_consecutive_reconnect_failures")
                .with_description(
                    "Soniox STT WS consecutive failed reconnect attempts since last success",
                )
                .build(),
        }
    }

    pub fn set_reconnect_failures(&self, n: u64) {
        self.reconnect_failures.record(n, &[]);
    }
}

impl Default for SttMetrics {
    fn default() -> Self {
        Self::new()
    }
}
