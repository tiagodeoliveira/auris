/// Build-time configuration baked into the JS bundle.
///
/// Expo replaces every `process.env.EXPO_PUBLIC_*` reference with a
/// string literal during bundling — they're NOT read at runtime, so
/// changing them requires a fresh `eas build` (or `eas update`,
/// which embeds them into the OTA bundle at publish time).
///
/// Defaults below cover the local-dev case (no env file). For
/// CI / cloud builds, the workflows pass these through from repo
/// variables (see .github/workflows/README.md).

const fallbackServerUrl = "ws://localhost:7331";

export const serverUrl: string =
  (process.env.EXPO_PUBLIC_SERVER_URL ?? "").trim() || fallbackServerUrl;

export const auth0Config = {
  domain: (process.env.EXPO_PUBLIC_AUTH0_DOMAIN ?? "").trim(),
  clientId: (process.env.EXPO_PUBLIC_AUTH0_MOBILE_CLIENT_ID ?? "").trim(),
  audience: (process.env.EXPO_PUBLIC_AUTH0_API_AUDIENCE ?? "").trim(),
} as const;

/// True when every Auth0 field is populated. The auth bootstrapping
/// in Phase 1 should bail out gracefully (show "Auth0 not configured"
/// rather than attempt a broken sign-in) when this is false.
export const auth0Configured: boolean = Boolean(
  auth0Config.domain && auth0Config.clientId && auth0Config.audience,
);
