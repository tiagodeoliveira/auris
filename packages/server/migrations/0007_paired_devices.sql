-- 0007_paired_devices.sql
--
-- Adds the device-pairing tables backing the "Pair glasses" flow.
-- The PWA on the EvenHub glasses cannot use Auth0's redirect-callback
-- flow (the companion app loads the .ehpk from a dynamic 127.0.0.1
-- port, which Auth0 won't wildcard). Instead, mobile mints a
-- short-lived pairing code; PWA redeems it for a server-issued JWT.
--
--   paired_devices    — durable record of each paired device. Holds
--                       the (hashed) current refresh token so we can
--                       rotate on every use and revoke individual
--                       devices without touching others.
--   pair_codes        — single-use codes minted by an authed client,
--                       consumed (public) by the device being paired.
--
-- Both tables cascade off `users(id)`, so deleting a user wipes their
-- pair state. The PWA's access tokens are stateless (verified by
-- HS256 signature alone) so no per-token row is required.

CREATE TABLE paired_devices (
    -- UUID v4 minted server-side at redeem time. Embedded in every
    -- access token's `device_id` claim so revocation is per-device,
    -- not per-user.
    device_id            TEXT PRIMARY KEY,
    user_id              TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Human-readable label shown in mobile's "Paired devices" list.
    -- Caller-provided at redeem time; defaults to "G2 glasses" if
    -- the PWA didn't send one.
    device_label         TEXT NOT NULL,
    paired_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Argon2id hash of the current refresh token. Rotated on every
    -- /pair/refresh: the old hash is replaced atomically so a replay
    -- of a previously-valid refresh token fails the lookup.
    refresh_token_hash   TEXT NOT NULL,
    -- Set when the user (or an admin) revokes this device. Once
    -- non-NULL, /pair/refresh + access-token verification both
    -- reject. The row stays for audit / "show recently unpaired
    -- devices" UX rather than being deleted outright.
    revoked_at           TIMESTAMPTZ
);

CREATE INDEX idx_paired_devices_user ON paired_devices(user_id);
-- Used by /pair/refresh: hash incoming token, look up the row.
-- Unique so a hash collision (vanishingly unlikely with Argon2id)
-- can't silently authenticate two devices as one. Partial — once
-- revoked, the hash slot is free to be reused by another device.
CREATE UNIQUE INDEX idx_paired_devices_refresh_hash
    ON paired_devices(refresh_token_hash)
    WHERE revoked_at IS NULL;

CREATE TABLE pair_codes (
    -- Normalized form: uppercase, no dashes/whitespace. 8 chars from
    -- the 32-char ambiguous-free alphabet (see pairing::ALPHABET).
    code            TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 5 minutes after `created_at` at mint time. Past this, the code
    -- can't be redeemed even if still in the table (sweeper deletes
    -- expired rows on its own schedule).
    expires_at      TIMESTAMPTZ NOT NULL,
    -- NULL until first successful redeem. Codes are single-use: the
    -- redeem transaction is conditional on `used_at IS NULL`.
    used_at         TIMESTAMPTZ,
    -- ON DELETE SET NULL so unpairing a device doesn't break audit
    -- visibility of past redeem events.
    used_by_device  TEXT REFERENCES paired_devices(device_id) ON DELETE SET NULL
);

CREATE INDEX idx_pair_codes_user ON pair_codes(user_id);
CREATE INDEX idx_pair_codes_expires ON pair_codes(expires_at);
