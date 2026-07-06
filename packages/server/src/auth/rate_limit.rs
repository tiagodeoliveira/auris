//! Per-IP + global rate limiting for the public pair endpoints.
//!
//! `POST /pair/redeem` and `POST /pair/refresh` are intentionally
//! unauthenticated (the unpaired PWA must reach them), which makes
//! them the only place an anonymous caller can spend our CPU:
//! a garbage `/pair/refresh` token triggers an argon2 verify against
//! EVERY active device hash (~50 ms each, memory-hard) before the
//! handler can 401. This module bounds that amplification.
//!
//! Design mirrors the house style of `pairing.rs`:
//!   - a pure, clock-injected decision function (`check`) so window
//!     boundaries are unit-testable without sleeping (cf.
//!     `match_refresh_slot`),
//!   - process-global `LazyLock<Mutex<...>>` state with lazy
//!     sweep-on-access, no background task (cf. `REDEEM_CACHE`).
//!
//! Two layers, both fixed 60 s windows:
//!   - per-IP budget (`PER_IP_MAX`) — the normal limiter,
//!   - global ceiling (`GLOBAL_MAX`) — the backstop for the
//!     behind-proxy deployment where every client shares the proxy's
//!     peer address (see `client_ip` / `AURIS_TRUST_PROXY`). Even if
//!     IP attribution is wrong, total argon2 work stays bounded.
//!
//! State is process-local and lost on restart — same property as
//! `REDEEM_CACHE`; an attacker gains one fresh window per restart,
//! which is negligible.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

/// Fixed window length for both the per-IP and global budgets.
pub(crate) const WINDOW: Duration = Duration::from_secs(60);

/// Requests allowed per IP per window. Honest use is ~1 refresh/hour
/// per device and one-shot redeems; 10/min leaves huge headroom while
/// capping a single source's argon2 fan-out.
pub(crate) const PER_IP_MAX: u32 = 10;

/// Requests allowed across ALL IPs per window. This is the backstop
/// that bounds total argon2 work when the kleos reverse proxy
/// collapses every client into one peer address.
pub(crate) const GLOBAL_MAX: u32 = 120;

/// Idle eviction horizon: per-IP entries whose window started longer
/// ago than this are swept on the next `check`, so the map can't
/// grow unbounded under address-spraying.
pub(crate) const IDLE_EVICT: Duration = Duration::from_secs(600);

/// One fixed-window counter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bucket {
    window_start: Instant,
    count: u32,
}

impl Bucket {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            count: 0,
        }
    }
}

/// Pure decision core: mutate the per-IP map and the global bucket
/// for a request from `ip` at `now`; return `true` iff the request
/// is allowed. Sweeps idle per-IP entries on every call. DB-free and
/// clock-injected so the window boundary is unit-testable.
///
/// Note: the global counter is consumed even when the per-IP check
/// rejects (and vice versa). That slightly over-counts rejected
/// requests against the other budget — acceptable, and it errs on
/// the side of rejecting floods.
pub(crate) fn check(
    map: &mut HashMap<IpAddr, Bucket>,
    global: &mut Bucket,
    ip: IpAddr,
    now: Instant,
) -> bool {
    // Lazy sweep — same no-background-task pattern as REDEEM_CACHE.
    // (`Instant::duration_since` saturates to zero for a future
    // `window_start`, so a fresh entry is never evicted.)
    map.retain(|_, b| now.duration_since(b.window_start) < IDLE_EVICT);

    fn roll(b: &mut Bucket, max: u32, now: Instant) -> bool {
        if now.duration_since(b.window_start) >= WINDOW {
            *b = Bucket::new(now);
        }
        if b.count >= max {
            false
        } else {
            b.count += 1;
            true
        }
    }

    let global_ok = roll(global, GLOBAL_MAX, now);
    let per_ip_ok = roll(
        map.entry(ip).or_insert_with(|| Bucket::new(now)),
        PER_IP_MAX,
        now,
    );
    global_ok && per_ip_ok
}

static IP_BUCKETS: LazyLock<Mutex<HashMap<IpAddr, Bucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static GLOBAL_BUCKET: LazyLock<Mutex<Bucket>> =
    LazyLock::new(|| Mutex::new(Bucket::new(Instant::now())));

/// Thin wrapper over `check` against the process-global buckets.
/// Handlers call this first; `false` → respond 429.
/// `.expect("poisoned")` matches the `REDEEM_CACHE` convention — a
/// poisoned mutex here means a panic mid-increment, which is
/// unrecoverable state anyway.
pub(crate) fn pair_endpoint_allows(ip: IpAddr) -> bool {
    let mut map = IP_BUCKETS.lock().expect("rate-limit ip mutex poisoned");
    let mut global = GLOBAL_BUCKET
        .lock()
        .expect("rate-limit global mutex poisoned");
    check(&mut map, &mut global, ip, Instant::now())
}

/// Pure core of client-IP attribution: pick the left-most
/// `X-Forwarded-For` entry when (and only when) the deployment has
/// declared the proxy trusted; otherwise — or when the header is
/// missing/malformed — fall back to the socket peer.
pub(crate) fn client_ip_from(
    forwarded_for: Option<&str>,
    peer: IpAddr,
    trust_proxy: bool,
) -> IpAddr {
    if !trust_proxy {
        return peer;
    }
    forwarded_for
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .and_then(|s| s.parse::<IpAddr>().ok())
        .unwrap_or(peer)
}

