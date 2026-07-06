// Design tokens for the mobile app.
//
// The "Listening Room" design system: warm-editorial palette derived
// from the brand SVGs (`assets/branding/icon-{coral,light,dark}.svg`),
// concentric-arc geometry, typography-forward layouts. Two complete
// token sets ship — `light` and `dark` — keyed by the scheme. Consume
// via the `useTheme()` hook so components react to OS scheme changes
// without prop drilling.
//
// Backwards-compat aliases: a few legacy keys (`bg.base`, `bg.page`,
// `text.onInverse`) still resolve to the new equivalent so existing
// screens / primitives compile while subsequent design-overhaul agents
// migrate call sites at their own pace. Mark new code with the modern
// names (`bg.elevated`, `bg.canvas`, `text.onCoral`).

type Scheme = "light" | "dark";

// Font families resolve to the exact strings exported by the
// @expo-google-fonts/* packages. The fonts are loaded in
// `app/_layout.tsx` via `useFonts` before the tree renders; reading
// any of these before that resolves would fall back to the platform
// system font, which is the correct degraded behavior.
const FONT = {
  display: "BebasNeue_400Regular",
  sans: "SpaceGrotesk_400Regular",
  sansMedium: "SpaceGrotesk_500Medium",
  sansSemi: "SpaceGrotesk_600SemiBold",
  sansBold: "SpaceGrotesk_700Bold",
  mono: "JetBrainsMono_400Regular",
  monoMedium: "JetBrainsMono_500Medium",
  monoSemi: "JetBrainsMono_600SemiBold",
} as const;

// Spacing — 4pt grid. Don't reach for raw 6 or 10.
const SPACING = {
  xs: 4,
  sm: 8,
  md: 12,
  lg: 16,
  xl: 24,
  xxl: 32,
  xxxl: 48,
} as const;

// Radius — `xl` is new; the wide-shallow card silhouette feels more
// generous than the previous 12px ceiling.
const RADIUS = {
  sm: 6,
  md: 8,
  lg: 12,
  xl: 16,
  pill: 999,
} as const;

// Typography ramp — shared across schemes. `caption` and `labelMono`
// are the signature label treatments; always pair with
// `textTransform: "uppercase"` at the call site (we don't bake it in
// because some captions are mixed-case — e.g. file names).
const TYPE = {
  display: {
    fontFamily: FONT.display,
    fontSize: 48,
    letterSpacing: 2,
    lineHeight: 52,
  },
  headline: {
    fontFamily: FONT.sansBold,
    fontSize: 28,
    lineHeight: 34,
  },
  title: {
    fontFamily: FONT.sansSemi,
    fontSize: 20,
    lineHeight: 26,
  },
  subtitle: {
    fontFamily: FONT.sansSemi,
    fontSize: 17,
    lineHeight: 22,
  },
  body: {
    fontFamily: FONT.sans,
    fontSize: 15,
    lineHeight: 21,
  },
  bodyMedium: {
    fontFamily: FONT.sansMedium,
    fontSize: 15,
    lineHeight: 21,
  },
  bodySmall: {
    fontFamily: FONT.sans,
    fontSize: 13,
    lineHeight: 18,
  },
  caption: {
    fontFamily: FONT.monoMedium,
    fontSize: 11,
    letterSpacing: 2,
    lineHeight: 14,
  },
  labelMono: {
    fontFamily: FONT.monoMedium,
    fontSize: 10,
    letterSpacing: 0.6,
    lineHeight: 13,
  },
  mono: {
    fontFamily: FONT.mono,
    fontSize: 12,
    lineHeight: 16,
  },
  monoMedium: {
    fontFamily: FONT.monoMedium,
    fontSize: 12,
    lineHeight: 16,
  },
} as const;

