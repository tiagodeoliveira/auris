-- "Quick asks" — user-curated chat prompts available across all the
-- user's meetings. Each quick_ask is a (label, text) pair: the label
-- is the short mnemonic shown on glasses / on chat-chip rows; the
-- text is the full multiline prompt that gets sent as a Chat intent
-- when the user picks it.
--
-- Scoped per-user (not per-meeting). User can have at most 50
-- entries (enforced at the application layer, not the DB).
--
-- Order is user-controlled via `position`: smaller positions come
-- first. Integer with gaps so reorders don't have to rewrite the
-- whole list.

CREATE TABLE quick_asks (
    id              UUID PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    label           TEXT NOT NULL,
    text            TEXT NOT NULL,
    position        INTEGER NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Listing the user's library, ordered, is the dominant read pattern.
CREATE INDEX idx_quick_asks_user ON quick_asks(user_id, position ASC);
