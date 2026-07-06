// Row component for the Artifacts tab list. Encapsulates the row's
// internal state (expand for long_summary, two-click delete arm) so
// the parent screen only deals with `onDeleted` callbacks.
//
// Phase D refresh: the row now reads as a self-contained <Card>-style
// surface — status chips replace the glyph treatment, the armed
// delete shifts the row into a danger-tint state, and the chevron is
// rendered in coral. Tap anywhere outside the chevron + trash hit
// zones pushes /artifact/[id]. Behaviour (selectability, two-click
// delete window, expand semantics) is unchanged — this is a visual
// refresh, not a behaviour refactor.

import { router, type Href } from "expo-router";
import { useEffect, useRef, useState } from "react";
import { Pressable, StyleSheet, Text, View, type GestureResponderEvent } from "react-native";

import { haptics } from "@/src/lib/haptics";
import { useTheme } from "@/src/theme/useTheme";
import { Chip, IconButton, MonoLabel } from "@/src/ui/components";
import type { Artifact } from "@/src/wire/artifacts-api";

interface ArtifactRowProps {
  artifact: Artifact;
  /// Fires after a successful DELETE. The parent removes the row
  /// from its local list optimistically and reloads on next focus.
  onDeleted: (id: string) => Promise<void>;
}

export function ArtifactRow({ artifact, onDeleted }: ArtifactRowProps) {
  const t = useTheme();
  const [expanded, setExpanded] = useState(false);
  const [armed, setArmed] = useState(false);
  const armTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (armTimerRef.current) clearTimeout(armTimerRef.current);
    };
  }, []);

  const canExpand =
    artifact.summary_status === "done" &&
    !!artifact.long_summary &&
    artifact.long_summary.trim().length > 0;

  function disarmSoon() {
    if (armTimerRef.current) clearTimeout(armTimerRef.current);
    armTimerRef.current = setTimeout(() => setArmed(false), 3000);
  }

  function handleDelete(e: GestureResponderEvent) {
    e.stopPropagation();
    if (!armed) {
      // Arm tap: medium impact (same vocabulary as the meeting Stop
      // arm) — says "you've primed a destructive action".
      haptics.medium();
      setArmed(true);
      disarmSoon();
      return;
    }
    if (armTimerRef.current) clearTimeout(armTimerRef.current);
    setArmed(false);
    // Confirm tap: warning notification — the row is about to vanish.
    haptics.warning();
    void onDeleted(artifact.id);
  }

  function handleExpand(e: GestureResponderEvent) {
    e.stopPropagation();
    setExpanded((v) => !v);
  }

  function openDetail() {
    router.push(`/artifact/${artifact.id}` as Href);
  }

  // The Card silhouette is drawn inline so we can swap its background
  // when armed (danger tint reads as "you're one tap away"). Re-using
  // <Card> directly would force a wrapper for the press handler.
  const ringColor = t.scheme === "dark" ? t.color.border.hairline : "transparent";
  return (
    <Pressable
      style={({ pressed }) => [
        styles.row,
        {
          backgroundColor: armed ? t.color.danger.tint : t.color.bg.elevated,
          borderRadius: t.radius.lg,
          padding: t.spacing.lg,
          gap: t.spacing.xs,
          borderWidth: t.scheme === "dark" ? 1 : 0,
          borderColor: ringColor,
          ...(t.scheme === "light" ? t.shadow.card : null),
        },
        pressed && { opacity: 0.92 },
      ]}
      onPress={openDetail}
    >
      <View style={styles.headerLine}>
        <StatusChip status={artifact.summary_status} />
        <Text
          style={{
            ...t.type.body,
            fontFamily: t.font.sansSemi,
            color: t.color.text.primary,
            flex: 1,
          }}
          numberOfLines={1}
        >
          {artifact.name}
        </Text>
      </View>

      <View style={[styles.metaLine, { gap: t.spacing.sm }]}>
        <Chip label={artifact.mime_type} tone="neutral" size="sm" />
        <MonoLabel tone="muted">{formatBytes(artifact.size_bytes)}</MonoLabel>
      </View>

      {artifact.short_summary ? (
        <Text
          style={{
            ...t.type.bodySmall,
            color: t.color.text.secondary,
          }}
          numberOfLines={expanded ? undefined : 2}
        >
          {artifact.short_summary}
        </Text>
      ) : (
        <Text
          style={{
            ...t.type.bodySmall,
            color: t.color.text.placeholder,
            fontStyle: "italic",
          }}
          numberOfLines={1}
        >
          {summaryFallback(artifact.summary_status)}
        </Text>
      )}

      {expanded && canExpand && (
        <View
          style={{
            marginTop: t.spacing.xs,
            padding: t.spacing.md,
            backgroundColor: t.color.bg.subtle,
            borderRadius: t.radius.md,
          }}
        >
          <Text style={{ ...t.type.bodySmall, color: t.color.text.primary }}>
            {artifact.long_summary}
          </Text>
        </View>
      )}

      <View style={[styles.actionsLine, { marginTop: t.spacing.xs, gap: t.spacing.sm }]}>
        {canExpand && (
          <Pressable
            onPress={handleExpand}
            hitSlop={8}
            style={[styles.chevronBtn, { gap: t.spacing.xs, paddingVertical: t.spacing.xs }]}
          >
            <Text
              style={{
                ...t.type.bodySmall,
                color: t.color.brand.coral,
                fontFamily: t.font.sansSemi,
                width: 12,
              }}
            >
              {expanded ? "▾" : "▸"}
            </Text>
            <MonoLabel tone="brand">{expanded ? "HIDE" : "READ MORE"}</MonoLabel>
          </Pressable>
        )}
        <View style={styles.spacer} />
        {armed ? (
          <IconButton
            glyph="!"
            label="CONFIRM?"
            tone="danger"
            filled
            onPress={handleDelete}
            accessibilityLabel="Confirm delete"
          />
        ) : (
          <IconButton
            glyph="×"
            tone="danger"
            onPress={handleDelete}
            accessibilityLabel="Delete artifact"
          />
        )}
      </View>
    </Pressable>
  );
}

export function StatusChip({ status }: { status: string }) {
  switch (status) {
    case "done":
      return <Chip label="DONE" tone="success" size="sm" />;
    case "pending":
      return <Chip label="PROCESSING" tone="pending" size="sm" />;
    case "failed":
      return <Chip label="FAILED" tone="danger" size="sm" />;
    default:
      return <Chip label={status.toUpperCase() || "UNKNOWN"} tone="neutral" size="sm" />;
  }
}

function summaryFallback(status: string): string {
  switch (status) {
    case "pending":
      return "── summary generating";
    case "failed":
      return "── summary failed";
    default:
      return "── no summary";
  }
}

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

const styles = StyleSheet.create({
  row: {
    flexDirection: "column",
  },
  headerLine: {
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
  },
  metaLine: {
    flexDirection: "row",
    alignItems: "center",
  },
  actionsLine: {
    flexDirection: "row",
    alignItems: "center",
  },
  spacer: { flex: 1 },
  chevronBtn: {
    flexDirection: "row",
    alignItems: "center",
  },
});
