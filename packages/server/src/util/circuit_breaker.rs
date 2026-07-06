//! Generic circuit breaker — Rust port of mnemo's
//! `internal/llm/breaker.go`. Tracks consecutive failures and
//! trips open at a threshold. After cooldown, admits one probe.
//! All public methods are safe for concurrent use.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Receives state-transition notifications. Implementations must
/// be safe for concurrent use; the breaker calls `transition`
/// while holding its own lock. Pass `None` to opt out.
pub trait BreakerObserver: Send + Sync {
    fn transition(&self, name: &str, to_state: &str);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Closed,
    Open,
    HalfOpen,
}

fn label(s: State) -> &'static str {
    match s {
        State::Closed => "closed",
        State::Open => "open",
        State::HalfOpen => "half_open",
    }
}

/// Floor for the half-open probe deadline. 180s = 2 × the longest
/// in-op timeout anywhere in the codebase (`PDF_TIMEOUT` = 90s in
/// `llm/client.rs`), so a probe that is genuinely still running is
/// never declared stale: anything older has either completed
/// (recorded an outcome) or was dropped without recording one.
const PROBE_DEADLINE_FLOOR: Duration = Duration::from_secs(180);

pub struct CircuitBreaker {
    name: String,
    threshold: u32,
    cooldown: Duration,
    /// HalfOpen self-heal: if a probe has been in flight longer than
    /// this without recording an outcome, `allow()` declares it stale
    /// and admits a replacement probe. Defaults to
    /// `max(cooldown, PROBE_DEADLINE_FLOOR)`; override with
    /// [`with_probe_deadline`](Self::with_probe_deadline).
    probe_deadline: Duration,
    observer: Option<Box<dyn BreakerObserver>>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("name", &self.name)
            .field("threshold", &self.threshold)
            .field("cooldown", &self.cooldown)
            .finish_non_exhaustive()
    }
}

