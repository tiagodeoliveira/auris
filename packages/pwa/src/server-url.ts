// Build-time server URL. Mirrors the Mac app's hardcoded
// `AppSettings.serverURLDefault` — the user shouldn't be configuring
// this in the UI. Different deployments ship different binaries;
// dev/prod each bake their own value.
//
// Reads `VITE_SERVER_URL` at build time (or the legacy
// `VITE_DEFAULT_SERVER_URL` for compat). Falls back to localhost so
// `vite dev` works without an .env file.

const env = import.meta.env as Record<string, string | undefined>;

export const SERVER_URL: string =
  env.VITE_SERVER_URL ?? env.VITE_DEFAULT_SERVER_URL ?? "ws://localhost:7331";
