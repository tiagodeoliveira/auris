// React hook returning the active token set for the current effective
// color scheme. Single source of truth for "is the user in dark mode"
// — downstream components MUST consume tokens via this hook (not the
// static `tokens` export) so they re-render when the scheme flips at
// runtime.
//
// Resolution order (first match wins):
//   1. `themeOverride` slice on the app store ("light" | "dark") —
//      set by Settings → Appearance; persisted to AsyncStorage.
//   2. `themeOverride === "system"` falls through to the OS scheme.
//   3. `useColorScheme()` — the platform's current setting.
//
// Both the store selector and `useColorScheme()` are reactive, so the
// hook re-renders the entire tree whenever the user toggles the picker
// or flips the OS appearance.

import { useColorScheme } from "react-native";

import { useAppStore } from "../store";
import { themes, type Scheme, type Tokens } from "./tokens";

export function useTheme(): Tokens {
  const override = useAppStore((s) => s.themeOverride);
  const osScheme = useColorScheme();

  const effective: Scheme =
    override === "light" || override === "dark" ? override : ((osScheme ?? "light") as Scheme);

  return themes[effective] ?? themes.light;
}

// Sibling hook for when a component needs the raw scheme string
// (e.g. to pick an SVG variant or decide between shadow vs. ring).
// Honors the same override resolution as `useTheme()` above.
export function useScheme(): Scheme {
  const override = useAppStore((s) => s.themeOverride);
  const osScheme = useColorScheme();
  if (override === "light" || override === "dark") return override;
  return (osScheme ?? "light") as Scheme;
}
