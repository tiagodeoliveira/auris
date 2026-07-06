import {
  Pressable,
  StyleSheet,
  Text,
  View,
  type GestureResponderEvent,
  type ViewStyle,
} from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface ChipProps {
  label: string;
  /** Tone shifts the background/foreground for at-a-glance status. */
  tone?: "neutral" | "brand" | "action" | "danger" | "success" | "pending" | "warning";
  /** Compact form: shorter padding, smaller text. */
  size?: "sm" | "md";
  /** Renders the chip as a Pressable. If onRemove is also set, a small × button is appended. */
  onPress?: (e: GestureResponderEvent) => void;
  onRemove?: (e: GestureResponderEvent) => void;
  style?: ViewStyle;
}

export function Chip({
  label,
  tone = "neutral",
  size = "md",
  onPress,
  onRemove,
  style,
}: ChipProps) {
  const t = useTheme();

  // Tones consume tokens through the active theme so dark mode reads
  // correctly. Brand + action both use the coral pair — they map to
  // the same hex deliberately (action IS the brand in this system).
  const TONE: Record<NonNullable<ChipProps["tone"]>, { bg: string; fg: string }> = {
    neutral: { bg: t.color.bg.tint, fg: t.color.text.primary },
    brand: { bg: t.color.brand.coralDim, fg: t.color.brand.coral },
    action: { bg: t.color.action.primaryDim, fg: t.color.action.primary },
    danger: { bg: t.color.danger.tint, fg: t.color.danger.base },
    success: {
      // Cleaner translucent on dark; opaque tint on light. The
      // hex below matches the previous "success" mint.
      bg: t.scheme === "dark" ? "rgba(22, 163, 74, 0.18)" : "#dcfce7",
      fg: t.color.status.ok,
    },
    pending: { bg: t.color.amber.tint, fg: t.color.status.pending },
    warning: { bg: t.color.amber.tint, fg: t.color.amber.text },
  };

  const toneStyle = TONE[tone];
  const padH = size === "sm" ? t.spacing.sm : t.spacing.md;
  const padV = size === "sm" ? 2 : t.spacing.xs;
  const fontSize = size === "sm" ? 11 : 13;

  const content = (
    <View
      style={[
        styles.chip,
        {
          backgroundColor: toneStyle.bg,
          paddingHorizontal: padH,
          paddingVertical: padV,
          borderRadius: t.radius.pill,
          gap: t.spacing.xs,
        },
        style,
      ]}
    >
      <Text
        style={[
          {
            ...t.type.bodyMedium,
            color: toneStyle.fg,
            fontSize,
            // Reset lineHeight to fontSize so the pill hugs the text
            // (bodyMedium ships with lineHeight 21 which would
            // balloon the chip).
            lineHeight: fontSize + 4,
          },
        ]}
        numberOfLines={1}
      >
        {label}
      </Text>
      {onRemove && (
        <Pressable
          onPress={onRemove}
          hitSlop={6}
          style={({ pressed }) => [styles.removeBtn, pressed && { opacity: 0.5 }]}
        >
          <Text style={[styles.removeGlyph, { color: toneStyle.fg }]}>×</Text>
        </Pressable>
      )}
    </View>
  );

  if (onPress) {
    return (
      <Pressable onPress={onPress} style={({ pressed }) => [pressed && { opacity: 0.7 }]}>
        {content}
      </Pressable>
    );
  }
  return content;
}

const styles = StyleSheet.create({
  chip: {
    flexDirection: "row",
    alignItems: "center",
    alignSelf: "flex-start",
  },
  removeBtn: {
    paddingHorizontal: 2,
  },
  removeGlyph: {
    fontSize: 16,
    fontWeight: "600",
    lineHeight: 16,
  },
});
