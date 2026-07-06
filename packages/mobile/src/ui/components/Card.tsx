import { View, type ViewProps, type ViewStyle } from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface CardProps extends ViewProps {
  /** Defaults to "md" padding (was "lg" before the 2026-05-26
   * tightening pass; the compose flow felt airy compared to PWA).
   * Pass "none" if the card holds a list. */
  padding?: "xs" | "sm" | "md" | "lg" | "xl" | "xxl" | "xxxl" | "none";
  /** Visual variant. "elevated" gets shadow + elevated bg; "flat" uses the tint background. */
  variant?: "elevated" | "flat";
}

export function Card({
  padding = "md",
  variant = "elevated",
  style,
  children,
  ...rest
}: CardProps) {
  const t = useTheme();
  const padValue = padding === "none" ? 0 : t.spacing[padding];

  // On dark mode the shadow is invisible — replace with a hairline
  // 1px ring so the card edge still reads on the slate canvas.
  const useRing = t.scheme === "dark" && variant === "elevated";

  const baseStyle: ViewStyle = {
    backgroundColor: variant === "elevated" ? t.color.bg.elevated : t.color.bg.subtle,
    borderRadius: t.radius.lg,
    padding: padValue,
    ...(variant === "elevated" && !useRing ? t.shadow.card : null),
    ...(useRing ? { borderWidth: 1, borderColor: t.color.border.hairline } : null),
  };
  return (
    <View style={[baseStyle, style]} {...rest}>
      {children}
    </View>
  );
}