// Light color palette — warm-editorial. The cream `#f4f1ec` is the
// brand cream from `icon-light.svg`'s background and threads through
// `bg.subtle` + `border.soft` to keep rest-stops on-brand.
const LIGHT_COLOR = {
  bg: {
    canvas: "#fbf9f4", // warm off-white page background
    elevated: "#ffffff", // card surface on cream
    subtle: "#f4f1ec", // brand cream — tinted bands, rest stops
    tint: "#efe9e0", // warmer pillow for chips / KV rows
    // Legacy aliases — DO NOT use in new code:
    base: "#ffffff", // -> bg.elevated
    page: "#fbf9f4", // -> bg.canvas
  },
  text: {
    primary: "#1e293b",
    secondary: "#647386",
    muted: "#96a3b4",
    placeholder: "#96a3b4",
    onCoral: "#ffffff",
    onSlate: "#f4f1ec",
    // Legacy alias:
    onInverse: "#ffffff", // -> text.onCoral
  },
  border: {
    strong: "#e5dcc8", // warm border (not cool gray)
    soft: "#f4f1ec",
    hairline: "rgba(30, 41, 59, 0.08)",
  },
  brand: {
    coral: "#d97757",
    coralDim: "#fbe7df",
    coralDeep: "#c45f3f",
    coralGlow: "rgba(217, 119, 87, 0.25)",
    slate: "#1e293b",
    cream: "#f4f1ec",
  },
  action: {
    // Action is now coral — these aliases preserve the old name
    // (`action.primary`) but point at the new brand color.
    primary: "#d97757",
    primaryDim: "#fbe7df",
    primaryDeep: "#c45f3f",
  },
  danger: {
    base: "#e5484d",
    tint: "#fee5e7",
  },
  status: {
    ok: "#16a34a",
    pending: "#ca8a04",
    error: "#dc2626",
  },
  amber: {
    base: "#f2b705",
    text: "#765a00",
    tint: "#fef3c7",
  },
} as const;

// Dark color palette — slate canvas mirroring the Mac overlay dark.
// Shadows are nearly invisible on dark surfaces; primitives that
// previously relied on shadow.card should fall back to a
// `borderColor: border.hairline` 1px ring instead.
const DARK_COLOR = {
  bg: {
    canvas: "#1b2230",
    elevated: "#232b3a",
    subtle: "#2a3140",
    tint: "#2d3445",
    base: "#232b3a", // -> bg.elevated
    page: "#1b2230", // -> bg.canvas
  },
  text: {
    primary: "#e6ebf2",
    secondary: "#9aa7b8",
    muted: "#6b7889",
    placeholder: "#6b7889",
    onCoral: "#ffffff",
    onSlate: "#f4f1ec",
    onInverse: "#ffffff",
  },
  border: {
    strong: "#39414f",
    soft: "#2d3445",
    hairline: "rgba(255, 255, 255, 0.08)",
  },
  brand: {
    // Coral is invariant across modes — the device theme does not
    // interfere with coral branding. Only alpha-channel tints shift
    // for dark contrast.
    coral: "#d97757",
    coralDim: "rgba(217, 119, 87, 0.22)",
    coralDeep: "#c45f3f",
    coralGlow: "rgba(217, 119, 87, 0.35)",
    slate: "#0f1722",
    // Cream stays referenceable but is NOT a surface color in dark
    // mode — use bg.subtle / bg.tint for that.
    cream: "#f4f1ec",
  },
  action: {
    // Mirrors brand.coral; invariant across modes by design.
    primary: "#d97757",
    primaryDim: "rgba(217, 119, 87, 0.22)",
    primaryDeep: "#c45f3f",
  },
  danger: {
    // Same hex as light; only the tint background shifts to a
    // translucent variant that sits on dark slate cleanly.
    base: "#e5484d",
    tint: "rgba(229, 72, 77, 0.22)",
  },
  status: {
    // Status base hexes invariant — only tints shift for dark
    // surfaces (those don't live on `status`; see `amber.tint`).
    ok: "#16a34a",
    pending: "#ca8a04",
    error: "#dc2626",
  },
  amber: {
    base: "#f2b705",
    // Slightly brighter so amber chips/text read on dark slate.
    text: "#ffd970",
    tint: "rgba(242, 183, 5, 0.22)",
  },
} as const;

