// Active-meeting screen. Presented as a modal above the tabs so it
// can't be accidentally dismissed by tapping a tab. Auto-pops when
// meetingState flips back to idle (server-authoritative — could be
// because the user tapped Stop here, or because another client
// stopped the meeting).
//
// Phase 2 lays out the read-only structure: mode tabs + items list
// + pause/resume/stop. Phase 3 adds audio capture. Phase 4 adds
// item expand + chat input + camera-attached moments.

import { router } from "expo-router";
import { useEffect, useMemo } from "react";
import {
  FlatList,
  Pressable,
  SafeAreaView,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from "react-native";

import { useAppStore } from "@/src/store";
import type { Item, ModeOption } from "@/src/wire/contract";

/// Same short-label map as packages/pwa/src/ui/mode-tabs.ts. Keep
/// in sync if the PWA's labels change.
const MODE_LABELS: Record<string, string> = {
  transcript: "TRANSCRIPT",
  highlights: "HIGHLIGHTS",
  actions: "ACTIONS",
  open_questions: "QUESTIONS",
  summary: "SUMMARY",
  chat: "CHAT",
};

function modeLabel(mode: ModeOption): string {
  return MODE_LABELS[mode.id] ?? mode.label.toUpperCase();
}

export default function MeetingScreen() {
  const meetingState = useAppStore((s) => s.meetingState);
  const availableModes = useAppStore((s) => s.availableModes);
  const currentMode = useAppStore((s) => s.currentMode);
  const itemsByMode = useAppStore((s) => s.itemsByMode);
  const liveInterim = useAppStore((s) => s.liveTranscriptInterim);
  const send = useAppStore((s) => s.send);

  // Server-authoritative dismissal — when meeting goes idle we pop
  // back to the tabs. Both the local "Stop" button and a stop from
  // another client (Mac, PWA) end up here.
  useEffect(() => {
    if (meetingState === "idle") {
      router.back();
    }
  }, [meetingState]);

  const items = itemsByMode[currentMode] ?? [];
  const showLiveRow = currentMode === "transcript" && liveInterim.trim().length > 0;

  return (
    <SafeAreaView style={styles.root}>
      {/* Mode tabs — horizontal scroll so a long mode list (chat
          included) doesn't get cramped on narrow phones. */}
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.tabsRow}
      >
        {availableModes.map((mode) => {
          const active = mode.id === currentMode;
          return (
            <Pressable
              key={mode.id}
              onPress={() => send({ type: "set_mode", mode: mode.id })}
              style={[styles.tab, active && styles.tabActive]}
            >
              <Text style={[styles.tabLabel, active && styles.tabLabelActive]}>
                {modeLabel(mode)}
              </Text>
            </Pressable>
          );
        })}
      </ScrollView>

      <FlatList
        style={styles.list}
        data={items}
        keyExtractor={(it) => it.id}
        renderItem={({ item }) => <ItemRow item={item} mode={currentMode} />}
        ListEmptyComponent={
          <View style={styles.empty}>
            <Text style={styles.emptyText}>{placeholderForMode(currentMode, meetingState)}</Text>
          </View>
        }
        ListFooterComponent={
          showLiveRow ? (
            <View style={styles.liveRow}>
              <Text style={styles.liveTime}>[ ⋯ ]</Text>
              <Text style={styles.liveBody}>{liveInterim}</Text>
            </View>
          ) : null
        }
        contentContainerStyle={styles.listContent}
      />

      <ControlsBar
        meetingState={meetingState}
        onPause={() => send({ type: "pause" })}
        onResume={() => send({ type: "resume" })}
        onStop={() => send({ type: "stop_meeting" })}
      />
    </SafeAreaView>
  );
}

function ItemRow({ item, mode }: { item: Item; mode: string }) {
  return (
    <View style={styles.itemRow}>
      <Text style={styles.itemTime}>{formatT(item.t)}</Text>
      <View style={styles.itemBody}>
        <Text style={styles.itemText}>{item.text}</Text>
        {renderMeta(mode, item)}
      </View>
    </View>
  );
}

