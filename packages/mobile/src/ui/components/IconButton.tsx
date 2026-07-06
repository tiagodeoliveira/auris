import {
  Pressable,
  StyleSheet,
  Text,
  type GestureResponderEvent,
  type ViewStyle,
} from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface IconButtonProps {
  /** Glyph (emoji, SF symbol fallback, or short text like "↓"). */
  glyph: string;
  /** Optional inline label rendered after the glyph. */
  label?: string;
  onPress: (e: GestureResponderEvent) => void;
  /** Tone shifts the icon + label color. Defaults to action coral. */
  tone?: "action" | "danger" | "neutral" | "brand";
  /** Show a tinted background (button-like) instead of a bare icon. */
  filled?: boolean;
  disabled?: boolean;
  accessibilityLabel?: string;
  style?: ViewStyle;
}

export function IconButton({
  glyph,
  label,
  onPress,
  tone = "action",
  filled = false,
  disabled = false,
  accessibilityLabel,
  style,
}: IconButtonProps) {
  const t = useTheme();

  // Tone -> (fg, bg). Action and brand both resolve to coral — the
  // visual brand is the action color in this system.
  const TONE: Record<NonNullable<IconButtonProps["tone"]>, { fg: string; bg: string }> = {
    action: { fg: t.color.action.primary, bg: t.color.action.primaryDim },
    danger: { fg: t.color.danger.base, bg: t.color.danger.tint },
    neutral: { fg: t.color.text.secondary, bg: t.color.bg.tint },
    brand: { fg: t.color.brand.coral, bg: t.color.brand.coralDim },
  };

  const tn = TONE[tone];
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      accessibilityLabel={accessibilityLabel ?? label ?? glyph}
      hitSlop={8}
      style={({ pressed }) => [
        styles.btn,
        { borderRadius: t.radius.md, gap: t.spacing.xs },
        filled && {
          backgroundColor: tn.bg,
          paddingHorizontal: t.spacing.md,
          paddingVertical: t.spacing.sm,
        },
        pressed && !disabled && { opacity: 0.6 },
        disabled && { opacity: 0.4 },
        style,
      ]}
    >
      <Text style={[styles.glyph, { color: tn.fg }]}>{glyph}</Text>
      {label && (
        <Text
          style={{
            ...t.type.bodySmall,
            color: tn.fg,
            // bodySmall ships with fontFamily Space Grotesk regular;
            // labels on buttons should read a touch heavier.
            fontFamily: t.font.sansSemi,
          }}
        >
          {label}
        </Text>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  btn: {
    flexDirection: "row",
    alignItems: "center",
  },
  glyph: {
    fontSize: 16,
    fontWeight: "600",
  },
});
