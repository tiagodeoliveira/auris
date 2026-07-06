import { StyleSheet, Text, View, type ViewProps } from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface SectionProps extends ViewProps {
  /** Small uppercase label above the section content. */
  title?: string;
  /** Optional secondary line under the title. */
  subtitle?: string;
  /** Slot rendered at the right of the title row (e.g. an action button). */
  action?: React.ReactNode;
}

export function Section({ title, subtitle, action, style, children, ...rest }: SectionProps) {
  const t = useTheme();
  return (
    <View style={[{ marginBottom: t.spacing.lg }, style]} {...rest}>
      {(title || action) && (
        <View style={[styles.headerRow, { marginBottom: t.spacing.sm }]}>
          {title && (
            <Text
              style={{
                ...t.type.caption,
                color: t.color.text.secondary,
                textTransform: "uppercase",
              }}
            >
              {title}
            </Text>
          )}
          {action && <View style={styles.actionSlot}>{action}</View>}
        </View>
      )}
      {subtitle && (
        <Text
          style={{
            ...t.type.bodySmall,
            color: t.color.text.secondary,
            marginBottom: t.spacing.sm,
          }}
        >
          {subtitle}
        </Text>
      )}
      <View>{children}</View>
    </View>
  );
}

const styles = StyleSheet.create({
  headerRow: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
  },
  actionSlot: {
    flexShrink: 0,
  },
});
