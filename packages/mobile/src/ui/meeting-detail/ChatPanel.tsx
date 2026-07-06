// Read-only chat-history rendering for the past-meeting detail
// screen. Mirrors the live-meeting `ChatPane` / `ChatBubble` from
// `app/meeting.tsx` — user turns as plain coral bubbles, assistant
// turns rendered through `react-native-markdown-display`. The detail
// version is read-only (no input bar, no pending pulse — past
// meetings have no in-flight chat turns).
//
// The component renders a plain View column rather than a FlatList
// because it lives inside the detail screen's outer ScrollView, and
// nesting a same-axis VirtualizedList there is a runtime warning.
// Chat histories are bounded by the conversation length, so the
// non-virtualized cost is fine.

import { useMemo } from "react";
import { StyleSheet, Text, View } from "react-native";
import Markdown from "react-native-markdown-display";

import { useTheme } from "@/src/theme/useTheme";
import { AurisMark } from "@/src/ui/AurisMark";
import type { Item } from "@/src/wire/contract";

interface ChatPanelProps {
  items: Item[];
}

export function ChatPanel({ items }: ChatPanelProps) {
  const t = useTheme();

  if (items.length === 0) {
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
          no chat history
        </Text>
        <Text
          style={{
            ...t.type.body,
            color: t.color.text.secondary,
            textAlign: "center",
          }}
        >
          ── nothing asked during this meeting.
        </Text>
      </View>
    );
  }

  return (
    <View style={{ paddingVertical: t.spacing.sm }}>
      {items.map((it) => (
        <ChatBubble key={it.id} item={it} />
      ))}
    </View>
  );
}

/// One chat turn. `meta.role === "user"` → coral bubble, plain text.
/// `meta.role === "assistant"` (or missing) → tinted bubble, Markdown
/// body. The live screen also handles `assistant-pending` with a
/// breathe pulse — past meetings have no pending turns, so we render
/// any unknown role as plain assistant.
function ChatBubble({ item }: { item: Item }) {
  const t = useTheme();
  const role = (item.meta?.role as string | undefined) ?? "assistant";
  const isUser = role === "user";
  // Screenshots this message rode (persisted in meta by the server).
  const attachmentIds = (item.meta as { attachment_ids?: string[] } | undefined)?.attachment_ids;
  const attachmentCount = Array.isArray(attachmentIds) ? attachmentIds.length : 0;

  const markdownStyles = useMemo(
    () => ({
      body: {
        fontSize: 15,
        color: t.color.text.primary,
        lineHeight: 21,
        margin: 0,
      },
      paragraph: { marginTop: 0, marginBottom: 0 },
      strong: { fontWeight: "700" as const },
      em: { fontStyle: "italic" as const },
      code_inline: {
        backgroundColor: t.color.bg.tint,
        paddingHorizontal: 4,
        borderRadius: t.radius.sm,
        fontFamily: t.font.mono,
        fontSize: 14,
      },
      link: {
        color: t.color.brand.coral,
        textDecorationLine: "underline" as const,
      },
    }),
    [t],
  );

  return (
    <View
      style={{
        paddingHorizontal: t.spacing.md,
        paddingVertical: t.spacing.xs,
        alignItems: isUser ? "flex-end" : "flex-start",
      }}
    >
      <View
        style={{
          maxWidth: "80%",
          paddingHorizontal: t.spacing.md,
          paddingVertical: t.spacing.sm,
          borderRadius: t.radius.lg + 2,
          ...(isUser
            ? {
                backgroundColor: t.color.brand.coral,
                borderBottomRightRadius: t.radius.sm - 2,
              }
            : {
                backgroundColor: t.color.bg.subtle,
                borderWidth: 1,
                borderColor: t.color.border.soft,
                borderBottomLeftRadius: t.radius.sm - 2,
              }),
        }}
      >
        {isUser ? (
          <>
            <Text
              style={{
                ...t.type.body,
                color: t.color.text.onCoral,
              }}
            >
              {item.text}
            </Text>
            {attachmentCount > 0 ? (
              <Text
                accessibilityLabel={`${attachmentCount} image attachment${attachmentCount > 1 ? "s" : ""}`}
                style={{
                  fontSize: 11,
                  fontWeight: "600",
                  color: t.color.text.onCoral,
                  opacity: 0.85,
                  marginTop: 4,
                }}
              >
                {attachmentCount > 1 ? `🖼 ${attachmentCount}` : "🖼"}
              </Text>
            ) : null}
          </>
        ) : (
          <Markdown style={markdownStyles}>{item.text}</Markdown>
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  empty: {
    alignItems: "center",
    justifyContent: "center",
  },
});
