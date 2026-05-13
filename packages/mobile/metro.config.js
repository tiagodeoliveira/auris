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

// Android dev-client bundle URL rewrite.
//
// Android's dev-client APK requests bundle URLs of the form
//   /node_modules/expo-router/entry.bundle?platform=android&…
// Metro's HTTP bundler converts that URL path into a project-
// relative entry file (no hierarchical walk). With pnpm
// `node-linker=hoisted`, deps live at the monorepo root
// `node_modules/`, NOT under `packages/mobile/node_modules/`, so
// the request 404s with "Unable to resolve module
// ./node_modules/expo-router/entry".
//
// Prepend `/../..` to redirect the relative path up two levels
// to the workspace root, where the hoisted deps actually live.
// Metro happily follows the `..` segments.
//
// iOS uses a different URL shape (bare-module form) and works
// without this rewrite, but applying it universally is safe —
// the `/../..` prefix only kicks in for URLs that already start
// with `/node_modules/`.
config.server = config.server || {};
config.server.rewriteRequestUrl = (url) => {
  if (url.startsWith("/node_modules/")) {
    return "/../.." + url;
  }
  return url;
};

module.exports = config;
