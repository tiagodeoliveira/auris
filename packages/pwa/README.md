# @meeting-companion/pwa

TypeScript Progressive Web App that runs inside the EvenHub Flutter
WebView. Mirrors the server's meeting state, drives the G2 glasses
display, and provides the user's control surface (compose, dictate,
review chips, run a meeting).

System overview: [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md).
Wire protocol: [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md).
Design system + interaction patterns: [`docs/UX.md`](../../docs/UX.md).

## Run

```bash
pnpm -F @meeting-companion/pwa dev          # vite dev server only
pnpm -F @meeting-companion/pwa dev:sim      # vite + EvenHub simulator
pnpm -F @meeting-companion/pwa dev:qr       # QR code for real glasses sideload
```

## Test

```bash
pnpm -F @meeting-companion/pwa test         # unit tests + typecheck
pnpm -F @meeting-companion/pwa test:integration  # simulator HTTP tests
```

## Pack for distribution

```bash
pnpm -F @meeting-companion/pwa build
pnpm -F @meeting-companion/pwa pack         # → meeting-companion.ehpk
```

## Configuration

The PWA reads its server URL, server token, and Soniox API key from
the EvenHub bridge's local storage with a browser `localStorage`
fallback (per [ADR-0003](../../docs/adr/0003-persistence-via-bridge.md)).
On first run, open Settings (gear icon, top-right) and fill them in.

To skip retyping in development, copy `.env.example` to `.env.local`
and set the `VITE_DEFAULT_*` variables — the PWA seeds first-run
defaults from these.

## Manual hardware checklist (Phase 1)

1. Sideload to G2: `pnpm -F @meeting-companion/pwa dev:qr`, scan QR in
   the Even Realities companion app.
2. Verify the idle "NEW MEETING" surface renders.
3. Tap mic on phone → dictation starts (Soniox transcript fills the
   textarea live).
4. Tap mic again → dictation stops; transcript stays editable.
5. Tap `EXTRACT TAGS` (or Cmd/Ctrl+Enter in the textarea) → metadata
   chips appear in the chip strip.
6. Edit a chip value, press Enter → confirmed save (no flicker, no
   typing erasure).
7. Tap `START MEETING` → idle compose hides, active surface appears
   (header + memory badge if mnemo populated context + mode tabs +
   items mirror).
8. Speak; verify transcript items appear within ~1–2 s of sentence
   boundaries; live in-flight row shows the dim italic interim text.
9. Wait ~15–20 s; verify highlights / actions / open_questions populate
   from the LLM summarizers.
10. Switch modes via tabs; verify items reflect the selected mode.
11. Tap `STOP` once (armed: "Tap again to confirm"); tap again
    (commits). Verify everything clears.

### Hardware-specific follow-ups (open)

- **[ADR-0001](../../docs/adr/0001-gesture-map.md) follow-up**: log
  raw `bridge.onEvenHubEvent` payloads on temple tap, ring tap, and
  swipe; identify which field carries the
  [`EventSourceType`](https://hub.evenrealities.com/docs/api/types#eventsourcetype)
  source distinction (`1`=G2 right, `2`=R1 ring, `3`=G2 left). Update
  the gesture-router with the empirical field name.
- **[ADR-0003](../../docs/adr/0003-persistence-via-bridge.md)
  follow-up**: confirm `bridge.setLocalStorage` actually persists
  across a full Even Realities App restart (not just an in-app reload).
  If not, the `localStorage` fallback becomes the primary surface
  on hardware too.
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