function renderMeta(mode: string, item: Item): React.ReactNode {
  const meta = item.meta as Record<string, unknown> | undefined;
  if (!meta) return null;
  let text = "";
  switch (mode) {
    case "actions": {
      const owner = typeof meta.owner === "string" ? `OWNER · ${meta.owner}` : "";
      const due = typeof meta.due === "string" ? `DUE · ${meta.due}` : "";
      text = [owner, due].filter(Boolean).join("  ·  ");
      break;
    }
    case "highlights":
      text = typeof meta.importance === "string" ? `IMPORTANCE · ${meta.importance}` : "";
      break;
    case "open_questions":
      text =
        typeof meta.kind === "string"
          ? meta.kind.toUpperCase() + (typeof meta.context === "string" ? ` · ${meta.context}` : "")
          : "";
      break;
    case "transcript":
      text = typeof meta.speaker === "string" ? `SPEAKER · ${meta.speaker}` : "";
      break;
  }
  if (!text) return null;
  return <Text style={styles.itemMeta}>{text}</Text>;
}

function ControlsBar({
  meetingState,
  onPause,
  onResume,
  onStop,
}: {
  meetingState: string;
  onPause: () => void;
  onResume: () => void;
  onStop: () => void;
}) {
  const paused = meetingState === "paused";
  return (
    <View style={styles.controls}>
      <Pressable
        style={[styles.controlButton, styles.controlSecondary]}
        onPress={paused ? onResume : onPause}
      >
        <Text style={styles.controlLabel}>{paused ? "Resume" : "Pause"}</Text>
      </Pressable>
      <Pressable style={[styles.controlButton, styles.controlDanger]} onPress={onStop}>
        <Text style={[styles.controlLabel, styles.controlLabelDanger]}>Stop</Text>
      </Pressable>
    </View>
  );
}

function placeholderForMode(mode: string, state: string): string {
  if (state === "idle") return "Meeting ended.";
  if (mode === "chat") return "─ ask the agent anything";
  return `─ no ${mode.replace("_", " ")} yet`;
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
  root: { flex: 1, backgroundColor: "#fff" },

  tabsRow: {
    paddingHorizontal: 12,
    paddingVertical: 8,
    gap: 6,
  },
  tab: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 16,
    borderWidth: 1,
    borderColor: "#d5dee9",
  },
  tabActive: {
    backgroundColor: "#2563eb",
    borderColor: "#2563eb",
  },
  tabLabel: {
    fontSize: 11,
    fontWeight: "600",
    letterSpacing: 0.5,
    color: "#647386",
  },
  tabLabelActive: { color: "#fff" },

  list: { flex: 1 },
  listContent: { paddingBottom: 16 },

  itemRow: {
    flexDirection: "row",
    paddingHorizontal: 16,
    paddingVertical: 10,
    gap: 12,
    borderBottomWidth: 1,
    borderBottomColor: "#eef2f7",
  },
  itemTime: {
    fontFamily: "Menlo",
    fontSize: 12,
    color: "#647386",
    paddingTop: 2,
  },
  itemBody: { flex: 1 },
  itemText: { fontSize: 15, color: "#17212e", lineHeight: 21 },
  itemMeta: {
    fontSize: 11,
    fontWeight: "600",
    letterSpacing: 0.5,
    color: "#647386",
    marginTop: 4,
  },

  liveRow: {
    flexDirection: "row",
    paddingHorizontal: 16,
    paddingVertical: 10,
    gap: 12,
  },
  liveTime: { fontFamily: "Menlo", fontSize: 12, color: "#96a3b4" },
  liveBody: { flex: 1, fontSize: 15, color: "#96a3b4", fontStyle: "italic" },

  empty: { padding: 24, alignItems: "center" },
  emptyText: { color: "#96a3b4", fontSize: 14 },

  controls: {
    flexDirection: "row",
    gap: 8,
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderTopWidth: 1,
    borderTopColor: "#eef2f7",
  },
  controlButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 10,
    alignItems: "center",
    borderWidth: 1,
  },
  controlSecondary: {
    borderColor: "#d5dee9",
    backgroundColor: "#fff",
  },
  controlDanger: {
    borderColor: "#e5484d",
    backgroundColor: "#fff",
  },
  controlLabel: { fontSize: 14, fontWeight: "600", color: "#17212e" },
  controlLabelDanger: { color: "#e5484d" },
});
