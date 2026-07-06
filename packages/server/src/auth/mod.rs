//! Authentication and device-pairing.
//!
//! `validator` verifies Auth0-issued bearer tokens via JWKS.
//! `pairing` mints + verifies Auris-issued HS256 tokens for the
//! device-pairing flow.

pub mod pairing;
pub mod rate_limit;
pub mod validator;

// Re-export the most-used types so external callers can keep
// `use crate::auth::AuthValidator;` rather than threading through
// the submodule path.
pub use pairing::AurisJwtIssuer;
pub use validator::resolve_user_id;
pub use validator::AuthValidator;

/// Auth mode is decided at boot: either we validate JWTs against
/// Auth0 + our own paired-device issuer, or we run with a synthetic
/// dev user (env-flag bypass for `websocat`/`curl` smoke testing
/// without a browser flow).
pub enum AuthMode {
    /// `AURIS_AUTH_DISABLED=1` set. Every request is
    /// attributed to a fixed dev user (`auth0_sub = "dev|local"`).
    /// The pair-flow endpoints return 503 in this mode — there's no
    /// JWT issuer to mint with.
    Disabled,
    /// Real validation. Tokens are routed by their `iss` claim:
    /// Auth0-issued tokens go to the JWKS validator, paired-device
    /// tokens go to the local HS256 issuer. Both are required at
    /// boot because the pair flow needs both an authed minter
    /// (Auth0 JWTs from mobile) and a verifier (Auris JWTs from
    /// the PWA).
    Live {
        auth0: AuthValidator,
        auris: AurisJwtIssuer,
    },
}

/// Synthetic Auth0 sub used when the bypass flag is on. Has the same
/// shape as a real Auth0 sub (`<connection>|<id>`) so DB rows look
/// uniform.
pub const DEV_AUTH0_SUB: &str = "dev|local";
