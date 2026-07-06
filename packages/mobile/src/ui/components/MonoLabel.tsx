import { Text, type TextProps } from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface MonoLabelProps extends TextProps {
  /** Tone shifts the text color. Defaults to secondary. */
  tone?: "primary" | "secondary" | "muted" | "brand" | "danger";
  /** Children — a short label string. */
  children: React.ReactNode;
}

/**
 * MonoLabel — the small uppercase mono treatment that recurs on tab
 * labels, KV meta rows, section captions, and timestamp pills. Wraps
 * a `<Text>` with `type.labelMono` + `textTransform: "uppercase"` so
 * call sites don't repeat the recipe.
 *
 * Pair with regular `<Text>` body content; this primitive is for the
 * label itself, not for paragraph copy.
 */
export function MonoLabel({ tone = "secondary", style, children, ...rest }: MonoLabelProps) {
  const t = useTheme();
  const color =
    tone === "primary"
      ? t.color.text.primary
      : tone === "muted"
        ? t.color.text.muted
        : tone === "brand"
          ? t.color.brand.coral
          : tone === "danger"
            ? t.color.danger.base
            : t.color.text.secondary;
  return (
    <Text
      {...rest}
      style={[
        {
          ...t.type.labelMono,
          color,
          textTransform: "uppercase" as const,
        },
        style,
      ]}
    >
      {children}
    </Text>
  );
}
