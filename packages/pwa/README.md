# @auris/pwa

TypeScript Progressive Web App that runs inside the EvenHub Flutter
WebView. Mirrors the server's meeting state, drives the G2 glasses
display, and provides the user's control surface (compose, dictate,
review chips, run a meeting).

System overview: [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md).
Wire protocol: [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md).
Design system + interaction patterns: [`docs/UX.md`](../../docs/UX.md).

## Run

```bash
pnpm -F @auris/pwa dev          # vite dev server only
pnpm -F @auris/pwa dev:sim      # vite + EvenHub simulator
pnpm -F @auris/pwa dev:qr       # QR code for real glasses sideload
```

## Test

```bash
pnpm -F @auris/pwa test         # unit tests + typecheck
pnpm -F @auris/pwa test:integration  # simulator HTTP tests
```

## Pack for distribution

```bash
pnpm -F @auris/pwa build
pnpm -F @auris/pwa pack         # → auris.ehpk
```

## Configuration

The server URL is a build-time constant (`VITE_SERVER_URL`,
hard-coded into the bundle by Vite — see `src/server-url.ts`).
Auth0 SPA configuration is also build-time (`VITE_AUTH0_DOMAIN`,
`VITE_AUTH0_CLIENT_ID`, `VITE_AUTH0_AUDIENCE`).

For local dev, copy `.env.example` to `.env.local` and fill those
in. Auth0 access + refresh tokens are persisted at runtime via the
EvenHub bridge's local storage with a browser `localStorage`
fallback (per [ADR-0003](../../docs/adr/0003-persistence-via-bridge.md)).

The login screen mounts before any meeting state. After sign-in,
the WS auto-connects with the JWT on the query string.

## Manual hardware checklist (G2 sideload)

1. Sideload to G2: `pnpm -F @auris/pwa dev:qr`, scan QR in
   the Even Realities companion app.
2. Sign in with Auth0 on first launch.
3. Verify the idle "NEW MEETING" surface renders.
4. Tap mic on phone → dictation starts (server-mediated `/stt`
   transcript fills the textarea live).
5. Tap mic again → dictation stops; transcript stays editable.
6. Tap `EXTRACT TAGS` (or Cmd/Ctrl+Enter in the textarea) → metadata
   chips appear in the chip strip.
7. Edit a chip value, press Enter → confirmed save (no flicker, no
   typing erasure).
8. Tap `START MEETING` → idle compose hides, active surface appears
   (header + memory badge if mnemo populated context + mode tabs +
   items mirror).
9. Speak; verify transcript items appear within ~1–2 s of sentence
   boundaries; live in-flight row shows the dim italic interim text.
10. Wait ~15–20 s; verify highlights / actions / open_questions
    populate from the agent loop.
11. Switch to `SUMMARY` tab; verify the rolling summary appears as a
    single item.
12. Switch to `CHAT` tab; type a question, send; verify the agent's
    reply lands as the assistant bubble in the same Q+A pair.
13. Switch back to `TRANSCRIPT`; tap an item's chevron; verify a
    detail expansion arrives.
14. Tap `STOP` once (armed: "Tap again to confirm"); tap again
    (commits). Verify everything clears.

### Hardware-specific follow-ups (open)

- **[ADR-0001](../../docs/adr/0001-gesture-map.md) follow-up**: log
  raw `bridge.onEvenHubEvent` payloads on temple tap, ring tap, and
  swipe; identify which field carries the
  [`EventSourceType`](https://hub.evenrealities.com/docs/api/types#eventsourcetype)
  source distinction (`1`=G2 right, `2`=R1 ring, `3`=G2 left). Update
  the gesture-router with the empirical field name.
- **Font-metrics recalibration** for `LINES_PER_SCREEN` and
  `CHARS_PER_LINE` in `src/glasses/layout-active-list.ts`, using the
  [`everything-evenhub:font-measurement`](https://hub.evenrealities.com/docs/)
  skill against real LVGL font metrics.

## Architecture in one screen

UI → Store → WS → Server → … → Server → WS → Store → UI.

- The store (`src/store.ts`) is the single source of truth.
- Each UI component (`src/ui/*.ts`) subscribes to a slice and
  re-renders on change.
- Components self-hide based on `meetingState` — the parent
  `ui/index.ts` only knows mount order, not visibility.
- WS events are reduced to store changes by `src/ws-handlers.ts`.
- Outbound intents flow through the `ReconnectingSocket` in `src/ws.ts`.

See [`docs/UX.md`](../../docs/UX.md) for the detailed component map and
interaction patterns.