struct Inner {
    state: State,
    failures: u32,
    opened_at: Option<Instant>,
    probe_in_flight: bool,
    /// When the in-flight HalfOpen probe was admitted. `None` whenever
    /// `probe_in_flight` is false. Drives the stale-probe self-heal.
    probe_started_at: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new(
        name: impl Into<String>,
        threshold: u32,
        cooldown: Duration,
        observer: Option<Box<dyn BreakerObserver>>,
    ) -> Self {
        Self {
            name: name.into(),
            threshold,
            cooldown,
            probe_deadline: cooldown.max(PROBE_DEADLINE_FLOOR),
            observer,
            inner: Mutex::new(Inner {
                state: State::Closed,
                failures: 0,
                opened_at: None,
                probe_in_flight: false,
                probe_started_at: None,
            }),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Override the half-open probe deadline (defaults to
    /// `max(cooldown, 180s)`). Builder-style; production construction
    /// sites keep the default — this exists so tests can use
    /// millisecond deadlines.
    pub fn with_probe_deadline(mut self, deadline: Duration) -> Self {
        self.probe_deadline = deadline;
        self
    }

    pub fn allow(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.state {
            State::Closed => true,
            State::Open => {
                let elapsed = inner
                    .opened_at
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::ZERO);
                if elapsed >= self.cooldown {
                    self.transition_locked(&mut inner, State::HalfOpen);
                    inner.probe_in_flight = true;
                    inner.probe_started_at = Some(Instant::now());
                    true
                } else {
                    false
                }
            }
            State::HalfOpen => {
                if inner.probe_in_flight {
                    // Defense-in-depth (improvement #16): a probe whose
                    // outcome was never recorded — e.g. a raw-`allow()`
                    // caller whose future was dropped mid-await — would
                    // otherwise wedge the breaker in HalfOpen forever.
                    // Past the deadline, declare it stale and admit a
                    // replacement probe. If the stale probe later
                    // completes anyway, its late outcome recording is
                    // benign: last write wins, worst case one spurious
                    // transition.
                    let stale = inner
                        .probe_started_at
                        .map(|t| t.elapsed() > self.probe_deadline)
                        .unwrap_or(true);
                    if stale {
                        tracing::warn!(
                            breaker = %self.name,
                            deadline_secs = self.probe_deadline.as_secs(),
                            "half-open probe exceeded deadline without recording an outcome; admitting replacement probe"
                        );
                        inner.probe_started_at = Some(Instant::now());
                        true
                    } else {
                        false
                    }
                } else {
                    inner.probe_in_flight = true;
                    inner.probe_started_at = Some(Instant::now());
                    true
                }
            }
        }
    }

    pub fn success(&self) {
        let mut inner = self.inner.lock().unwrap();
        self.transition_locked(&mut inner, State::Closed);
        inner.failures = 0;
        inner.probe_in_flight = false;
        inner.probe_started_at = None;
    }

    pub fn failure(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.probe_in_flight = false;
        inner.probe_started_at = None;
        if inner.state == State::HalfOpen {
            self.transition_locked(&mut inner, State::Open);
            inner.opened_at = Some(Instant::now());
            return;
        }
        inner.failures += 1;
        if inner.failures >= self.threshold {
            self.transition_locked(&mut inner, State::Open);
            inner.opened_at = Some(Instant::now());
        }
    }

    /// RAII variant of [`allow`](Self::allow): returns a guard whose
    /// `Drop` records [`failure`](Self::failure) unless
    /// [`ProbeGuard::succeed`] was called first, in which case it
    /// records [`success`](Self::success). Returns `None` when the
    /// breaker denies the call (Open pre-cooldown, or a fresh HalfOpen
    /// probe already in flight).
    ///
    /// Prefer this over the raw `allow()` / `success()` / `failure()`
    /// triplet for ANY call site: if the future holding the guard is
    /// dropped mid-await (`tokio::select!`, `tokio::time::timeout`,
    /// `?` early-return, panic), the outcome is still recorded —
    /// pessimistically as a failure — instead of leaking
    /// `probe_in_flight` and wedging the breaker (improvement #16).
    pub fn try_acquire(&self) -> Option<ProbeGuard<'_>> {
        if self.allow() {
            Some(ProbeGuard {
                breaker: self,
                succeeded: false,
            })
        } else {
            None
        }
    }

    fn transition_locked(&self, inner: &mut Inner, to: State) {
        if inner.state == to {
            return;
        }
        inner.state = to;
        if let Some(obs) = &self.observer {
            obs.transition(&self.name, label(to));
        }
    }
}

/// RAII outcome recorder handed out by [`CircuitBreaker::try_acquire`].
///
/// Default-on-drop is `failure()` ("assume the worst") — the safe
/// default for the HalfOpen state, where a leaked probe permanently
/// blocks further probes. A dropped-but-healthy probe merely re-opens
/// the breaker for one extra cooldown: noisy but self-healing. For the
/// Closed state a spurious drop costs one failure count — tolerable.
pub struct ProbeGuard<'a> {
    breaker: &'a CircuitBreaker,
    succeeded: bool,
}

impl ProbeGuard<'_> {
    /// Mark this call as successful. Must be called before the guard
    /// leaves scope on the success path; `Drop` then records
    /// `success()` instead of `failure()`. Calling it more than once
    /// is a no-op.
    pub fn succeed(&mut self) {
        self.succeeded = true;
    }
}

