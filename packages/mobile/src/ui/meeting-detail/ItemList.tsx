// Read-only list of `Item`s for one mode on the past-meeting detail
// screen. Editorial treatment: each row leads with a coral `▸` bullet
// and a mono `[mm:ss]` timestamp; body sits in 15pt body type; a
// MonoLabel meta line below carries speaker / owner / importance /
// kind data when present.
//
// Empty states feature the AurisMark — the same listening-room glyph
// used across the app — instead of a generic emoji.

import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import Animated, { FadeInDown } from "react-native-reanimated";

import { useTheme } from "@/src/theme/useTheme";
import type { Item } from "@/src/wire/contract";
import { AurisMark } from "@/src/ui/AurisMark";
import { MonoLabel } from "@/src/ui/components";

interface ItemListProps {
  items: Item[];
  /** Mode id — controls which meta fields get rendered under the body. */
  mode: string;
  /** Override the empty-state title (defaults derived from mode). */
  emptyTitle?: string;
}

// Empty-state copy per mode. Em-dash prefix is the system-wide marker
// for absence/placeholder; matches the rest of the design.
const EMPTY_COPY: Record<string, { title: string; body: string }> = {
  transcript: {
    title: "no transcript",
    body: "── lines will land here once the audio is processed",
  },
  assist: {
    title: "no assist insights",
    body: "── the agent stayed quiet during this meeting",
  },
  highlights: {
    title: "no highlights",
    body: "── nothing surfaced for this mode",
  },
  actions: {
    title: "no actions",
    body: "── no follow-ups extracted from this conversation",
  },
  open_questions: {
    title: "no questions",
    body: "── nothing was left open at the end",
  },
  summary: {
    title: "no summary",
    body: "── a summary appears once the meeting wraps up",
  },
};

export function ItemList({ items, mode, emptyTitle }: ItemListProps) {
  const t = useTheme();

  if (items.length === 0) {
    const empty = EMPTY_COPY[mode];
    return (
      <View
        style={[
          styles.empty,
          {
            paddingHorizontal: t.spacing.xxl,
            paddingVertical: t.spacing.xxxl,
            gap: t.spacing.sm,
          },
        ]}
      >
        <View style={{ marginBottom: t.spacing.sm }}>
          <AurisMark size={48} variant="mono" background={false} animate="breathe" />
        </View>
        <Text
          style={{
            ...t.type.subtitle,
            color: t.color.text.primary,
            textAlign: "center",
          }}
        >
          {emptyTitle ?? empty?.title ?? `no ${mode.replace(/_/g, " ")}`}
        </Text>
        {empty?.body && (
          <Text
            style={{
              ...t.type.body,
              color: t.color.text.secondary,
              textAlign: "center",
            }}
          >
            {empty.body}
          </Text>
        )}
      </View>
    );
  }

  return (
    <View>
      {items.map((it, idx) => (
        <Animated.View key={it.id} entering={FadeInDown.delay(idx * 30).duration(200)}>
          <ItemRow item={it} mode={mode} />
        </Animated.View>
      ))}
    </View>
  );
}

