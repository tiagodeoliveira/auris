// Metro config — monorepo-aware.
//
// We use `node-linker=hoisted` in the workspace-root .npmrc, which
// produces a flat node_modules layout (yarn / npm-classic style).
// That means Metro's default hierarchical lookup just works — we
// don't need to override `disableHierarchicalLookup` (Expo's
// doctor flags that override as dangerous, and rightly so once
// hoisting handles resolution).
//
// What we DO still customize: watch the entire monorepo so changes
// in workspace siblings (e.g. a future `packages/shared-ts`) trigger
// Fast Refresh inside the mobile app.
const { getDefaultConfig } = require("expo/metro-config");
const path = require("path");

const projectRoot = __dirname;
const monorepoRoot = path.resolve(projectRoot, "../..");

const config = getDefaultConfig(projectRoot);
config.watchFolders = [monorepoRoot];

module.exports = config;