// Shadows — used on light surfaces. On dark, primitives swap to a
// 1px `border.hairline` ring (shadows on near-black don't read).
const LIGHT_SHADOW = {
  card: {
    shadowColor: "#0f172a",
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0.05,
    shadowRadius: 3,
    elevation: 2,
  },
  floating: {
    shadowColor: "#0f172a",
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.12,
    shadowRadius: 12,
    elevation: 8,
  },
} as const;

const DARK_SHADOW = {
  card: {
    shadowColor: "#000000",
    shadowOffset: { width: 0, height: 1 },
    shadowOpacity: 0,
    shadowRadius: 0,
    elevation: 0,
  },
  floating: {
    shadowColor: "#000000",
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.4,
    shadowRadius: 16,
    elevation: 8,
  },
} as const;

// The shared shape — derived from the light palette but with all
// literal hex types widened to `string` so the dark set is structurally
// compatible. Components see this `Tokens` shape regardless of scheme,
// which is exactly what we want: writing `t.color.bg.canvas` should
// have type `string`, not the literal `"#fbf9f4"`.
export type Tokens = {
  scheme: Scheme;
  color: {
    bg: {
      canvas: string;
      elevated: string;
      subtle: string;
      tint: string;
      base: string;
      page: string;
    };
    text: {
      primary: string;
      secondary: string;
      muted: string;
      placeholder: string;
      onCoral: string;
      onSlate: string;
      onInverse: string;
    };
    border: { strong: string; soft: string; hairline: string };
    brand: {
      coral: string;
      coralDim: string;
      coralDeep: string;
      coralGlow: string;
      slate: string;
      cream: string;
    };
    action: { primary: string; primaryDim: string; primaryDeep: string };
    danger: { base: string; tint: string };
    status: { ok: string; pending: string; error: string };
    amber: { base: string; text: string; tint: string };
  };
  spacing: typeof SPACING;
  radius: typeof RADIUS;
  type: typeof TYPE;
  shadow: {
    card: {
      shadowColor: string;
      shadowOffset: { width: number; height: number };
      shadowOpacity: number;
      shadowRadius: number;
      elevation: number;
    };
    floating: {
      shadowColor: string;
      shadowOffset: { width: number; height: number };
      shadowOpacity: number;
      shadowRadius: number;
      elevation: number;
    };
  };
  font: typeof FONT;
};

const LIGHT_TOKENS: Tokens = {
  scheme: "light",
  color: LIGHT_COLOR,
  spacing: SPACING,
  radius: RADIUS,
  type: TYPE,
  shadow: LIGHT_SHADOW,
  font: FONT,
};

const DARK_TOKENS: Tokens = {
  scheme: "dark",
  color: DARK_COLOR,
  spacing: SPACING,
  radius: RADIUS,
  type: TYPE,
  shadow: DARK_SHADOW,
  font: FONT,
};

// The token sets, keyed by scheme. `useTheme()` reads OS scheme and
// returns the active set.
export const themes: Record<Scheme, Tokens> = {
  light: LIGHT_TOKENS,
  dark: DARK_TOKENS,
};

// Default export points at the light tokens so legacy `import
// { tokens }` consumers keep working unchanged. Anything that needs
// dark-mode awareness should migrate to `useTheme()` instead.
export const tokens: Tokens = LIGHT_TOKENS;

// Resolve tokens for a given scheme (escape hatch for non-component
// code that knows the scheme out-of-band, e.g. SVG variant lookups).
export function tokensFor(scheme: Scheme): Tokens {
  return scheme === "dark" ? DARK_TOKENS : LIGHT_TOKENS;
}

export type { Scheme };
