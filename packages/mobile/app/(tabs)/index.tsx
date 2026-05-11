// Compose tab — Phase 2. Description input + Start button. On
// successful start_meeting, the server flips meetingState to active
// and we navigate to the /meeting modal automatically.

import { router, type Href } from "expo-router";
import { useEffect, useState } from "react";
import {
  KeyboardAvoidingView,
  Platform,
  Pressable,
  SafeAreaView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from "react-native";

import { serverUrl } from "@/src/config";
import { useAppStore } from "@/src/store";
import { MeetingAttachStrip } from "@/src/ui/MeetingAttachStrip";
import type { MeetingSummary } from "@/src/wire/meetings-api";

export default function ComposeScreen() {
  const wsStatus = useAppStore((s) => s.wsStatus);
  const meetingState = useAppStore((s) => s.meetingState);
  const send = useAppStore((s) => s.send);
  const setPendingAttachedMeetings = useAppStore((s) => s.setPendingAttachedMeetings);

  const [description, setDescription] = useState("");
  const [stagedMeetings, setStagedMeetings] = useState<MeetingSummary[]>([]);

  // Auto-navigate into the active-meeting modal whenever a meeting
  // is/becomes active. Covers both "we just started one" and "the
  // server already had one running when we connected". Cast to
  // Href because expo-router's typed-routes registry is regenerated
  // by the dev server, not by `tsc --noEmit` in CI — the route
  // exists at runtime, the type just hasn't propagated yet.
  useEffect(() => {
    if (meetingState === "active" || meetingState === "paused") {
      router.push("/meeting" as Href);
    }
  }, [meetingState]);

  const canStart = wsStatus === "open" && meetingState === "idle";

  return (
    <SafeAreaView style={styles.root}>
      <KeyboardAvoidingView
        style={styles.flex}
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        <View style={styles.body}>
          <Text style={styles.title}>Start a meeting</Text>
          <Text style={styles.hint}>
            Describe what this meeting is about. Optional, but the agent uses it to interpret the
            transcript.
          </Text>

          <TextInput
            style={styles.input}
            value={description}
            onChangeText={setDescription}
            placeholder="What's this meeting about?"
            placeholderTextColor="#96a3b4"
            multiline
            textAlignVertical="top"
            editable={meetingState === "idle"}
          />

          <MeetingAttachStrip selected={stagedMeetings} onChange={setStagedMeetings} />

          <Pressable
            style={[styles.startButton, !canStart && styles.startButtonDisabled]}
            disabled={!canStart}
            onPress={() => {
              const trimmed = description.trim();
              // Stage before sending start_meeting so the active
              // transition handler always sees them.
              setPendingAttachedMeetings(stagedMeetings.map((m) => m.id));
              send({
                type: "start_meeting",
                description: trimmed.length > 0 ? trimmed : undefined,
              });
              setStagedMeetings([]);
            }}
          >
            <Text style={styles.startButtonText}>
              {meetingState !== "idle"
                ? "Meeting in progress…"
                : wsStatus !== "open"
                  ? "Connecting…"
                  : "Start"}
            </Text>
          </Pressable>

          <View style={styles.footer}>
            <Text style={styles.footerLine}>
              <Text style={styles.footerLabel}>server </Text>
              {serverUrl}
            </Text>
            <Text style={styles.footerLine}>
              <Text style={styles.footerLabel}>ws </Text>
              {wsStatus}
            </Text>
          </View>
        </View>
      </KeyboardAvoidingView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  flex: { flex: 1 },
  body: { flex: 1, padding: 16, gap: 12 },
  title: { fontSize: 22, fontWeight: "600" },
  hint: { color: "#647386", fontSize: 14, lineHeight: 20 },
  input: {
    minHeight: 120,
    borderWidth: 1,
    borderColor: "#d5dee9",
    borderRadius: 10,
    padding: 12,
    fontSize: 15,
    lineHeight: 21,
    color: "#17212e",
    backgroundColor: "#fff",
  },
  startButton: {
    marginTop: 8,
    backgroundColor: "#2563eb",
    paddingVertical: 14,
    borderRadius: 10,
    alignItems: "center",
  },
  startButtonDisabled: { opacity: 0.5 },
  startButtonText: { color: "#fff", fontSize: 16, fontWeight: "600" },
  footer: {
    marginTop: "auto",
    gap: 2,
  },
  footerLine: {
    fontSize: 12,
    color: "#96a3b4",
  },
  footerLabel: {
    fontWeight: "600",
    color: "#647386",
  },
});
