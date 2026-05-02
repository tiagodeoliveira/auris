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
