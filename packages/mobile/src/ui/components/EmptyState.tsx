import { StyleSheet, Text, View, type ViewProps } from "react-native";

import { useTheme } from "@/src/theme/useTheme";

interface EmptyStateProps extends ViewProps {
  /** Big glyph at the top — emoji works fine (📭, 🎙️, etc). */
  glyph?: string;
  /** Bold one-line headline. */
  title: string;
  /** Optional paragraph below the headline. */
  body?: string;
  /** Optional action slot rendered below the body (e.g. a primary CTA). */
  action?: React.ReactNode;
}

export function EmptyState({ glyph, title, body, action, style, ...rest }: EmptyStateProps) {
  const t = useTheme();
  return (
    <View
      style={[
        styles.container,
        {
          paddingHorizontal: t.spacing.xxl,
          paddingVertical: t.spacing.xxxl,
          gap: t.spacing.sm,
        },
        style,
      ]}
      {...rest}
    >
      {glyph && <Text style={[styles.glyph, { marginBottom: t.spacing.sm }]}>{glyph}</Text>}
      <Text
        style={{
          ...t.type.subtitle,
          color: t.color.text.primary,
          textAlign: "center",
        }}
      >
        {title}
      </Text>
      {body && (
        <Text
          style={{
            ...t.type.body,
            color: t.color.text.secondary,
            textAlign: "center",
          }}
        >
          {body}
        </Text>
      )}
      {action && <View style={{ marginTop: t.spacing.md }}>{action}</View>}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: "center",
    justifyContent: "center",
  },
  glyph: {
    fontSize: 40,
  },
});
