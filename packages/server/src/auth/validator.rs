//! JWT validation against Auth0.
//!
//! Stage-1 scaffolding: the validator is constructed from
//! `AUTH0_DOMAIN` + `AUTH0_API_AUDIENCE` env vars at boot, fetches the
//! tenant's JWKS lazily on first verify, caches keys by `kid`, and
//! refetches once on cache miss (handles Auth0 key rotation).
//!
//! Not yet wired into request paths — that's stage 2. This module is
//! standalone, fully unit-testable on its own.
//!
//! Bypass: when `AURIS_AUTH_DISABLED=1`, callers should
//! short-circuit to a synthetic dev user. The bypass is the caller's
//! responsibility (axum middleware in stage 2); this module always
//! validates a real JWT.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

/// How long to wait between JWKS refetches when a `kid` cache miss
/// happens. Without rate-limiting, an attacker could forge tokens
/// with random `kid`s and force us to hammer Auth0.
const JWKS_REFETCH_COOLDOWN: Duration = Duration::from_secs(60);

/// HTTP timeout for JWKS fetches. Auth0 is fast (~100ms p99); a
/// generous cap keeps us responsive without thrashing on a bad
/// network.
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Subset of standard JWT claims we care about. Auth0 may add custom
/// claims (e.g., `https://meeting.example/tenant`) — we ignore those
/// for now and revisit when we need them.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    /// The Auth0 user id. Stable across logins; the foreign key into
    /// our `users` table (`users.auth0_sub`).
    pub sub: String,
    /// User's email if the `email` scope was requested at login.
    /// Best-effort; some social connections don't return it.
    #[serde(default)]
    pub email: Option<String>,
    /// User's display name if `profile` scope was requested. Same
    /// caveat as `email`.
    #[serde(default)]
    pub name: Option<String>,
    /// Token expiry (seconds since unix epoch). `jsonwebtoken`
    /// validates this for us — kept here so callers can log it.
    pub exp: u64,
}

/// JWKS response shape. Auth0 returns `{"keys": [...]}` with one
/// entry per active signing key. We only consume RSA keys signed
/// with RS256 (Auth0's default).
#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<JwkEntry>,
}

#[derive(Debug, Deserialize)]
struct JwkEntry {
    kid: String,
    /// Key type — "RSA" for the keys we accept.
    #[allow(dead_code)]
    kty: String,
    /// RSA modulus, base64url.
    n: String,
    /// RSA exponent, base64url.
    e: String,
}

/// Cached decoding key + when we last refreshed the cache. The
/// `Instant` is used to gate refetches — see `JWKS_REFETCH_COOLDOWN`.
struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    last_fetched_at: Option<Instant>,
}

#[derive(Clone)]
pub struct AuthValidator {
    inner: Arc<AuthValidatorInner>,
}

struct AuthValidatorInner {
    /// `https://<tenant>.<region>.auth0.com/` — note the trailing
    /// slash. Used as the JWT issuer (`iss`) and as the prefix for
    /// JWKS lookup.
    issuer: String,
    /// JWKS endpoint URL. Derived from `issuer` once at construction.
    jwks_url: String,
    /// Audience the token must be addressed to (our API identifier).
    audience: String,
    /// HTTP client used for JWKS fetches. Reused across calls so TLS
    /// + connection pooling kicks in.
    http: reqwest::Client,
    /// Decoded keys keyed by `kid`.
    cache: RwLock<KeyCache>,
}

impl AuthValidator {
    /// Build a validator from `AUTH0_DOMAIN` + `AUTH0_API_AUDIENCE`.
    /// `AUTH0_DOMAIN` must be the bare domain (no scheme, no path) —
    /// e.g. `dev-jrva0wzk3qkdxcar.us.auth0.com`. We add `https://`
    /// and the trailing slash to match the JWT issuer claim Auth0
    /// emits.
    pub fn from_env() -> Result<Self> {
        let domain = std::env::var("AUTH0_DOMAIN")
            .context("AUTH0_DOMAIN env var is required for JWT validation")?;
        let audience = std::env::var("AUTH0_API_AUDIENCE")
            .context("AUTH0_API_AUDIENCE env var is required for JWT validation")?;
        Self::new(&domain, &audience)
    }

