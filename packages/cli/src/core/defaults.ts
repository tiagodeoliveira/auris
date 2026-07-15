// Default identity + endpoint values shipped with this build.
//
// Committed values are dev placeholders so a from-source `build` works against
// a local auris with no setup. The release workflow (.github/workflows/
// cli-release.yml) stamps real values here from repo Variables at tag time;
// runtime env (AURIS_BASE_URL / AURIS_AUTH0_*) overrides either.
export const DEFAULTS = {
  aurisBaseUrl: "http://localhost:7331",
  auth0Domain: "",
  auth0Audience: "",
  auth0ClientId: "",
} as const;
