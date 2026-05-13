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
        let domain = domain.trim().trim_end_matches('/');
        if domain.is_empty() {
            return Err(anyhow!("AUTH0_DOMAIN is empty"));
        }
        let issuer = format!("https://{domain}/");
        let jwks_url = format!("https://{domain}/.well-known/jwks.json");
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

/// Resolve a request's bearer token to a local `users.id`. Two paths:
///
/// - `AuthMode::Disabled`: bypass — every caller maps to a fixed dev
///   user upserted on first call.
/// - `AuthMode::Live`: validate the JWT against Auth0, then upsert
///   the user from the claims.
///
/// Returns the local UUID (`users.id`); callers thread that into
/// downstream DB writes / state lookups. The JWT itself is no longer
/// needed past this point.
pub async fn resolve_user_id(
    auth: &crate::ws::AuthMode,
    db: &sqlx::PgPool,
    token: Option<&str>,
) -> anyhow::Result<String> {
    match auth {
        crate::ws::AuthMode::Disabled => {
            let row = crate::db::upsert_user_by_auth0_sub(
                db,
                crate::ws::DEV_AUTH0_SUB,
                Some("dev@local"),
                Some("Local Dev"),
            )
            .await?;
            Ok(row.id)
        }
        crate::ws::AuthMode::Live(validator) => {
            let token = token.ok_or_else(|| anyhow!("missing bearer token"))?;
            let claims = validator.validate(token).await?;
            let row = crate::db::upsert_user_by_auth0_sub(
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
}