    /// Test/explicit constructor.
    pub fn new(domain: &str, audience: &str) -> Result<Self> {
        Self::build(domain, audience, None)
    }

    /// Test-only constructor that overrides where the JWKS document is
    /// fetched from (a local stub server), while `issuer`/`audience`
    /// validation still uses `https://<domain>/`. Production always
    /// derives the JWKS URL from the domain via `new`.
    #[cfg(test)]
    pub(crate) fn new_with_jwks_url(
        domain: &str,
        audience: &str,
        jwks_url: String,
    ) -> Result<Self> {
        Self::build(domain, audience, Some(jwks_url))
    }

    fn build(domain: &str, audience: &str, jwks_url_override: Option<String>) -> Result<Self> {
        let domain = domain.trim().trim_end_matches('/');
        if domain.is_empty() {
            return Err(anyhow!("AUTH0_DOMAIN is empty"));
        }
        let issuer = format!("https://{domain}/");
        let jwks_url =
            jwks_url_override.unwrap_or_else(|| format!("https://{domain}/.well-known/jwks.json"));
        let http = reqwest::Client::builder()
            .timeout(JWKS_FETCH_TIMEOUT)
            .build()
            .context("build reqwest client for JWKS")?;
        Ok(Self {
            inner: Arc::new(AuthValidatorInner {
                issuer,
                jwks_url,
                audience: audience.to_string(),
                http,
                cache: RwLock::new(KeyCache {
                    keys: HashMap::new(),
                    last_fetched_at: None,
                }),
            }),
        })
    }

    /// Validate a JWT and return its claims.
    ///
    /// Verifies:
    ///  * RS256 signature against Auth0's published key matching
    ///    the token's `kid` header.
    ///  * `iss` matches `https://<domain>/`.
    ///  * `aud` matches the configured audience.
    ///  * `exp` hasn't passed (with the default 60s clock skew
    ///    `jsonwebtoken` allows).
    pub async fn validate(&self, token: &str) -> Result<Claims> {
        let header = decode_header(token).context("decode JWT header")?;
        let kid = header
            .kid
            .ok_or_else(|| anyhow!("JWT header missing `kid`"))?;
        if header.alg != Algorithm::RS256 {
            return Err(anyhow!(
                "unexpected JWT alg {:?}; expected RS256",
                header.alg
            ));
        }

        // First try the cache. On miss, refetch (rate-limited) and
        // try again. A second miss is fatal — token's `kid` is bogus
        // or we're out of sync with Auth0 in some unrecoverable way.
        let key = match self.cached_key(&kid).await {
            Some(k) => k,
            None => {
                self.refetch_jwks_if_due().await?;
                self.cached_key(&kid)
                    .await
                    .ok_or_else(|| anyhow!("JWT signed with unknown kid `{kid}`"))?
            }
        };

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.inner.issuer]);
        validation.set_audience(&[&self.inner.audience]);
        // `exp` validation is on by default; kept explicit for clarity.
        validation.validate_exp = true;

        let data = decode::<Claims>(token, &key, &validation).context("verify JWT signature")?;
        Ok(data.claims)
    }

    async fn cached_key(&self, kid: &str) -> Option<DecodingKey> {
        let cache = self.inner.cache.read().await;
        cache.keys.get(kid).cloned()
    }

    /// Fetch JWKS and update the cache. Rate-limited via
    /// `JWKS_REFETCH_COOLDOWN` so a flood of forged-`kid` tokens
    /// can't pin us to outbound HTTP.
    async fn refetch_jwks_if_due(&self) -> Result<()> {
        // Quick check: someone else recently refreshed? Skip.
        {
            let cache = self.inner.cache.read().await;
            if let Some(t) = cache.last_fetched_at {
                if t.elapsed() < JWKS_REFETCH_COOLDOWN {
                    return Ok(());
                }
            }
        }
        let jwks: Jwks = self
            .inner
            .http
            .get(&self.inner.jwks_url)
            .send()
            .await
            .with_context(|| format!("GET {}", self.inner.jwks_url))?
            .error_for_status()
            .context("JWKS endpoint returned non-2xx")?
            .json()
            .await
            .context("parse JWKS response")?;

        let mut new_keys = HashMap::with_capacity(jwks.keys.len());
        for entry in jwks.keys {
            match DecodingKey::from_rsa_components(&entry.n, &entry.e) {
                Ok(k) => {
                    new_keys.insert(entry.kid, k);
                }
                Err(e) => {
                    tracing::warn!(error = %e, kid = %entry.kid, "JWKS entry rejected");
                }
            }
        }
        if new_keys.is_empty() {
            return Err(anyhow!("JWKS contained no usable RSA keys"));
        }

        let mut cache = self.inner.cache.write().await;
        cache.keys = new_keys;
        cache.last_fetched_at = Some(Instant::now());
        tracing::info!(count = cache.keys.len(), "JWKS refreshed");
        Ok(())
    }
}