impl Drop for ProbeGuard<'_> {
    // Keep Drop minimal: success()/failure() only take the breaker
    // mutex and never run user code under it, so this cannot panic or
    // poison anything even when invoked during unwinding.
    fn drop(&mut self) {
        if self.succeeded {
            self.breaker.success();
        } else {
            self.breaker.failure();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingObserver {
        transitions: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl BreakerObserver for CountingObserver {
        fn transition(&self, name: &str, to_state: &str) {
            self.transitions
                .lock()
                .unwrap()
                .push((name.to_string(), to_state.to_string()));
        }
    }

    #[allow(clippy::type_complexity)]
    fn make() -> (CircuitBreaker, Arc<Mutex<Vec<(String, String)>>>) {
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let obs = CountingObserver {
            transitions: transitions.clone(),
        };
        let cb = CircuitBreaker::new("test", 3, Duration::from_millis(50), Some(Box::new(obs)));
        (cb, transitions)
    }

    #[test]
    fn allows_when_closed() {
        let (cb, _) = make();
        assert!(cb.allow());
    }

    #[test]
    fn trips_after_threshold_consecutive_failures() {
        let (cb, t) = make();
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        // Next allow should be denied (open).
        assert!(!cb.allow());
        assert_eq!(
            t.lock().unwrap().as_slice(),
            &[("test".into(), "open".into())]
        );
    }

    #[test]
    fn success_resets_failure_count() {
        let (cb, _) = make();
        for _ in 0..2 {
            assert!(cb.allow());
            cb.failure();
        }
        assert!(cb.allow());
        cb.success();
        // After success, can fail 3 more times before tripping.
        for _ in 0..2 {
            assert!(cb.allow());
            cb.failure();
        }
        assert!(cb.allow()); // still closed
    }

    #[test]
    fn cooldown_admits_one_probe() {
        let (cb, t) = make();
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        assert!(!cb.allow());
        std::thread::sleep(Duration::from_millis(60));
        // Cooldown elapsed: first allow is the probe (half-open).
        assert!(cb.allow());
        // While probe is in flight, no other calls are admitted.
        assert!(!cb.allow());
        // Successful probe closes the breaker.
        cb.success();
        assert!(cb.allow());
        let kinds: Vec<String> = t.lock().unwrap().iter().map(|(_, k)| k.clone()).collect();
        assert_eq!(kinds, vec!["open", "half_open", "closed"]);
    }

    #[test]
    fn failed_probe_reopens_with_fresh_cooldown() {
        let (cb, t) = make();
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        std::thread::sleep(Duration::from_millis(60));
        assert!(cb.allow()); // probe
        cb.failure();
        assert!(!cb.allow()); // back to open immediately
        let kinds: Vec<String> = t.lock().unwrap().iter().map(|(_, k)| k.clone()).collect();
        assert_eq!(kinds, vec!["open", "half_open", "open"]);
    }

    #[test]
    fn observer_not_called_for_no_op_transitions() {
        let (cb, t) = make();
        cb.success(); // already closed; no transition
        cb.success();
        assert!(t.lock().unwrap().is_empty());
    }

    #[test]
    fn concurrent_failures_serialize_correctly() {
        let (cb, _) = make();
        let cb = Arc::new(cb);
        let mut handles = Vec::new();
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..20 {
            let cb = cb.clone();
            let counter = counter.clone();
            handles.push(std::thread::spawn(move || {
                if cb.allow() {
                    counter.fetch_add(1, Ordering::SeqCst);
                    cb.failure();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // After threshold (3) failures the breaker opens — only those
        // 3 should have made it through. The other 17 see Allow=false.
        // Concurrency may let a few extra through if their Allow
        // observed Closed before the third Failure latched Open; the
        // contract is "trips at threshold," not "trips exactly at
        // threshold," so we tolerate a few but cap at 6 to catch real
        // breakage.
        let admitted = counter.load(Ordering::SeqCst);
        assert!(
            (3..=6).contains(&admitted),
            "expected 3..=6 admitted under concurrency, got {admitted}",
        );
    }

    // ── improvement #16: cancellation-safe gating ─────────────────────

    /// `try_acquire` returns a guard in the Closed state.
    #[test]
    fn try_acquire_returns_guard_when_closed() {
        let (cb, _) = make();
        let mut guard = cb.try_acquire().expect("closed breaker admits");
        guard.succeed();
    }

    /// `try_acquire` returns `None` while the breaker is Open
    /// (pre-cooldown) — equivalent to `allow() == false`.
    #[test]
    fn try_acquire_denied_while_open() {
        let (cb, _) = make();
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        assert!(
            cb.try_acquire().is_none(),
            "open breaker must deny try_acquire"
        );
    }

    /// THE core wedge regression: a HalfOpen probe guard dropped
    /// WITHOUT `succeed()` (models a future cancelled while parked at
    /// an `.await`) must record a failure — re-opening the breaker
    /// with a fresh cooldown — instead of leaking `probe_in_flight`
    /// and denying every subsequent call until process restart.
    #[test]
    fn probe_guard_drop_without_succeed_reopens_breaker() {
        let (cb, t) = make();
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        std::thread::sleep(Duration::from_millis(60));
        {
            let guard = cb.try_acquire();
            assert!(guard.is_some(), "post-cooldown probe must be admitted");
            // Guard dropped here without succeed().
        }
        // Drop recorded a failure: breaker is Open again.
        assert!(
            !cb.allow(),
            "breaker must be open right after the dropped probe"
        );
        let kinds: Vec<String> = t.lock().unwrap().iter().map(|(_, k)| k.clone()).collect();
        assert_eq!(kinds, vec!["open", "half_open", "open"]);
        // Self-heal: the next cooldown admits a fresh probe. Pre-fix,
        // probe_in_flight stayed true forever and this returned None.
        std::thread::sleep(Duration::from_millis(60));
        assert!(
            cb.try_acquire().is_some(),
            "fresh probe must be admitted after cooldown"
        );
    }

    /// `succeed()` before the guard drops closes the breaker, exactly
    /// like the old `success()` call did.
    #[test]
    fn probe_guard_succeed_closes_breaker() {
        let (cb, _) = make();
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        std::thread::sleep(Duration::from_millis(60));
        {
            let mut guard = cb.try_acquire().expect("probe admitted after cooldown");
            guard.succeed();
        }
        assert!(cb.allow(), "breaker must be closed after successful probe");
    }

    /// In the Closed state, a dropped guard counts exactly one failure
    /// (pessimistic default — same semantics as llm/client.rs's
    /// BreakerGuard).
    #[test]
    fn closed_state_guard_drop_counts_one_failure() {
        let (cb, _) = make(); // threshold 3
        drop(cb.try_acquire().expect("closed: admitted"));
        drop(cb.try_acquire().expect("still closed: admitted"));
        // Two recorded failures < threshold 3 — still closed.
        assert!(
            cb.allow(),
            "two dropped guards must not trip a threshold-3 breaker"
        );
        cb.failure(); // third failure
        assert!(!cb.allow(), "third recorded failure trips the breaker");
    }

    /// Defense-in-depth: a HalfOpen probe whose outcome is never
    /// recorded (a legacy raw `allow()` caller whose future was
    /// dropped) must not wedge the breaker — past the probe deadline,
    /// a replacement probe is admitted.
    #[test]
    fn stale_halfopen_probe_past_deadline_admits_new_probe() {
        let cb = CircuitBreaker::new("test", 3, Duration::from_millis(50), None)
            .with_probe_deadline(Duration::from_millis(30));
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        std::thread::sleep(Duration::from_millis(60));
        // Raw allow() — simulates a caller that never records an outcome.
        assert!(cb.allow(), "post-cooldown probe admitted");
        // Probe still fresh: others denied.
        assert!(!cb.allow(), "second caller denied while probe is fresh");
        // Past the deadline: the stale probe is replaced.
        std::thread::sleep(Duration::from_millis(40));
        assert!(
            cb.allow(),
            "stale probe must be replaced — new probe admitted"
        );
    }

    /// A stale-but-alive probe that completes after its replacement
    /// was admitted records a benign late outcome (last write wins;
    /// no panic, no stuck state).
    #[test]
    fn stale_probe_late_success_is_benign() {
        let cb = CircuitBreaker::new("test", 3, Duration::from_millis(50), None)
            .with_probe_deadline(Duration::from_millis(30));
        for _ in 0..3 {
            assert!(cb.allow());
            cb.failure();
        }
        std::thread::sleep(Duration::from_millis(60));
        assert!(cb.allow(), "probe A admitted"); // never records
        std::thread::sleep(Duration::from_millis(40));
        assert!(cb.allow(), "probe B replaces stale A");
        // A completes late and records success: breaker closes.
        cb.success();
        assert!(cb.allow(), "breaker closed after A's late success");
        // B completes: no-op transition, still closed, no panic.
        cb.success();
        assert!(cb.allow());
    }
}
