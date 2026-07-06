// Branded pull-to-refresh — a thin wrapper around the platform
// `RefreshControl` that pre-supplies coral tints from the theme.
//
// Why a wrapper instead of inlining the colors at each call site:
//   - The tint props differ per-platform (`tintColor` on iOS,
//     `colors` array on Android, plus `progressBackgroundColor` on
//     Android for the spinner halo). Centralizing them keeps the
//     coral branding consistent across history / artifacts / past-
//     meeting detail without copy-paste drift the next time the
//     brand palette tweaks.
//   - Consumers pass through `refreshing` + `onRefresh`; everything
//     else is opinionated.
//
// TODO (deeper polish): replace the platform spinner with an animated
// AurisMark pulse. That requires a portal-style overlay that tracks
// scroll-y while the user drags — the platform RefreshControl doesn't
// expose its progress so it can't drive a custom mark. Out of scope
// for this pass; documented here so the next iteration can pick it up.

import { RefreshControl, type RefreshControlProps } from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface BrandedRefreshProps {
  refreshing: boolean;
  onRefresh: () => void;
  /// Optional accessibility label; defaults are platform-supplied.
  accessibilityLabel?: string;
  /// Escape hatch for any RefreshControl prop we haven't surfaced.
  /// Use sparingly — the point of the wrapper is consistency.
  extraProps?: Partial<RefreshControlProps>;
}

export function BrandedRefresh({
  refreshing,
  onRefresh,
  accessibilityLabel,
  extraProps,
}: BrandedRefreshProps) {
  const t = useTheme();
  return (
    <RefreshControl
      refreshing={refreshing}
      onRefresh={onRefresh}
      // iOS — the system spinner tint.
      tintColor={t.color.brand.coral}
      // Android — color stops driving the rotating arc + the halo
      // background. Coral on the elevated surface so the spinner
      // doesn't dissolve into the page background in dark mode.
      colors={[t.color.brand.coral]}
      progressBackgroundColor={t.color.bg.elevated}
      accessibilityLabel={accessibilityLabel}
      {...extraProps}
    />
  );
}
