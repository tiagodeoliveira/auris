//! Device-pairing flow.
//!
//! The PWA on EvenHub glasses cannot use Auth0's redirect-callback
//! flow (the .ehpk loads at `http://127.0.0.1:<random-port>/` and
//! Auth0 doesn't honor wildcard ports for any client type we have).
//! Instead:
//!
//!   1. Mobile (authed via Auth0) calls `POST /pair/code` →
//!      `mint_code()` returns an 8-char short-lived code.
//!   2. PWA submits the code to `POST /pair/redeem` →
//!      `redeem_code()` consumes it, inserts a `paired_devices` row,
//!      mints an HS256 access JWT and a 32-byte refresh token, and
//!      returns both.
//!   3. PWA refreshes via `POST /pair/refresh` →
//!      `rotate_refresh()` looks up the device by hash, mints a
//!      fresh pair, and rotates the stored hash.
//!
//! This module owns the crypto + DB ops. HTTP handlers in `api.rs`
//! own the wire format + rate limits + error mapping.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration as StdDuration, Instant};

use anyhow::{anyhow, Context, Result};
use argon2::{
    password_hash::{rand_core::OsRng as Argon2Rng, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, PasswordHash,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────

/// Code alphabet — 31 characters, ambiguous pairs removed (no
/// `0`/`O`, `1`/`I`/`L`). We sample with modulo over a `u8`; the
/// resulting bias (`256 mod 31 = 8` overrepresented values) is
/// `~0.4%` per char — negligible against the entropy headroom we
/// already have at `CODE_LEN = 8`.
pub const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const _: () = assert!(ALPHABET.len() == 31, "alphabet size invariant");

/// Code length in chars. `31^8 ≈ 8.5 × 10^11` — comfortable headroom
/// even against an unconstrained brute force.
pub const CODE_LEN: usize = 8;

/// How long a freshly-minted pair code remains redeemable.
pub const CODE_TTL: Duration = Duration::minutes(5);

/// Access-token lifetime. Short enough that a leaked token doesn't
/// outlive the typical session by long; long enough that the PWA
/// doesn't paper the network with `/pair/refresh` requests.
pub const ACCESS_TTL: Duration = Duration::hours(1);

/// How long a successful `/pair/redeem` is replayable. The PWA inside
/// WKWebView occasionally throws `TypeError("Load failed")` even
/// after the server has processed the request — the row is created,
/// the code marked used, but the response body never reaches JS. A
/// retry with the same code would otherwise hit `invalid_code` and
/// strand the user. Caching the freshly-minted `RedeemedPair` here
/// for a brief window lets the retry replay safely. 60 s comfortably
/// covers user-initiated retries without keeping plaintext tokens in
/// memory longer than needed.
const REDEEM_CACHE_TTL: StdDuration = StdDuration::from_secs(60);

/// In-memory replay cache for `/pair/redeem`. Keyed by normalized
/// code so a retry from the same client (or a different one with the
/// same code on hand) gets the SAME tokens as the original request —
/// the device row was already created and minting a fresh pair would
/// orphan it. Module-private; only `redeem_code` reads/writes.
static REDEEM_CACHE: LazyLock<Mutex<HashMap<String, (Instant, RedeemedPair)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn redeem_cache_get(code: &str) -> Option<RedeemedPair> {
    let now = Instant::now();
    let mut cache = REDEEM_CACHE.lock().expect("redeem cache mutex poisoned");
    // Sweep expired entries on access — no separate background task
    // needed for a personal-scale deployment, and we only ever touch
    // the cache from `redeem_code` (low traffic, short window).
    cache.retain(|_, (ts, _)| now.duration_since(*ts) < REDEEM_CACHE_TTL);
    cache.get(code).map(|(_, pair)| pair.clone())
}

fn redeem_cache_put(code: String, pair: &RedeemedPair) {
    let mut cache = REDEEM_CACHE.lock().expect("redeem cache mutex poisoned");
    cache.insert(code, (Instant::now(), pair.clone()));
}

/// Refresh-token lifetime. No DB-level enforcement — the row
/// persists; a stale refresh just fails when the user pairs again
/// and we never look the old hash up. UX-wise, a 90-day inactive
/// device shows the pair screen on next launch.
pub const REFRESH_TTL: Duration = Duration::days(90);

/// Grace window during which the *previous* refresh-token hash is
/// still accepted after a rotation. Covers the case where the client
/// rotated server-side but failed to persist the new token (flaky
/// bridge KV) — its next refresh presents the old token and must
/// still succeed. Sized comfortably above `ACCESS_TTL` so a client
/// that persisted the new access token but not the new refresh token
/// (and therefore doesn't refresh again until the access token
/// expires) still lands inside the window.
pub const REFRESH_GRACE: Duration = Duration::hours(48);

/// JWT issuer claim for Auris-minted tokens. Used by the auth
/// extractor to dispatch to the right validator (vs. Auth0's
/// `https://<tenant>/`).
pub const JWT_ISSUER: &str = "auris-server";

/// JWT audience claim. Distinct from Auth0's audience so a leaked
/// Auth0 access token can't accidentally be accepted as an Auris
/// token (and vice versa).
pub const JWT_AUDIENCE: &str = "auris-api";

/// Default human-readable label when the PWA didn't send one on
/// redeem. Shown in mobile's "Paired devices" list.
pub const DEFAULT_DEVICE_LABEL: &str = "G2 glasses";

// ─────────────────────────────────────────────────────────────────────
// Code generation / normalization / display formatting
// ─────────────────────────────────────────────────────────────────────

/// Sample `CODE_LEN` random chars from `ALPHABET`. Uses the OS CSPRNG
/// so the codes are unguessable in the strong sense. The modulo bias
/// from `31 < 256` is `~0.4%` per char — negligible at this scale,
/// and absorbed into the entropy headroom.
pub fn generate_code() -> String {
    let mut rng = OsRng;
    let mut buf = [0u8; CODE_LEN];
    rng.fill_bytes(&mut buf);
    buf.iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

/// Canonical form for DB storage + lookup. Uppercases (so we can
/// accept lowercase user input) and strips dashes / whitespace
/// (we display `XXXX-XXXX` but store `XXXXXXXX`). Caller still
/// has to validate length+alphabet via `is_valid_normalized` —
/// this just rearranges characters.
pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// True iff the input (already normalized) is the right length AND
/// every char is in our alphabet. Used to fast-reject malformed
/// input before hitting the DB.
pub fn is_valid_normalized(code: &str) -> bool {
    code.len() == CODE_LEN && code.bytes().all(|b| ALPHABET.contains(&b))
}

/// Display form: split in the middle with a dash for readability
/// (`K7M2-4XQ9`). Storage stays unhyphenated; this is purely UX.
pub fn format_code(code: &str) -> String {
    if code.len() != CODE_LEN {
        return code.to_string();
    }
    let mid = CODE_LEN / 2;
    format!("{}-{}", &code[..mid], &code[mid..])
}

// ─────────────────────────────────────────────────────────────────────
// Refresh tokens: generate / hash / verify
// ─────────────────────────────────────────────────────────────────────

/// Length of the raw refresh-token entropy. 32 bytes = 256 bits, the
/// usual ceiling for symmetric secrets.
pub const REFRESH_TOKEN_BYTES: usize = 32;

/// Generate a fresh refresh token. Returned base64url-encoded so it
/// fits cleanly into JSON / Authorization headers without escaping.
pub fn generate_refresh_token() -> String {
    let mut buf = [0u8; REFRESH_TOKEN_BYTES];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Argon2id-hash a refresh token for storage. Each token gets a
/// fresh random salt; the resulting PHC string is self-describing
/// (algorithm + parameters + salt + hash all inline) so we don't
/// need a separate `salt` column.
pub fn hash_refresh_token(token: &str) -> Result<String> {
    let salt = SaltString::generate(&mut Argon2Rng);
    let argon = Argon2::default();
    let hash = argon
        .hash_password(token.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash failed: {e}"))?
        .to_string();
    Ok(hash)
}

/// Verify a refresh token against its stored hash.
///
/// Returns `Ok(true)` on match, `Ok(false)` on a well-formed
/// mismatch. `Err` is reserved for the case where the stored hash
/// itself is malformed (corruption / migration bug) — callers should
/// treat that as a server error, not a user-facing 401.
pub fn verify_refresh_token(token: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow!("parse stored hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(token.as_bytes(), &parsed)
        .is_ok())
}

// ─────────────────────────────────────────────────────────────────────
// JWT mint / verify (HS256)
// ─────────────────────────────────────────────────────────────────────

/// Claims carried by an Auris-issued access token. Shape mirrors the
/// subset of Auth0 claims that downstream handlers actually use, so
/// the auth extractor can produce a single unified `Claims` regardless
/// of issuer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AurisClaims {
    /// Issuer — always `JWT_ISSUER`. Used by the dispatch layer.
    pub iss: String,
    /// `users.auth0_sub` — same value Auth0 would return for the
    /// same user, so downstream code is issuer-agnostic.
    pub sub: String,
    /// Audience — always `JWT_AUDIENCE`. Validated on verify.
    pub aud: String,
    /// `paired_devices.device_id` — uniquely identifies the device
    /// holding this token. Used for revocation + last-seen updates.
    pub device_id: String,
    /// Issued at (unix seconds).
    pub iat: u64,
    /// Expiry (unix seconds). Validated on verify with default
    /// 60-second clock skew.
    pub exp: u64,
}

/// HS256 signing key wrapper. Created once at boot from the
/// `AURIS_JWT_HS256_SECRET` env var; held as `Arc` inside the
/// auth-mode enum so request handlers can mint + verify cheaply.
#[derive(Clone)]
pub struct AurisJwtIssuer {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl AurisJwtIssuer {
    /// Construct from a secret. Minimum length enforced: short
    /// secrets weaken HS256 dramatically (the secret is the key).
    /// 32 bytes matches the entropy of the refresh tokens we issue.
    pub fn new(secret: &[u8]) -> Result<Self> {
        if secret.len() < 32 {
            return Err(anyhow!(
                "AURIS_JWT_HS256_SECRET must be at least 32 bytes (got {})",
                secret.len()
            ));
        }
        Ok(Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
        })
    }

    /// Mint a fresh access token. `now` is injectable for tests;
    /// production callers pass `Utc::now()`.
    pub fn mint(&self, sub: &str, device_id: &str, now: DateTime<Utc>) -> Result<String> {
        let iat = now.timestamp() as u64;
        let exp = (now + ACCESS_TTL).timestamp() as u64;
        let claims = AurisClaims {
            iss: JWT_ISSUER.to_string(),
            sub: sub.to_string(),
            aud: JWT_AUDIENCE.to_string(),
            device_id: device_id.to_string(),
            iat,
            exp,
        };
        encode(&Header::default(), &claims, &self.encoding).context("encode auris JWT")
    }

    /// Verify a token. Checks signature, issuer, audience, and
    /// expiry (with the default clock skew that `jsonwebtoken`
    /// applies on `exp`).
    pub fn verify(&self, token: &str) -> Result<AurisClaims> {
        let mut validation = Validation::default();
        validation.set_issuer(&[JWT_ISSUER]);
        validation.set_audience(&[JWT_AUDIENCE]);
        validation.validate_exp = true;
        let data = decode::<AurisClaims>(token, &self.decoding, &validation)
            .context("verify auris JWT")?;
        Ok(data.claims)
    }
}

/// Peek at a token's `iss` claim without verifying the signature.
/// Used by the dispatch layer to route to Auth0 vs Auris validators.
/// Returns `None` if the token isn't structurally a JWT or lacks an
/// `iss` claim — callers should treat that as a hard reject.
pub fn peek_issuer(token: &str) -> Option<String> {
    // JWT format: header.payload.signature, all base64url-encoded.
    let payload_b64 = token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    #[derive(Deserialize)]
    struct IssOnly {
        iss: Option<String>,
    }
    let parsed: IssOnly = serde_json::from_slice(&payload).ok()?;
    parsed.iss
}

// ─────────────────────────────────────────────────────────────────────
// DB ops — pair codes
// ─────────────────────────────────────────────────────────────────────

/// A freshly-minted or re-fetched code, ready to return to the user.
#[derive(Debug, Clone)]
pub struct MintedCode {
    /// Unhyphenated storage form. Caller passes through `format_code`
    /// before showing to the user.
    pub code: String,
    pub expires_at: DateTime<Utc>,
}

/// Mint a code for `user_id`. Idempotent within the active window:
/// if the user already has an unused, unexpired code, return it
/// instead of minting a second one. Prevents the "I tapped twice and
/// now I have two codes" confusion.
pub async fn mint_code(db: &PgPool, user_id: &str) -> Result<MintedCode> {
    // Existing active code?
    let existing: Option<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT code, expires_at \
         FROM pair_codes \
         WHERE user_id = $1 AND used_at IS NULL AND expires_at > NOW() \
         ORDER BY created_at DESC \
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .context("lookup existing pair_code")?;
    if let Some((code, expires_at)) = existing {
        return Ok(MintedCode { code, expires_at });
    }

    // Mint a new one. Retry on PK collision (vanishingly unlikely
    // given the alphabet size, but the cost of a retry is one SQL
    // round-trip vs. losing a user's pair attempt to a 500).
    let now = Utc::now();
    let expires_at = now + CODE_TTL;
    for _ in 0..5 {
        let code = generate_code();
        let res = sqlx::query(
            "INSERT INTO pair_codes (code, user_id, created_at, expires_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&code)
        .bind(user_id)
        .bind(now)
        .bind(expires_at)
        .execute(db)
        .await;
        match res {
            Ok(_) => return Ok(MintedCode { code, expires_at }),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
            Err(e) => return Err(anyhow::Error::from(e).context("insert pair_code")),
        }
    }
    Err(anyhow!("pair_code generation collided 5 times in a row"))
}

/// Result of a successful redeem.
#[derive(Debug, Clone)]
pub struct RedeemedPair {
    pub device_id: String,
    pub user_id: String,
    pub access_token: String,
    /// Plaintext — the only chance the PWA has to capture it.
    pub refresh_token: String,
    pub access_expires_at: DateTime<Utc>,
}

/// Atomically consume a code and provision a new paired device.
///
/// All three rows (mark code used, insert paired_devices, link the
/// two) happen in one transaction. If anything fails, none of it
/// commits — so a partially-redeemed code can't strand a half-
/// configured device.
///
/// `code_input` may be the user's typed form (mixed case, with
/// dashes). We normalize before lookup.
pub async fn redeem_code(
    db: &PgPool,
    issuer: &AurisJwtIssuer,
    code_input: &str,
    device_label: Option<String>,
) -> Result<RedeemedPair> {
    let code = normalize_code(code_input);
    if !is_valid_normalized(&code) {
        return Err(anyhow!("invalid_code"));
    }

    // Idempotent retry. If this code was redeemed successfully in the
    // last `REDEEM_CACHE_TTL`, return the same tokens — covers the
    // WKWebView "Load failed" case where the response body got
    // dropped client-side even though the server fully processed the
    // request. Without this, the second click on Pair would hit
    // `invalid_code` (the DB row is marked used) and strand the user.
    if let Some(cached) = redeem_cache_get(&code) {
        return Ok(cached);
    }

    let mut tx = db.begin().await.context("begin redeem tx")?;

    // SELECT FOR UPDATE locks the pair_code row so a concurrent
    // redeem of the same code blocks here and (on commit) sees
    // `used_at IS NOT NULL`. JOIN users to pull the auth0_sub for
    // the JWT — pair_codes.user_id is the internal UUID (FK to
    // users.id), but the JWT's `sub` claim must be the auth0_sub
    // so the dual-issuer extractor's upsert_user_by_auth0_sub call
    // finds the same user the mobile client paired from.
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT pc.user_id, u.auth0_sub \
         FROM pair_codes pc \
         JOIN users u ON u.id = pc.user_id \
         WHERE pc.code = $1 AND pc.used_at IS NULL AND pc.expires_at > NOW() \
         FOR UPDATE OF pc",
    )
    .bind(&code)
    .fetch_optional(&mut *tx)
    .await
    .context("lookup pair_code")?;
    let (user_id, auth0_sub) = row.ok_or_else(|| anyhow!("invalid_code"))?;

    let device_id = Uuid::new_v4().to_string();
    let label = device_label
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DEVICE_LABEL.to_string());

    let refresh_token = generate_refresh_token();
    let refresh_hash = hash_refresh_token(&refresh_token)?;

    sqlx::query(
        "INSERT INTO paired_devices \
            (device_id, user_id, device_label, refresh_token_hash) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&device_id)
    .bind(&user_id)
    .bind(&label)
    .bind(&refresh_hash)
    .execute(&mut *tx)
    .await
    .context("insert paired_device")?;

    sqlx::query(
        "UPDATE pair_codes \
         SET used_at = NOW(), used_by_device = $1 \
         WHERE code = $2",
    )
    .bind(&device_id)
    .bind(&code)
    .execute(&mut *tx)
    .await
    .context("mark pair_code used")?;

    tx.commit().await.context("commit redeem tx")?;

    let now = Utc::now();
    // JWT `sub` is the auth0_sub so downstream auth (extractor +
    // upsert_user_by_auth0_sub) resolves to the SAME user row the
    // mobile client paired from. paired_devices.user_id keeps the
    // internal UUID — that's the FK target.
    let access_token = issuer.mint(&auth0_sub, &device_id, now)?;
    let access_expires_at = now + ACCESS_TTL;
    let pair = RedeemedPair {
        device_id,
        user_id,
        access_token,
        refresh_token,
        access_expires_at,
    };
    // Stash the result so a retry within the replay window finds it.
    // Cache key is the normalized code — same key the lookup above
    // would use.
    redeem_cache_put(code, &pair);
    Ok(pair)
}

// ─────────────────────────────────────────────────────────────────────
// DB ops — paired devices
// ─────────────────────────────────────────────────────────────────────

/// Result of a successful refresh rotation. Shape mirrors `RedeemedPair`
/// minus the device_id (caller already has it).
#[derive(Debug, Clone)]
pub struct RotatedPair {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: DateTime<Utc>,
}

/// Decide whether `token` authenticates against one device's refresh
/// slots. Returns `Some(true)` if it matched the *current* hash,
/// `Some(false)` if it matched the *previous* hash within the grace
/// window, and `None` if neither. Pure (no DB) so the grace-window
/// boundary and match priority are unit-testable without a database.
///
/// The current hash is always verified; the previous hash is only
/// verified when it exists AND its rotation timestamp is inside the
/// grace window — so an expired previous token costs no argon2 work
/// and can never authenticate.
fn match_refresh_slot(
    token: &str,
    current_hash: &str,
    prev_hash: Option<&str>,
    prev_rotated_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Option<bool>> {
    if verify_refresh_token(token, current_hash)? {
        return Ok(Some(true));
    }
    if let (Some(ph), Some(rotated)) = (prev_hash, prev_rotated_at) {
        if rotated > now - REFRESH_GRACE && verify_refresh_token(token, ph)? {
            return Ok(Some(false));
        }
    }
    Ok(None)
}

/// One active paired-device row pulled for the refresh scan:
/// `(device_id, auth0_sub, refresh_token_hash, prev_refresh_token_hash,
/// prev_rotated_at)`.
type RefreshCandidate = (
    String,
    String,
    String,
    Option<String>,
    Option<DateTime<Utc>>,
);

/// Exchange a refresh token for a fresh access+refresh pair. The
/// old refresh-token hash is overwritten in the same transaction
/// so a replay of the original token fails the lookup.
///
/// Returns `None` when the token doesn't match any active device —
/// callers should respond 401 without leaking which condition failed
/// (unknown hash vs. revoked device vs. malformed token).
pub async fn rotate_refresh(
    db: &PgPool,
    issuer: &AurisJwtIssuer,
    refresh_token: &str,
) -> Result<Option<RotatedPair>> {
    // We can't query "by hash" directly because each token gets a
    // unique salt. Instead, scan active devices and verify against
    // each — bounded by N(active devices for this server). For
    // a personal-scale deployment this is fine; if we ever ship to
    // a tenant with thousands of devices, switch the schema to
    // store an HMAC-SHA256 of the token (deterministic, indexable)
    // and keep argon2 as a defense-in-depth wrapper.
    // JOIN users so we have the auth0_sub ready for JWT minting —
    // the device row keeps internal users.id as its FK, but the JWT
    // `sub` claim must match what the dual-issuer extractor will
    // look up (upsert_user_by_auth0_sub).
    let candidates: Vec<RefreshCandidate> = sqlx::query_as(
        "SELECT pd.device_id, u.auth0_sub, pd.refresh_token_hash, \
                pd.prev_refresh_token_hash, pd.prev_rotated_at \
         FROM paired_devices pd \
         JOIN users u ON u.id = pd.user_id \
         WHERE pd.revoked_at IS NULL",
    )
    .fetch_all(db)
    .await
    .context("scan paired_devices")?;

    // Constant-time across candidates: verify against every active
    // device hash regardless of match position. The early break would
    // leak the matched row's index through wall-clock timing (each
    // argon2 verify is ~50ms). Without it, total latency only depends
    // on `candidates.len()`, which is already inferable from
    // `/pair/devices`.
    //
    // We check both the current hash AND the previous hash (within the
    // grace window). `matched_via_current` distinguishes the two so the
    // update can either shift the chain (normal rotation) or hold the
    // previous hash (a lagging client recovering from a failed persist
    // — see below).
    let now = Utc::now();
    let mut matched: Option<(String, String, bool)> = None;
    for (device_id, auth0_sub, hash, prev_hash, prev_rotated_at) in candidates {
        let slot = match_refresh_slot(
            refresh_token,
            &hash,
            prev_hash.as_deref(),
            prev_rotated_at,
            now,
        )?;
        if let Some(via_current) = slot {
            if matched.is_none() {
                matched = Some((device_id, auth0_sub, via_current));
            }
        }
    }
    let Some((device_id, auth0_sub, matched_via_current)) = matched else {
        return Ok(None);
    };

    let new_refresh = generate_refresh_token();
    let new_hash = hash_refresh_token(&new_refresh)?;

    // Conditional update — only rotate if the device hasn't been
    // revoked in the meantime. The WHERE on `device_id` is the
    // primary key path; the `revoked_at IS NULL` is the gate.
    let updated = if matched_via_current {
        // Normal rotation: retire the current hash into the previous
        // slot (Postgres evaluates SET right-hand sides against the OLD
        // row, so `prev = refresh_token_hash` captures the outgoing
        // current) and stamp the grace timer.
        sqlx::query(
            "UPDATE paired_devices \
             SET prev_refresh_token_hash = refresh_token_hash, \
                 prev_rotated_at = NOW(), \
                 refresh_token_hash = $1, \
                 last_seen_at = NOW() \
             WHERE device_id = $2 AND revoked_at IS NULL",
        )
        .bind(&new_hash)
        .bind(&device_id)
        .execute(db)
        .await
        .context("rotate refresh hash")?
    } else {
        // Lagging client: it presented the PREVIOUS token because it
        // never persisted the last rotation. Mint a fresh current but
        // KEEP the previous hash (and its grace timer) untouched, so
        // the client can keep retrying with its old token until one
        // persist finally sticks. Without this, a second consecutive
        // failed persist would strand it again.
        sqlx::query(
            "UPDATE paired_devices \
             SET refresh_token_hash = $1, last_seen_at = NOW() \
             WHERE device_id = $2 AND revoked_at IS NULL",
        )
        .bind(&new_hash)
        .bind(&device_id)
        .execute(db)
        .await
        .context("rotate refresh hash (grace)")?
    };
    if updated.rows_affected() == 0 {
        // Revoked between the scan and the update.
        return Ok(None);
    }

    let now = Utc::now();
    let access_token = issuer.mint(&auth0_sub, &device_id, now)?;
    Ok(Some(RotatedPair {
        access_token,
        refresh_token: new_refresh,
        access_expires_at: now + ACCESS_TTL,
    }))
}

/// Device row shape returned by `GET /pair/devices`. Excludes the
/// hash — clients have no reason to see it.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceSummary {
    pub device_id: String,
    pub device_label: String,
    pub paired_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// List active (non-revoked) devices for a user, newest first.
pub async fn list_devices(db: &PgPool, user_id: &str) -> Result<Vec<DeviceSummary>> {
    let rows: Vec<(String, String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT device_id, device_label, paired_at, last_seen_at \
         FROM paired_devices \
         WHERE user_id = $1 AND revoked_at IS NULL \
         ORDER BY paired_at DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .context("list paired_devices")?;
    Ok(rows
        .into_iter()
        .map(
            |(device_id, device_label, paired_at, last_seen_at)| DeviceSummary {
                device_id,
                device_label,
                paired_at,
                last_seen_at,
            },
        )
        .collect())
}

/// Mark a device as revoked. Returns the number of rows changed
/// (0 if the device doesn't exist or doesn't belong to this user —
/// indistinguishable to the caller, by design).
pub async fn revoke_device(db: &PgPool, user_id: &str, device_id: &str) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE paired_devices \
         SET revoked_at = NOW() \
         WHERE device_id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(device_id)
    .bind(user_id)
    .execute(db)
    .await
    .context("revoke paired_device")?;
    Ok(res.rows_affected())
}

/// Verify that a device referenced by an Auris JWT is still active
/// (not revoked). The JWT signature alone proves the token was
/// minted by us; this extra check is what makes revocation
/// effective before the access token expires.
///
/// Also bumps `last_seen_at` as a best-effort side effect.
pub async fn assert_device_active(db: &PgPool, device_id: &str) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE paired_devices \
         SET last_seen_at = NOW() \
         WHERE device_id = $1 AND revoked_at IS NULL",
    )
    .bind(device_id)
    .execute(db)
    .await
    .context("touch paired_device")?;
    Ok(res.rows_affected() > 0)
}

/// Sweep expired and used-and-aged pair codes. Called from a
/// background task on a 5-minute cadence; keeps the codes table
/// from growing unbounded while leaving a 24-hour audit window.
pub async fn sweep_expired_codes(db: &PgPool) -> Result<u64> {
    let res = sqlx::query(
        "DELETE FROM pair_codes \
         WHERE expires_at < NOW() - INTERVAL '24 hours' \
            OR used_at < NOW() - INTERVAL '24 hours'",
    )
    .execute(db)
    .await
    .context("sweep pair_codes")?;
    Ok(res.rows_affected())
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_has_no_ambiguous_chars() {
        let s = std::str::from_utf8(ALPHABET).unwrap();
        for forbidden in ['0', 'O', '1', 'I', 'L'] {
            assert!(
                !s.contains(forbidden),
                "alphabet must not contain ambiguous char {forbidden}"
            );
        }
    }

    #[test]
    fn generate_code_yields_valid_normalized_codes() {
        for _ in 0..200 {
            let code = generate_code();
            assert!(is_valid_normalized(&code), "{code} should be valid");
        }
    }

    #[test]
    fn normalize_strips_whitespace_dashes_and_uppercases() {
        assert_eq!(normalize_code("k7m2-4xq9"), "K7M24XQ9");
        assert_eq!(normalize_code("  K7M2 4XQ9  "), "K7M24XQ9");
        assert_eq!(normalize_code("k 7 m 2 - 4 x q 9"), "K7M24XQ9");
    }

    #[test]
    fn is_valid_rejects_wrong_length_and_bad_chars() {
        assert!(!is_valid_normalized("K7M24XQ"), "too short");
        assert!(!is_valid_normalized("K7M24XQ99"), "too long");
        assert!(!is_valid_normalized("K7M24XQ0"), "contains banned 0");
        assert!(!is_valid_normalized("K7M24XQI"), "contains banned I");
        assert!(is_valid_normalized("K7M24XQ9"));
    }

    #[test]
    fn format_inserts_dash_in_the_middle() {
        assert_eq!(format_code("K7M24XQ9"), "K7M2-4XQ9");
    }

    #[test]
    fn refresh_token_roundtrip() {
        let token = generate_refresh_token();
        let hash = hash_refresh_token(&token).unwrap();
        assert!(verify_refresh_token(&token, &hash).unwrap());
        assert!(!verify_refresh_token("different", &hash).unwrap());
    }

    #[test]
    fn match_slot_accepts_current_token() {
        let cur = generate_refresh_token();
        let cur_hash = hash_refresh_token(&cur).unwrap();
        let now = Utc::now();
        assert_eq!(
            match_refresh_slot(&cur, &cur_hash, None, None, now).unwrap(),
            Some(true),
            "current token must match the current slot"
        );
    }

    #[test]
    fn match_slot_accepts_previous_token_within_grace() {
        // The desync recovery case: client kept the old token after a
        // failed persist. It must still authenticate via the prev slot.
        let prev = generate_refresh_token();
        let prev_hash = hash_refresh_token(&prev).unwrap();
        let cur = generate_refresh_token();
        let cur_hash = hash_refresh_token(&cur).unwrap();
        let now = Utc::now();
        let rotated_recently = now - Duration::hours(1); // well inside 48h
        assert_eq!(
            match_refresh_slot(
                &prev,
                &cur_hash,
                Some(&prev_hash),
                Some(rotated_recently),
                now
            )
            .unwrap(),
            Some(false),
            "previous token within grace must match the prev slot"
        );
    }

    #[test]
    fn match_slot_rejects_previous_token_outside_grace() {
        let prev = generate_refresh_token();
        let prev_hash = hash_refresh_token(&prev).unwrap();
        let cur = generate_refresh_token();
        let cur_hash = hash_refresh_token(&cur).unwrap();
        let now = Utc::now();
        let rotated_long_ago = now - (REFRESH_GRACE + Duration::hours(1));
        assert_eq!(
            match_refresh_slot(
                &prev,
                &cur_hash,
                Some(&prev_hash),
                Some(rotated_long_ago),
                now
            )
            .unwrap(),
            None,
            "previous token past the grace window must be rejected"
        );
    }

    #[test]
    fn match_slot_rejects_unknown_token() {
        let cur = generate_refresh_token();
        let cur_hash = hash_refresh_token(&cur).unwrap();
        let prev = generate_refresh_token();
        let prev_hash = hash_refresh_token(&prev).unwrap();
        let stranger = generate_refresh_token();
        let now = Utc::now();
        assert_eq!(
            match_refresh_slot(
                &stranger,
                &cur_hash,
                Some(&prev_hash),
                Some(now - Duration::hours(1)),
                now,
            )
            .unwrap(),
            None,
            "a token matching neither slot must be rejected"
        );
    }

    #[test]
    fn match_slot_current_takes_priority_over_prev() {
        // If the same token somehow hashed into both slots, current
        // wins — callers use the bool to decide shift-vs-hold.
        let tok = generate_refresh_token();
        let hash_a = hash_refresh_token(&tok).unwrap();
        let hash_b = hash_refresh_token(&tok).unwrap(); // different salt, same token
        let now = Utc::now();
        assert_eq!(
            match_refresh_slot(
                &tok,
                &hash_a,
                Some(&hash_b),
                Some(now - Duration::hours(1)),
                now
            )
            .unwrap(),
            Some(true),
            "current slot must win when both match"
        );
    }

    #[test]
    fn refresh_token_each_call_unique() {
        // 32 bytes of CSPRNG: P(collision in 10 samples) ≈ 0.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10 {
            assert!(seen.insert(generate_refresh_token()));
        }
    }

    #[test]
    fn refresh_token_hash_different_each_time() {
        // Same input must produce different PHC strings (random salt).
        let token = generate_refresh_token();
        let h1 = hash_refresh_token(&token).unwrap();
        let h2 = hash_refresh_token(&token).unwrap();
        assert_ne!(h1, h2);
        // Both still verify against the original input.
        assert!(verify_refresh_token(&token, &h1).unwrap());
        assert!(verify_refresh_token(&token, &h2).unwrap());
    }

    #[test]
    fn issuer_rejects_short_secret() {
        let res = AurisJwtIssuer::new(b"too-short");
        assert!(res.is_err());
    }

    #[test]
    fn jwt_roundtrip_with_unified_claims() {
        let issuer = AurisJwtIssuer::new(&[0xAB; 32]).unwrap();
        let now = Utc::now();
        let token = issuer.mint("auth0|abc123", "dev-uuid-1", now).unwrap();
        let claims = issuer.verify(&token).unwrap();
        assert_eq!(claims.sub, "auth0|abc123");
        assert_eq!(claims.device_id, "dev-uuid-1");
        assert_eq!(claims.iss, JWT_ISSUER);
        assert_eq!(claims.aud, JWT_AUDIENCE);
    }

    #[test]
    fn jwt_rejects_wrong_secret() {
        let a = AurisJwtIssuer::new(&[0xAB; 32]).unwrap();
        let b = AurisJwtIssuer::new(&[0xCD; 32]).unwrap();
        let tok = a.mint("auth0|abc", "dev", Utc::now()).unwrap();
        assert!(b.verify(&tok).is_err());
    }

    #[test]
    fn jwt_rejects_expired_token() {
        let issuer = AurisJwtIssuer::new(&[0xAB; 32]).unwrap();
        // Mint with iat well in the past; exp = iat + ACCESS_TTL,
        // also in the past. jsonwebtoken's default 60s leeway
        // tolerates clock skew but not multi-minute backdating.
        let past = Utc::now() - Duration::hours(2);
        let tok = issuer.mint("auth0|abc", "dev", past).unwrap();
        let err = issuer.verify(&tok).unwrap_err();
        assert!(format!("{err:#}").contains("Expired") || format!("{err:#}").contains("exp"));
    }

    #[test]
    fn peek_issuer_finds_iss_without_verifying() {
        let issuer = AurisJwtIssuer::new(&[0xAB; 32]).unwrap();
        let tok = issuer.mint("auth0|abc", "dev", Utc::now()).unwrap();
        // Tampered signature shouldn't matter for peeking.
        let tampered = format!("{tok}xxx");
        assert_eq!(peek_issuer(&tampered).as_deref(), Some(JWT_ISSUER));
    }

    #[test]
    fn peek_issuer_returns_none_for_garbage() {
        assert!(peek_issuer("not.a.jwt.at.all").is_none());
        assert!(peek_issuer("").is_none());
    }

    fn fake_pair(suffix: &str) -> RedeemedPair {
        RedeemedPair {
            device_id: format!("dev-{suffix}"),
            user_id: format!("user-{suffix}"),
            access_token: format!("access-{suffix}"),
            refresh_token: format!("refresh-{suffix}"),
            access_expires_at: Utc::now() + ACCESS_TTL,
        }
    }

    #[test]
    fn redeem_cache_round_trip_returns_same_pair() {
        // Put → get returns the same tokens. Test isolates by using
        // a code unique to this test so it doesn't collide with
        // parallel tests or leftover state from prior runs.
        let code = "TEST-RCACHE-RT".to_string();
        let pair = fake_pair("rt");
        redeem_cache_put(code.clone(), &pair);
        let got = redeem_cache_get(&code).expect("cache hit expected");
        assert_eq!(got.device_id, pair.device_id);
        assert_eq!(got.access_token, pair.access_token);
        assert_eq!(got.refresh_token, pair.refresh_token);
    }

    #[test]
    fn redeem_cache_evicts_expired_entries_on_access() {
        // Past-dated entries fall out of the cache on the next read,
        // even when no other entry was inserted in between. Sweep is
        // lazy + access-driven; this asserts that contract.
        let code = "TEST-RCACHE-EXP".to_string();
        let stale = Instant::now() - REDEEM_CACHE_TTL - StdDuration::from_secs(1);
        REDEEM_CACHE
            .lock()
            .unwrap()
            .insert(code.clone(), (stale, fake_pair("exp")));
        assert!(redeem_cache_get(&code).is_none());
    }

    #[test]
    fn redeem_cache_miss_for_unknown_code() {
        assert!(redeem_cache_get("TEST-NEVER-INSERTED").is_none());
    }
}
