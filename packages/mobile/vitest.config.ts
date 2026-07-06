// Vitest harness for packages/mobile — scoped to PURE TypeScript
// modules under src/. React-Native / Expo modules (hooks, native
// requires) are deliberately out of scope: they need a native host
// and are exercised on-device. Decision logic that must be unit
// tested lives in plain modules (see src/audio/interruption.ts)
// precisely so this node-environment runner can execute it.
//
// Mirrors packages/pwa/vitest.config.ts, minus jsdom (no DOM here).
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
