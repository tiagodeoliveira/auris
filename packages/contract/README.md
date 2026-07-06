# @auris/contract

**Status: standalone scaffolding, not yet adopted.** The server,
PWA, Mac, and mobile clients still use their hand-synced contract
files today. This package defines the proto schema + generated
types/codecs in three languages so that any (or all) of them can
migrate at their own pace. Until that happens, this package is
inert — nothing imports from it.

The wire protocol — intended single source of truth for the
WebSocket contract between the server, PWA, Mac, and mobile clients.
Defined in **Protocol Buffers** (proto3), generated to **Rust**
(prost), **TypeScript** (ts-proto), and **Swift** (swift-protobuf).

Wire format (when adopted): **binary protobuf over WS binary frames.**
Inspect bytes with `protoc --decode` or any `.proto`-aware viewer;
the binary cost buys schema evolution rules that hand-rolled JSON
couldn't give us.

## Layout

```
packages/contract/
├── proto/auris/v1/      ← source of truth (3 .proto files)
│   ├── common.proto                   shared types (Item, Status, Device, …)
│   ├── intents.proto                  Intent oneof + per-variant messages
│   └── events.proto                   Event oneof + per-variant messages
├── buf.yaml                           buf module config + lint rules
├── buf.gen.yaml                       buf codegen config (TS + Swift only)
├── rust/                              prost-build crate (build.rs → OUT_DIR)
│   ├── Cargo.toml
│   ├── build.rs
│   └── src/lib.rs
├── ts/                                pnpm workspace package
│   ├── package.json                   "@auris/contract"
│   ├── tsconfig.json
│   └── src/
│       ├── index.ts                   curated public re-exports
│       └── gen/                       ts-proto output (committed)
└── swift/                             local SwiftPM package
    ├── Package.swift                  "AurisContract"
    └── Sources/AurisContract/
        ├── AurisContract.swift  manual public surface
        └── Generated/                       swift-protobuf output (committed)
```

**Rust output is NOT committed** — `prost-build` generates into cargo's
`OUT_DIR` on every build. **TS + Swift outputs ARE committed** so
`pnpm install` / `swift build` work out of the box without protoc on
every dev machine. Diff is reviewable in PRs that touch the proto.

## Adding a field

Schema evolution is the whole point of protobuf — additive changes
don't require a coordinated client roll-out.

1. **Edit the right .proto file** under `proto/auris/v1/`.
   - New types? `common.proto`.
   - New intent variant? `intents.proto`. Pick the next free oneof
     number (current high-water: 11). Add a new top-level message
     for the payload, then add it to the `Intent.kind` oneof.
   - New event variant? `events.proto`. Same pattern.
   - New field on an existing message? Pick the next free field
     number. Mark `optional` if old payloads might omit it.

2. **Regenerate TS + Swift outputs:**

   ```sh
   just contract-gen
   ```

   Rust regenerates automatically on the next `cargo build`.

3. **Verify everything still compiles:**

   ```sh
   just contract-check
   ```

4. **Commit** the .proto edit + the generated TS/Swift diff together.
   Reviewer reads the proto change to evaluate the wire shape;
   reads the generated diff to spot accidental breaking changes.

## Removing a field

Don't just delete it — protobuf needs to know the field number can
never be reused. Use `reserved`:

```proto
message Item {
  reserved 5;            // was: meta_json
  reserved "meta_json";
  // …
}
```

Same for oneof variants:

```proto
message Intent {
  oneof kind {
    reserved 7;          // was: extract_metadata
    // …
  }
}
```

## Breaking changes

These force a `PROTOCOL_VERSION` bump and a coordinated client
roll-out:

- Changing a field's type (`int32` → `string`, etc.).
- Changing a field's number.
- Changing a singular field to repeated, or vice versa for non-scalar
  types.
- Removing a `oneof` variant a client depends on.

When you bump the version:

1. Edit `PROTOCOL_VERSION` in `packages/contract/rust/src/lib.rs`
   (Rust) and `packages/contract/ts/src/index.ts` (TS).
2. Bump `Snapshot.protocol_version` value the server emits.
3. Update each client's compile-time check.
4. Land all four clients in one merge train, then deploy server
   first.

For really big shifts, copy the whole tree to `proto/auris/v2/`
and let the generators produce parallel `v2.*` modules. Old clients
keep using `v1`; new clients import `v2`. ts-proto, prost, and
swift-protobuf all support multi-version coexistence.

## Just recipes

```sh
just contract-gen         # regenerate TS + Swift from .proto
just contract-lint        # buf lint (style + naming)
just contract-format      # buf format --write (in place)
just contract-check       # full canary: lint + format diff + Rust + TS + Swift compile
```

## Tooling prerequisites

For local regeneration:

```sh
brew install bufbuild/buf/buf protobuf
```

For `cargo build` of the Rust crate:

```sh
brew install protobuf      # provides protoc, used by prost-build
```

CI (Ubuntu) installs `buf` via its release tarball + `protoc` via
`apt-get install -y protobuf-compiler`.

## Why this shape

- **Single source of truth in `.proto`** — wire types live in one
  language-agnostic schema. No drift risk between four hand-synced
  files.
- **Binary on the wire** — smaller bytes, faster parse, future-proof
  for high-throughput audio sidecar messages. Inspectability via
  `protoc --decode` is fine; we don't need human-readable wire bytes
  in production.
- **prost / ts-proto / swift-protobuf** — all three generate
  idiomatic per-language types. prost integrates with cargo's
  build.rs (no committed output); ts-proto + swift-protobuf produce
  clean files we commit so consumers don't need protoc.
- **Versioned package paths (`auris.v1.*`)** — lets a
  future `v2` land in parallel without breaking `v1` imports.
- **`buf` as the orchestrator** — single tool for lint + format +
  cross-language gen, replaces three separate `protoc` invocations.

## See also

- [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md) — human-readable
  reference for the wire surface (intent / event tables, REST
  endpoints, mode catalog).
- [`docs/adr/README.md`](../../docs/adr/README.md) "Pending —
  Wire codegen" — design rationale for moving from hand-synced to
  protobuf.
- [`packages/server/src/contract.rs`](../server/src/contract.rs),
  [`packages/pwa/src/contract.ts`](../pwa/src/contract.ts),
  [`packages/mac/Sources/Auris/Net/Protocol.swift`](../mac/Sources/Auris/Net/Protocol.swift),
  [`packages/mobile/src/wire/contract.ts`](../mobile/src/wire/contract.ts)
  — the four hand-synced contract files in active use today. When
  a client migrates to this package, its hand-synced file goes
  away. Until then they're the source of truth and this package
  is forward-looking infrastructure.
