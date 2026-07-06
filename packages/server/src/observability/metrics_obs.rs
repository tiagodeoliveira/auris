//! Adapter that wraps an Arc<BreakerMetrics> so it can be plugged into
//! CircuitBreaker's Option<Box<dyn BreakerObserver>> slot. Each breaker
//! gets its own Box; they share the underlying Arc<BreakerMetrics> so
//! all transitions hit the same OTel instruments.

use std::sync::Arc;

use crate::util::circuit_breaker::BreakerObserver;

use super::BreakerMetrics;

pub struct MetricsObs(pub Arc<BreakerMetrics>);

impl BreakerObserver for MetricsObs {
    fn transition(&self, name: &str, to_state: &str) {
        self.0.transition(name, to_state);
    }
}