/// Resolve the client IP for rate-limit bucketing. Reads the
/// `AURIS_TRUST_PROXY` boot flag (same `config::flag` semantics as
/// `AURIS_AUTH_DISABLED`: set + non-empty = on). auris runs behind
/// the kleos reverse proxy in production, where the socket peer is
/// the proxy itself; with the flag set we trust its left-most
/// `X-Forwarded-For`. Misconfiguration fails safe — everyone shares
/// the per-IP bucket but `GLOBAL_MAX` still bounds total work.
pub(crate) fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    let trust_proxy = crate::config::flag("AURIS_TRUST_PROXY");
    let xff = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    client_ip_from(xff, peer.ip(), trust_proxy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    fn fresh() -> (HashMap<IpAddr, Bucket>, Bucket, Instant) {
        let now = Instant::now();
        (HashMap::new(), Bucket::new(now), now)
    }

    #[test]
    fn allows_up_to_limit_then_blocks() {
        let (mut map, mut global, now) = fresh();
        for i in 0..PER_IP_MAX {
            assert!(
                check(&mut map, &mut global, ip(1), now),
                "request {i} is within the per-IP budget and must be allowed"
            );
        }
        assert!(
            !check(&mut map, &mut global, ip(1), now),
            "request past PER_IP_MAX must be blocked"
        );
    }

    #[test]
    fn refills_after_window() {
        let (mut map, mut global, now) = fresh();
        for _ in 0..PER_IP_MAX {
            assert!(check(&mut map, &mut global, ip(1), now));
        }
        assert!(!check(&mut map, &mut global, ip(1), now));
        let later = now + WINDOW + Duration::from_secs(1);
        assert!(
            check(&mut map, &mut global, ip(1), later),
            "budget must refill once the window has rolled"
        );
    }

    #[test]
    fn independent_ips_have_independent_buckets() {
        let (mut map, mut global, now) = fresh();
        for _ in 0..PER_IP_MAX {
            assert!(check(&mut map, &mut global, ip(1), now));
        }
        assert!(!check(&mut map, &mut global, ip(1), now));
        assert!(
            check(&mut map, &mut global, ip(2), now),
            "a different IP must have its own budget"
        );
    }

    #[test]
    fn global_ceiling_trips_across_ips() {
        let (mut map, mut global, now) = fresh();
        for i in 0..GLOBAL_MAX {
            let addr = IpAddr::V4(Ipv4Addr::new(10, 1, (i / 256) as u8, (i % 256) as u8));
            assert!(
                check(&mut map, &mut global, addr, now),
                "request {i} is within the global ceiling"
            );
        }
        assert!(
            !check(&mut map, &mut global, ip(99), now),
            "global ceiling must trip across distinct IPs"
        );
    }

    #[test]
    fn evicts_idle_ip_entries() {
        let (mut map, mut global, now) = fresh();
        assert!(check(&mut map, &mut global, ip(1), now));
        assert!(map.contains_key(&ip(1)), "entry created on first check");
        let later = now + IDLE_EVICT + Duration::from_secs(1);
        assert!(check(&mut map, &mut global, ip(2), later));
        assert!(
            !map.contains_key(&ip(1)),
            "an entry idle past IDLE_EVICT must be swept on the next check"
        );
        assert_eq!(map.len(), 1, "only the fresh entry remains");
    }

    // ── client-IP resolution ─────────────────────────────────────────

    #[test]
    fn client_ip_ignores_xff_without_trust_proxy() {
        let peer = ip(7);
        assert_eq!(
            client_ip_from(Some("203.0.113.50"), peer, false),
            peer,
            "X-Forwarded-For is attacker-controlled unless a trusted proxy sets it"
        );
    }

    #[test]
    fn client_ip_uses_leftmost_xff_when_trusted() {
        let peer = ip(7);
        assert_eq!(
            client_ip_from(Some("203.0.113.50, 10.0.0.7"), peer, true),
            "203.0.113.50".parse::<IpAddr>().unwrap(),
            "left-most XFF entry is the original client"
        );
    }

    #[test]
    fn client_ip_falls_back_to_peer_on_malformed_xff() {
        let peer = ip(7);
        assert_eq!(
            client_ip_from(Some("not-an-ip"), peer, true),
            peer,
            "garbage XFF must not bypass attribution — fall back to peer"
        );
    }

    #[test]
    fn client_ip_falls_back_to_peer_when_header_missing() {
        let peer = ip(7);
        assert_eq!(client_ip_from(None, peer, true), peer);
    }

    #[test]
    fn client_ip_wrapper_respects_env_flag() {
        let peer = std::net::SocketAddr::from(([192, 0, 2, 1], 443));
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.50".parse().unwrap());

        std::env::remove_var("AURIS_TRUST_PROXY");
        assert_eq!(
            client_ip(&headers, peer),
            peer.ip(),
            "flag unset: peer wins"
        );

        std::env::set_var("AURIS_TRUST_PROXY", "1");
        assert_eq!(
            client_ip(&headers, peer),
            "203.0.113.50".parse::<IpAddr>().unwrap(),
            "flag set: trusted XFF wins"
        );
        std::env::remove_var("AURIS_TRUST_PROXY");
    }
}
