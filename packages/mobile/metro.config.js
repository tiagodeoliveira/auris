// Metro config — monorepo-aware.
//
// We use `node-linker=hoisted` in the workspace-root .npmrc, which
// produces a flat node_modules layout at the monorepo ROOT. Metro's
// default hierarchical-resolution walk handles most lookups, but
// Expo's own entry-point resolver (`expo-router/entry`, etc.) builds
// paths as `./node_modules/<name>` relative to `projectRoot` — those
// don't walk up, so we have to enumerate both locations explicitly.
//
// Two customisations:
//
//   1. `watchFolders` — include the monorepo root so Fast Refresh
//      picks up edits in sibling packages (`@auris/contract`, etc.).
//
//   2. `resolver.nodeModulesPaths` — list the mobile package's local
//      `node_modules/` AND the monorepo-root one, so deps that pnpm
//      hoisted resolve correctly when Metro walks Expo-emitted
//      relative paths. Without this, Android dev-client bundling
//      fails with "Unable to resolve module ./node_modules/expo-
//      router/entry from .../packages/mobile/." (iOS may happen to
//      work via a different cache, but the bug is the same.)
const { getDefaultConfig } = require("expo/metro-config");
const path = require("path");

const projectRoot = __dirname;
const monorepoRoot = path.resolve(projectRoot, "../..");

const config = getDefaultConfig(projectRoot);
config.watchFolders = [monorepoRoot];
config.resolver.nodeModulesPaths = [
  path.resolve(projectRoot, "node_modules"),
  path.resolve(monorepoRoot, "node_modules"),
];

module.exports = config;
