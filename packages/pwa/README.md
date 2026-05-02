# @meeting-companion/pwa

Phase 0 PWA. See [`docs/specs/pwa.md`](../../docs/specs/pwa.md) for the spec.

## Run

```bash
pnpm -F @meeting-companion/pwa dev          # vite dev server only
pnpm -F @meeting-companion/pwa dev:sim      # vite + simulator
pnpm -F @meeting-companion/pwa dev:qr       # QR code for real glasses sideload
```

## Test

```bash
pnpm -F @meeting-companion/pwa test         # unit tests
pnpm -F @meeting-companion/pwa test:integration  # simulator HTTP tests
```

## Pack for distribution

```bash
pnpm -F @meeting-companion/pwa build
pnpm -F @meeting-companion/pwa pack         # → meeting-companion.ehpk
```

See [`docs/specs/pwa.md`](../../docs/specs/pwa.md) §11 for the build pipeline details.

## Manual hardware checklist (Phase 1)

1. Sideload to G2: `pnpm -F @meeting-companion/pwa dev:qr`, scan QR in the Even Realities companion app.
2. Verify Layout A renders.
3. Tap "Describe meeting" on phone → glasses Layout E appears.
4. Speak; verify transcript appears on glasses + phone.
5. Wait for VAD silence (≥ 2.5 s) → meeting starts; glasses Layout B appears.
6. Verify mock items arrive every ~3 s.
7. Ring scroll → highlight moves.
8. Ring tap → Layout C; verify detail loads.
9. Phone Stop → first tap shows confirm; second commits.
10. **ADR-0001 follow-up**: log raw `bridge.onEvenHubEvent` payloads on temple tap, ring tap, swipe — identify which field carries the source distinction (`event.sysEvent.eventSource` per the SDK reference's `EventSourceType` enum: `1`=G2 right, `2`=R1, `3`=G2 left). Document findings in `docs/specs/pwa.md` §10.4.
11. Confirm `bridge.setLocalStorage` actually persists after a full Even Realities App restart (not just an in-app reload — see ADR-0003 follow-up).