function ItemRow({ item, mode }: { item: Item; mode: string }) {
  const t = useTheme();
  const hasDetail = !!item.detail && item.detail.length > 0;
  // Default open — past meetings are read-only and density isn't a
  // pain because the user came here to read.
  const [open, setOpen] = useState(true);

  const meta = metaFor(mode, item);

  // The final-summary worker stores ONE item tagged
  // meta.kind === "narrative" — flowing prose, not a bullet. Render it
  // as a clean full-width paragraph block: no coral ▸, no [00:00]
  // timestamp (meaningless for a whole-meeting recap), no bullet indent.
  const isNarrative = (item.meta as Record<string, unknown> | undefined)?.kind === "narrative";

  const body = (
    <View
      style={[
        styles.row,
        {
          paddingHorizontal: t.spacing.lg,
          paddingVertical: t.spacing.md,
          gap: t.spacing.sm,
          borderBottomColor: t.color.border.soft,
        },
      ]}
    >
      {!isNarrative && (
        <View style={styles.headerRow}>
          <Text style={[styles.bullet, { color: t.color.brand.coral }]}>▸</Text>
          <Text
            style={{
              ...t.type.monoMedium,
              color: t.color.text.secondary,
            }}
          >
            {formatT(item.t)}
          </Text>
          {hasDetail && (
            <Text
              style={{
                ...t.type.mono,
                color: t.color.text.muted,
              }}
            >
              {open ? "▾" : "▸"}
            </Text>
          )}
        </View>
      )}
      <Text
        style={{
          ...t.type.body,
          color: t.color.text.primary,
          marginLeft: isNarrative ? 0 : BULLET_INDENT,
        }}
      >
        {assistTypeGlyph(mode, item)}
        {item.text}
      </Text>
      {meta && (
        <View style={{ marginLeft: BULLET_INDENT, marginTop: 2 }}>
          <MonoLabel>{meta}</MonoLabel>
        </View>
      )}
      {hasDetail && open && (
        <View
          style={[
            styles.detail,
            {
              marginLeft: BULLET_INDENT,
              marginTop: t.spacing.xs,
              paddingTop: t.spacing.sm,
              borderTopColor: t.color.border.soft,
            },
          ]}
        >
          <Text
            style={{
              ...t.type.bodySmall,
              color: t.color.text.secondary,
            }}
          >
            {item.detail}
          </Text>
        </View>
      )}
    </View>
  );

  if (!hasDetail) {
    return <View style={styles.rowWrap}>{body}</View>;
  }
  return (
    <Pressable
      onPress={() => setOpen((v) => !v)}
      style={({ pressed }) => [styles.rowWrap, pressed && { opacity: 0.7 }]}
    >
      {body}
    </Pressable>
  );
}

// `▸` glyph + mono timestamp consume ~ 84pt of leading; the body and
// meta lines align to that visual rail.
const BULLET_INDENT = 18;

function metaFor(mode: string, item: Item): string | null {
  const meta = item.meta as Record<string, unknown> | undefined;
  if (!meta) return null;
  switch (mode) {
    case "actions": {
      const owner = typeof meta.owner === "string" ? meta.owner : null;
      const due = typeof meta.due === "string" ? meta.due : null;
      const parts = [owner ? `OWNER · ${owner}` : null, due ? `DUE · ${due}` : null].filter(
        Boolean,
      ) as string[];
      return parts.length > 0 ? parts.join("  ·  ") : null;
    }
    case "highlights":
      return typeof meta.importance === "string" ? `IMPORTANCE · ${meta.importance}` : null;
    case "open_questions": {
      const kind = typeof meta.kind === "string" ? `KIND · ${meta.kind}` : null;
      const context = typeof meta.context === "string" ? `CONTEXT · ${meta.context}` : null;
      const parts = [kind, context].filter(Boolean) as string[];
      return parts.length > 0 ? parts.join("  ·  ") : null;
    }
    case "transcript":
      return typeof meta.speaker === "string" ? `SPEAKER · ${meta.speaker}` : null;
    case "assist": {
      const t = typeof meta.type === "string" ? meta.type.toUpperCase() : null;
      const c = typeof meta.confidence === "number" ? `${Math.round(meta.confidence)}%` : null;
      const parts = [t, c].filter(Boolean) as string[];
      return parts.length > 0 ? parts.join("  ·  ") : null;
    }
    default:
      return null;
  }
}

// Emoji prefix for assist-mode items — matches the live overlay's
// at-a-glance type chip (📖 definition / ❓ question / 🧠 memory /
// 💡 coach). Empty for non-assist modes.
function assistTypeGlyph(mode: string, item: Item): string {
  if (mode !== "assist") return "";
  const meta = item.meta as Record<string, unknown> | undefined;
  const t = (meta?.type as string | undefined) ?? "";
  switch (t) {
    case "definition":
      return "📖  ";
    case "question":
      return "❓  ";
    case "memory":
      return "🧠  ";
    case "coach":
      return "💡  ";
    default:
      return "";
  }
}

function formatT(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60)
    .toString()
    .padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `[${m}:${s}]`;
}

const styles = StyleSheet.create({
  empty: {
    alignItems: "center",
    justifyContent: "center",
  },
  rowWrap: {},
  row: {
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  headerRow: {
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
  },
  bullet: {
    fontSize: 14,
    lineHeight: 16,
    fontWeight: "600",
  },
  detail: {
    borderTopWidth: StyleSheet.hairlineWidth,
  },
});