/// Resolve a request's bearer token to a local `users.id`. Three paths:
///
/// - `AuthMode::Disabled`: bypass — every caller maps to a fixed dev
///   user upserted on first call.
/// - `AuthMode::Live` with an Auth0-issued token (`iss = https://<tenant>/`):
///   validate against JWKS, then upsert the user from the claims.
/// - `AuthMode::Live` with an Auris-issued token (`iss = "auris-server"`):
///   verify HS256 signature, confirm the referenced device hasn't been
///   revoked, then look up the user. This is the path the PWA takes
///   after the device-pairing flow.
///
/// Dispatch is by `iss` peek (unverified header read). The peek is
/// safe — we still verify the signature in the chosen validator
/// before trusting any claim.
///
/// Returns the local UUID (`users.id`); callers thread that into
/// downstream DB writes / state lookups. The JWT itself is no longer
/// needed past this point.
pub async fn resolve_user_id(
    auth: &crate::auth::AuthMode,
    db: &sqlx::PgPool,
    token: Option<&str>,
) -> anyhow::Result<String> {
    match auth {
        crate::auth::AuthMode::Disabled => {
            let row = crate::storage::users::upsert_user_by_auth0_sub(
                db,
                crate::auth::DEV_AUTH0_SUB,
                Some("dev@local"),
                Some("Local Dev"),
            )
            .await?;
            Ok(row.id)
        }
        crate::auth::AuthMode::Live { auth0, auris } => {
            let token = token.ok_or_else(|| anyhow!("missing bearer token"))?;
            // Peek `iss` without verifying. The chosen validator
            // verifies the signature; the peek only decides which
            // validator to call.
            let iss = crate::auth::pairing::peek_issuer(token);
            if iss.as_deref() == Some(crate::auth::pairing::JWT_ISSUER) {
                // Paired-device path. HS256 verify + device-revocation check.
                let claims = auris.verify(token)?;
                if !crate::auth::pairing::assert_device_active(db, &claims.device_id).await? {
                    return Err(anyhow!("paired device revoked or unknown"));
                }
                // The token carries the same `sub` Auth0 would return
                // for this user. Upsert so `last_seen_at` ticks and a
                // missing-row scenario surfaces here rather than as a
                // foreign-key violation downstream.
                let row =
                    crate::storage::users::upsert_user_by_auth0_sub(db, &claims.sub, None, None)
                        .await?;
                Ok(row.id)
            } else {
                // Auth0 path. JWKS verify + claim upsert.
                let claims = auth0.validate(token).await?;
                let row = crate::storage::users::upsert_user_by_auth0_sub(
                    db,
                    &claims.sub,
                    claims.email.as_deref(),
                    claims.name.as_deref(),
                )
                .await?;
                Ok(row.id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validator_construction_normalises_domain() {
        // Trailing slashes and stray whitespace shouldn't break boot.
        let v = AuthValidator::new("  example.auth0.com/  ", "https://api.example/").unwrap();
        assert_eq!(v.inner.issuer, "https://example.auth0.com/");
        assert!(v.inner.jwks_url.ends_with("/.well-known/jwks.json"));
    }

    #[test]
    fn validator_rejects_empty_domain() {
        let res = AuthValidator::new("", "https://api.example/");
        assert!(res.is_err());
    }

    // ─────────────────────────────────────────────────────────────────
    // Fixtures for validate() / resolve_user_id() coverage. The RSA key
    // is a TEST-ONLY fixture (`openssl genrsa -traditional 2048`); it is
    // not a secret. FIXTURE_N is its base64url modulus.
    // ─────────────────────────────────────────────────────────────────

    use jsonwebtoken::{encode, EncodingKey, Header};

    const TEST_DOMAIN: &str = "test-tenant.invalid";
    const TEST_AUDIENCE: &str = "test-aud";
    const KID_1: &str = "fixture-kid-1";
    const KID_2: &str = "fixture-kid-2";

    const FIXTURE_RSA_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAvG8JpVaAX9S6JFjzf9w+E09G8CgCkzqQQlgFhJ4vDWGHQupH
hxco9z0S1tK+Ag0T/EYr4awlTSmWshpW9NAPrnXACN7fY/rz1zemw8mq1kPTbfs4
jmH+nlhuZx45ZJlwM5ptuAX3kq2ceZvOgkfxMpEEMYZ1aFrNAjKTFIGgS3ETKKl+
MyENXtPCs9iT4lVHng4SDgs9YZPjr0RZESnWExAoB4hoFDKBX+WmxMjny9CUCjUK
j2sMQbYGkNvQSWncR/d2GCiyNkWNSfdS2vcQEg1tagK+oArohK00A0g/jRjKCV1t
mwW4Ao8tL10XIHZKOpmVhY0r3d39f17k4iKlqQIDAQABAoIBAEQu4YykKjuC2//+
980SQpv2GbMYpyXjEQQQmZ6NJnVvDuSVpWAqbzZXMRPumbZgSRUFxycXhT/QhqjX
gxN+nc4A6YXML4Ub71O23W8G7/wr+rtXJfXPW7SsRvalJxtRshnaDU2DSzwV+gK+
8BCQO6SSeLP69UzXXksnRbUr2naxSPvygePKle9V9+7xOJ6KGtFt0jlY6SonDP2Q
mJlW85mjyqUUijEJEc+/OZL8iAs7wM3mmOc+OvR1KHRKQUdlVVVcn8Fm+cc4SezU
mIyf05EV348V2HFdSJQbGDgwSJubV3QOGrcnpKBSyOXiJkySG8bONdg5srqn4Ok9
8HIcMgkCgYEA/3BD3r2SP5YL+9aKnHPrHXGFESb752JxXecdJ0/3FKg2NKGKK9SA
V+vygmFLHJuuwPALAqia233HsQdHYL2Kq3kM4GcEGUjvTiRHa9VbJ1KrguQY9mO9
2nqpKPhFCVUX/2koTaVyPcAm/aOWMfC8fSCDDVoQZSoDiFu6Fziww4MCgYEAvNkR
rijBPhP1wEqE4E9yKQNa3jytEkxtTNc6UmaAO/ASc0q2G+OZLCSX0jZjLtTOVVGo
lh7qP4wpREVYhsKTa9nihPV9sGpjmVG0a+ustGFnkN5VTv27/PIpZDLMcwptA4uS
WEaPArvs490Y9ZDhEh4EhLBnej8QKHr9gEJMrmMCgYEA7rq7k7bUsjzHomyWSzZD
LNdlp+wpTc1BaqOPKaigoVu8nV/ERMZr1MAdfCD2FBykLImroKZ3ZF+ffCHzYcSD
j1Ko6CkfOYpirUNWxL84W/31cXVApzX8v+4XnsS5sMkojnp3Qmo35OJrDm4O90mo
v8Dc+mOMIyArAQvJVd6TxYUCgYA5uR/uXAa1MuSrIhv7dE0wvBXKWEGOlk3Sbvck
uK/5oigBlZSUcb0gAQ9m8bjfV6y553vgZxKy2eTDOW8VwePN04upmGASzHIlKxQ6
6I6hlCRT46Gvw17yshJ0zhIwF7+6la7lzKtp6oc+HxbB+MbTAtnetQzsENqfhPh3
e8x0gQKBgGklKiE/COtKd7lPrTQgYfxjFGm2UzN1NlsT4rg/0THjurE8R+tpf/dY
0U8NZflfeNScSAK8rZxMebc+7M2TjbVp5ZbHQ3iLKEDysF0XpQ0k4Q1vDTjRj1X4
OIE4/J/DWbyNTVBbL6NRbPSpUnI9nlZ7JmaVNr8zawlqIFNPtvTD
-----END RSA PRIVATE KEY-----
";

    const FIXTURE_N: &str = "vG8JpVaAX9S6JFjzf9w-E09G8CgCkzqQQlgFhJ4vDWGHQupHhxco9z0S1tK-Ag0T_EYr4awlTSmWshpW9NAPrnXACN7fY_rz1zemw8mq1kPTbfs4jmH-nlhuZx45ZJlwM5ptuAX3kq2ceZvOgkfxMpEEMYZ1aFrNAjKTFIGgS3ETKKl-MyENXtPCs9iT4lVHng4SDgs9YZPjr0RZESnWExAoB4hoFDKBX-WmxMjny9CUCjUKj2sMQbYGkNvQSWncR_d2GCiyNkWNSfdS2vcQEg1tagK-oArohK00A0g_jRjKCV1tmwW4Ao8tL10XIHZKOpmVhY0r3d39f17k4iKlqQ";
    const FIXTURE_E: &str = "AQAB";

    /// The issuer the validator expects for `TEST_DOMAIN`.
    fn test_issuer() -> String {
        format!("https://{TEST_DOMAIN}/")
    }

    fn unix_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[derive(serde::Serialize)]
    struct TestClaims {
        sub: String,
        email: Option<String>,
        name: Option<String>,
        exp: u64,
        iss: String,
        aud: String,
    }

    fn test_claims(sub: &str, iss: &str, aud: &str, exp: u64) -> TestClaims {
        TestClaims {
            sub: sub.to_string(),
            email: Some("fixture@test.invalid".to_string()),
            name: Some("Fixture User".to_string()),
            exp,
            iss: iss.to_string(),
            aud: aud.to_string(),
        }
    }

    /// Sign an RS256 token with the fixture key, Auth0-style.
    fn mint_rs256_token(sub: &str, kid: Option<&str>, iss: &str, aud: &str, exp: u64) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = kid.map(str::to_string);
        let key =
            EncodingKey::from_rsa_pem(FIXTURE_RSA_PEM.as_bytes()).expect("fixture PEM parses");
        encode(&header, &test_claims(sub, iss, aud, exp), &key).expect("sign RS256")
    }

    /// Sign an HS256 token. `kid` is settable so tests can reach the
    /// alg check (kid is extracted before alg in `validate`).
    fn mint_hs256_token(kid: Option<&str>, iss: &str, aud: &str, exp: u64) -> String {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = kid.map(str::to_string);
        let key = EncodingKey::from_secret(b"unit-test-hs256-secret");
        encode(
            &header,
            &test_claims("auth0|hs256-test", iss, aud, exp),
            &key,
        )
        .expect("sign HS256")
    }

    /// Mutable key set served by the stub: `(kid, n, e)` triples.
    type StubKeys = Arc<RwLock<Vec<(String, String, String)>>>;

    fn stub_keys(kids: &[&str]) -> StubKeys {
        Arc::new(RwLock::new(
            kids.iter()
                .map(|kid| {
                    (
                        kid.to_string(),
                        FIXTURE_N.to_string(),
                        FIXTURE_E.to_string(),
                    )
                })
                .collect(),
        ))
    }

    /// One-route axum server on 127.0.0.1:0 serving the JWKS document.
    async fn spawn_jwks_stub(
        keys: StubKeys,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        use axum::routing::get;
        use axum::{Json, Router};
        let app = Router::new().route(
            "/.well-known/jwks.json",
            get(move || {
                let keys = keys.clone();
                async move {
                    let keys = keys.read().await;
                    let entries: Vec<serde_json::Value> = keys
                        .iter()
                        .map(|(kid, n, e)| {
                            serde_json::json!({
                                "kid": kid, "kty": "RSA", "n": n, "e": e,
                                "alg": "RS256", "use": "sig",
                            })
                        })
                        .collect();
                    Json(serde_json::json!({ "keys": entries }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind jwks stub");
        let addr = listener.local_addr().expect("stub addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (addr, handle)
    }

    fn validator_for_stub(addr: std::net::SocketAddr) -> AuthValidator {
        AuthValidator::new_with_jwks_url(
            TEST_DOMAIN,
            TEST_AUDIENCE,
            format!("http://{addr}/.well-known/jwks.json"),
        )
        .expect("stub-backed validator")
    }

    #[test]
    fn fixture_modulus_matches_pem() {
        let token = mint_rs256_token(
            "auth0|fixture-check",
            Some(KID_1),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        let key = DecodingKey::from_rsa_components(FIXTURE_N, FIXTURE_E)
            .expect("FIXTURE_N/FIXTURE_E parse as RSA components");
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[test_issuer()]);
        validation.set_audience(&[TEST_AUDIENCE]);
        decode::<Claims>(&token, &key, &validation)
            .expect("FIXTURE_N must be the modulus of FIXTURE_RSA_PEM — regenerate both together");
    }

    #[tokio::test]
    async fn validate_accepts_valid_rs256_token() {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let v = validator_for_stub(addr);
        let token = mint_rs256_token(
            "auth0|accept-test",
            Some(KID_1),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        let claims = v.validate(&token).await.expect("valid token must verify");
        assert_eq!(claims.sub, "auth0|accept-test");
        assert_eq!(claims.email.as_deref(), Some("fixture@test.invalid"));
        assert_eq!(claims.name.as_deref(), Some("Fixture User"));
    }

    /// Flip one char in the middle of the signature segment.
    fn tamper_signature(token: &str) -> String {
        let (head, sig) = token.rsplit_once('.').expect("JWT has a signature segment");
        let mut sig: Vec<char> = sig.chars().collect();
        let i = sig.len() / 2;
        sig[i] = if sig[i] == 'A' { 'B' } else { 'A' };
        format!("{head}.{}", sig.into_iter().collect::<String>())
    }

    #[tokio::test]
    async fn validate_rejects_wrong_audience() {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let v = validator_for_stub(addr);
        let token = mint_rs256_token(
            "auth0|aud-test",
            Some(KID_1),
            &test_issuer(),
            "some-other-aud",
            unix_now() + 600,
        );
        let err = v.validate(&token).await.unwrap_err();
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("audience"),
            "expected audience rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn validate_rejects_wrong_issuer() {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let v = validator_for_stub(addr);
        let token = mint_rs256_token(
            "auth0|iss-test",
            Some(KID_1),
            "https://evil-tenant.invalid/",
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        let err = v.validate(&token).await.unwrap_err();
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("issuer"),
            "expected issuer rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn validate_rejects_expired_token() {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let v = validator_for_stub(addr);
        let token = mint_rs256_token(
            "auth0|exp-test",
            Some(KID_1),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() - 120,
        );
        let err = v.validate(&token).await.unwrap_err();
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("expired"),
            "expected expiry rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn validate_rejects_tampered_signature() {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let v = validator_for_stub(addr);
        let good = mint_rs256_token(
            "auth0|sig-test",
            Some(KID_1),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        v.validate(&good).await.expect("control token must verify");
        let err = v.validate(&tamper_signature(&good)).await.unwrap_err();
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("signature"),
            "expected signature rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn validate_rejects_non_rs256_alg() {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let v = validator_for_stub(addr);
        let token = mint_hs256_token(Some(KID_1), &test_issuer(), TEST_AUDIENCE, unix_now() + 600);
        let err = v.validate(&token).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unexpected JWT alg"),
            "expected alg rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn validate_rejects_missing_kid() {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let v = validator_for_stub(addr);
        let token = mint_rs256_token(
            "auth0|nokid-test",
            None,
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        let err = v.validate(&token).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing `kid`"),
            "expected kid rejection, got: {msg}"
        );
    }

    #[tokio::test]
    async fn validate_rejects_unknown_kid_within_cooldown() {
        let keys = stub_keys(&[KID_1]);
        let (addr, _stub) = spawn_jwks_stub(keys.clone()).await;
        let v = validator_for_stub(addr);
        let warm = mint_rs256_token(
            "auth0|cooldown-warm",
            Some(KID_1),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        v.validate(&warm).await.expect("warm-up token verifies");
        keys.write().await.push((
            KID_2.to_string(),
            FIXTURE_N.to_string(),
            FIXTURE_E.to_string(),
        ));
        let rotated = mint_rs256_token(
            "auth0|cooldown-miss",
            Some(KID_2),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        let err = v.validate(&rotated).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown kid"),
            "expected cooldown-suppressed refetch -> unknown kid, got: {msg}"
        );
    }

    #[tokio::test]
    async fn validate_refetches_jwks_on_kid_rotation() {
        let keys = stub_keys(&[KID_1]);
        let (addr, _stub) = spawn_jwks_stub(keys.clone()).await;
        let v = validator_for_stub(addr);
        let warm = mint_rs256_token(
            "auth0|rotation-warm",
            Some(KID_1),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        v.validate(&warm).await.expect("warm-up token verifies");
        *keys.write().await = vec![(
            KID_2.to_string(),
            FIXTURE_N.to_string(),
            FIXTURE_E.to_string(),
        )];
        {
            let mut cache = v.inner.cache.write().await;
            cache.last_fetched_at =
                Instant::now().checked_sub(JWKS_REFETCH_COOLDOWN + Duration::from_secs(1));
        }
        let rotated = mint_rs256_token(
            "auth0|rotation-new",
            Some(KID_2),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        let claims = v
            .validate(&rotated)
            .await
            .expect("kid rotation must trigger a refetch and then verify");
        assert_eq!(claims.sub, "auth0|rotation-new");
        let err = v.validate(&warm).await.unwrap_err();
        assert!(format!("{err:#}").contains("unknown kid"));
    }

    // ─────────────────────────────────────────────────────────────────
    // resolve_user_id(): the iss-peek dispatch fronting all auth surfaces.
    // ─────────────────────────────────────────────────────────────────

    use crate::auth::pairing::{self, AurisJwtIssuer};
    use crate::auth::AuthMode;
    use sqlx::PgPool;

    fn live_mode_with_dead_jwks() -> (AuthMode, AurisJwtIssuer) {
        let auth0 = AuthValidator::new(TEST_DOMAIN, TEST_AUDIENCE).expect("validator");
        let auris = AurisJwtIssuer::new(b"resolve-test-secret-0123456789abcdef").expect("issuer");
        (
            AuthMode::Live {
                auth0,
                auris: auris.clone(),
            },
            auris,
        )
    }

    async fn provision_device(
        pool: &PgPool,
        issuer: &AurisJwtIssuer,
    ) -> (crate::storage::users::UserRow, pairing::RedeemedPair) {
        let sub = format!("test|{}", uuid::Uuid::new_v4());
        let user = crate::storage::users::upsert_user_by_auth0_sub(pool, &sub, None, None)
            .await
            .expect("upsert user");
        let code = pairing::mint_code(pool, &user.id).await.expect("mint code");
        let pair = pairing::redeem_code(pool, issuer, &code.code, Some("validator-test".into()))
            .await
            .expect("redeem code");
        (user, pair)
    }

    #[sqlx::test]
    async fn resolve_disabled_maps_to_dev_user(pool: PgPool) {
        let id = resolve_user_id(&AuthMode::Disabled, &pool, None)
            .await
            .expect("disabled mode resolves without a token");
        let (sub,): (String,) = sqlx::query_as("SELECT auth0_sub FROM users WHERE id = $1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("dev user row exists");
        assert_eq!(sub, crate::auth::DEV_AUTH0_SUB);
        let id2 = resolve_user_id(&AuthMode::Disabled, &pool, None)
            .await
            .unwrap();
        assert_eq!(id, id2);
    }

    #[sqlx::test]
    async fn resolve_live_rejects_missing_token(pool: PgPool) {
        let (mode, _) = live_mode_with_dead_jwks();
        let err = resolve_user_id(&mode, &pool, None).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("missing bearer token"),
            "got: {err:#}"
        );
    }

    #[sqlx::test]
    async fn resolve_live_auris_token_active_device_resolves(pool: PgPool) {
        let (mode, issuer) = live_mode_with_dead_jwks();
        let (user, pair) = provision_device(&pool, &issuer).await;
        let id = resolve_user_id(&mode, &pool, Some(&pair.access_token))
            .await
            .expect("active paired device must resolve");
        assert_eq!(id, user.id, "must resolve to the paired user's row");
    }

    #[sqlx::test]
    async fn resolve_live_auris_token_revoked_device_rejected(pool: PgPool) {
        let (mode, issuer) = live_mode_with_dead_jwks();
        let (user, pair) = provision_device(&pool, &issuer).await;
        resolve_user_id(&mode, &pool, Some(&pair.access_token))
            .await
            .expect("pre-revocation control");
        let revoked = pairing::revoke_device(&pool, &user.id, &pair.device_id)
            .await
            .expect("revoke");
        assert_eq!(revoked, 1, "exactly one device revoked");
        let err = resolve_user_id(&mode, &pool, Some(&pair.access_token))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("revoked"), "got: {err:#}");
    }

    #[sqlx::test]
    async fn resolve_live_forged_auris_token_rejected(pool: PgPool) {
        let (mode, _) = live_mode_with_dead_jwks();
        let forger =
            AurisJwtIssuer::new(b"attacker-secret-0123456789abcdef-xx").expect("forger issuer");
        let token = forger
            .mint("auth0|victim", "some-device", chrono::Utc::now())
            .expect("forge");
        let err = resolve_user_id(&mode, &pool, Some(&token))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("verify auris JWT"),
            "expected device-path signature rejection, got: {err:#}"
        );
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE auth0_sub = $1")
            .bind("auth0|victim")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "forged token must not upsert a user");
    }

    #[sqlx::test]
    async fn resolve_live_foreign_hs256_falls_through_to_auth0(pool: PgPool) {
        let (mode, _) = live_mode_with_dead_jwks();
        let token = mint_hs256_token(None, "evil", TEST_AUDIENCE, unix_now() + 600);
        let err = resolve_user_id(&mode, &pool, Some(&token))
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing `kid`"),
            "expected Auth0-path rejection, got: {msg}"
        );
        assert!(
            !msg.contains("verify auris JWT"),
            "foreign issuer must not reach the device path: {msg}"
        );
    }

    #[sqlx::test]
    async fn resolve_live_auth0_token_upserts_user(pool: PgPool) {
        let (addr, _stub) = spawn_jwks_stub(stub_keys(&[KID_1])).await;
        let auth0 = validator_for_stub(addr);
        let auris = AurisJwtIssuer::new(b"resolve-test-secret-0123456789abcdef").expect("issuer");
        let mode = AuthMode::Live { auth0, auris };
        let sub = format!("auth0|{}", uuid::Uuid::new_v4());
        let token = mint_rs256_token(
            &sub,
            Some(KID_1),
            &test_issuer(),
            TEST_AUDIENCE,
            unix_now() + 600,
        );
        let id = resolve_user_id(&mode, &pool, Some(&token))
            .await
            .expect("valid Auth0 token must resolve");
        let (db_sub, email, name): (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT auth0_sub, email, name FROM users WHERE id = $1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .expect("upserted user row");
        assert_eq!(db_sub, sub);
        assert_eq!(email.as_deref(), Some("fixture@test.invalid"));
        assert_eq!(name.as_deref(), Some("Fixture User"));
    }
}
