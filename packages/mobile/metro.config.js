// Metro config — monorepo-aware. The repo uses pnpm workspaces with
// no hoisting; Metro by default only walks node_modules from the
// project root upward, which misses both `packages/*/node_modules`
// and the workspace root's `node_modules`. We extend the watch
// folders + module-resolution paths explicitly.
const { getDefaultConfig } = require("expo/metro-config");
const path = require("path");

const projectRoot = __dirname;
const monorepoRoot = path.resolve(projectRoot, "../..");

const config = getDefaultConfig(projectRoot);

// Watch the entire monorepo so changes in shared packages (e.g.
// `packages/shared-ts`, when extracted per MOBILE-PLAN §3) trigger
// a Fast Refresh inside the mobile app.
config.watchFolders = [monorepoRoot];

// pnpm doesn't hoist by default. Metro needs both the package's own
// node_modules and the workspace root's to resolve every dep.
config.resolver.nodeModulesPaths = [
  path.resolve(projectRoot, "node_modules"),
  path.resolve(monorepoRoot, "node_modules"),
];
config.resolver.disableHierarchicalLookup = true;

module.exports = config;
