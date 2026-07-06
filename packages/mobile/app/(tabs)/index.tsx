// Compose tab — Phase B redesign. Listening-room aesthetic:
// Bebas Neue display title with a coral underline accent, three
// editorial cards (description+tags, audio source, attachments),
// and a tall coral CTA.
//
// Pending artifact and meeting attachments are staged into the store;
// the store reducer drains them once the meeting transitions to
// active. This screen only stages — it doesn't POST.

import { router, type Href } from "expo-router";
import { useEffect, useMemo, useState } from "react";
import {
  ActivityIndicator,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import Animated, { useAnimatedStyle } from "react-native-reanimated";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { haptics } from "@/src/lib/haptics";
import { useAppStore } from "@/src/store";
import { usePressFeedback } from "@/src/theme/motion";
import { useTheme } from "@/src/theme/useTheme";
import { AudioSourcePicker, LOCAL_MIC_ID } from "@/src/ui/AudioSourcePicker";
import { MeetingAttachStrip } from "@/src/ui/MeetingAttachStrip";
import { MetadataEditor } from "@/src/ui/MetadataEditor";
import { Card, Chip, MonoLabel, Section } from "@/src/ui/components";
import { ArtifactPicker } from "@/src/ui/artifacts";
import { ArtifactsApi, type Artifact } from "@/src/wire/artifacts-api";
import type { MeetingSummary } from "@/src/wire/meetings-api";

export default function ComposeScreen() {
  const t = useTheme();
  const wsStatus = useAppStore((s) => s.wsStatus);
  const meetingState = useAppStore((s) => s.meetingState);
  const send = useAppStore((s) => s.send);
  const audioSourceDeviceId = useAppStore((s) => s.audioSourceDeviceId);
  const setPendingAttachedMeetings = useAppStore((s) => s.setPendingAttachedMeetings);
  const pendingArtifactAttachments = useAppStore((s) => s.pendingArtifactAttachments);
  const setPendingArtifactAttachments = useAppStore((s) => s.setPendingArtifactAttachments);
  const assistSensitivity = useAppStore((s) => s.assistSensitivity);
  const setAssistSensitivity = useAppStore((s) => s.setAssistSensitivity);

  const [description, setDescription] = useState("");
  const [descFocused, setDescFocused] = useState(false);
  const [stagedMeetings, setStagedMeetings] = useState<MeetingSummary[]>([]);
  const [artifactPickerOpen, setArtifactPickerOpen] = useState(false);

  // Resolve names for artifact chips. We fetch the library lazily
  // when there's at least one staged id and we don't already have
  // its row cached. Cheap — the artifacts list endpoint is small.
  const [artifactLibrary, setArtifactLibrary] = useState<Artifact[]>([]);

  useEffect(() => {
    if (pendingArtifactAttachments.length === 0) return;
    const missing = pendingArtifactAttachments.some(
      (id) => !artifactLibrary.some((a) => a.id === id),
    );
    if (!missing) return;
    let cancelled = false;
    const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
    if (!api) return;
    void (async () => {
      try {
        const list = await api.list();
        if (!cancelled) setArtifactLibrary(list);
      } catch (e) {
        console.warn("[compose] artifact name lookup failed:", e);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [pendingArtifactAttachments, artifactLibrary]);

  const stagedArtifacts = useMemo(
    () =>
      pendingArtifactAttachments.map((id) => ({
        id,
        name: artifactLibrary.find((a) => a.id === id)?.name ?? "Artifact",
      })),
    [pendingArtifactAttachments, artifactLibrary],
  );

  // Auto-navigate into the active-meeting modal whenever a meeting
  // is/becomes active. Covers both "we just started one" and "the
  // server already had one running when we connected".
  useEffect(() => {
    if (meetingState === "active") {
      router.push("/meeting" as Href);
    }
  }, [meetingState]);

  const canStart = wsStatus === "open" && meetingState === "idle";

  // Press-feedback for the START CTA — subtle scale-down on press-in
  // gives the tall coral button the "physical" feel the design system
  // calls for (the bare Pressable opacity dip alone feels flat at this
  // button size).
  const startPress = usePressFeedback();
  const startAnimStyle = useAnimatedStyle(() => ({
    transform: [{ scale: startPress.scale.value }],
  }));

  function onStart() {
    const trimmed = description.trim();
    // Stage meeting attachments before sending start_meeting so the
    // store's active-transition reducer always sees them.
    setPendingAttachedMeetings(stagedMeetings.map((m) => m.id));
    // Artifact attachments are already in the store (the
    // ArtifactPicker's onConfirm wrote them) — no extra stage step.

    // The LOCAL_MIC sentinel means "use the phone mic" — same wire
    // semantics as omitting the field (no remote source bound).
    // Once the audio capture pipeline registers the phone as a real
    // server-side device (Phase D), this branch will pass through
    // its real id instead.
    const remoteAudioSourceId =
      audioSourceDeviceId && audioSourceDeviceId !== LOCAL_MIC_ID ? audioSourceDeviceId : undefined;

    send({
      type: "start_meeting",
      description: trimmed.length > 0 ? trimmed : undefined,
      audio_source_device_id: remoteAudioSourceId,
      assist_sensitivity: assistSensitivity,
    });
    // Success notification — the "we're listening now" moment. Fires
    // before the meeting state flips on the wire so the user feels the
    // commit even on a slow connection.
    haptics.success();
    setStagedMeetings([]);
  }

  const startLabel =
    wsStatus !== "open"
      ? "CONNECTING…"
      : meetingState !== "idle"
        ? "MEETING IN PROGRESS"
        : "START ▸";

  // Dashed coral pill — the signature affordance shared with
  // MetadataEditor and MeetingAttachStrip. Repeated locally because
  // the chip-row for artifacts lives here, not in a sub-component.
  const dashedPill = {
    paddingHorizontal: t.spacing.md,
    paddingVertical: t.spacing.xs,
    borderRadius: t.radius.pill,
    borderWidth: 1,
    borderColor: t.color.brand.coral,
    borderStyle: "dashed" as const,
  };

  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: t.color.bg.canvas }}>
      <KeyboardAvoidingView
        style={{ flex: 1 }}
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        <ScrollView
          style={{ flex: 1 }}
          contentContainerStyle={{
            padding: t.spacing.md,
            paddingBottom: t.spacing.xxl,
            gap: t.spacing.md,
          }}
          keyboardShouldPersistTaps="handled"
        >
          {/* Display title with coral underline accent + tiny mono caption. */}
          <View style={{ gap: t.spacing.xs, marginTop: t.spacing.sm }}>
            <Text
              style={{
                ...t.type.display,
                color: t.color.text.primary,
              }}
            >
              NEW MEETING
            </Text>
            <View
              style={{
                height: 2,
                width: 48,
                backgroundColor: t.color.brand.coral,
                marginTop: 2,
              }}
            />
            <MonoLabel tone="muted" style={{ marginTop: t.spacing.sm }}>
              DESCRIBE · TAG · CAPTURE
            </MonoLabel>
          </View>

          {/* Card 1: Description + Tags */}
          <Card style={{ gap: t.spacing.md }}>
            <Section
              title="Description"
              subtitle="The agent uses this to interpret the transcript."
            >
              <SectionRule color={t.color.brand.coralDim} />
              <TextInput
                style={{
                  minHeight: 110,
                  borderWidth: 1,
                  borderColor: descFocused ? t.color.brand.coral : t.color.border.strong,
                  borderRadius: t.radius.md,
                  paddingHorizontal: t.spacing.md,
                  paddingVertical: t.spacing.sm,
                  ...t.type.body,
                  color: t.color.text.primary,
                  backgroundColor: t.color.bg.elevated,
                }}
                value={description}
                onChangeText={setDescription}
                onFocus={() => setDescFocused(true)}
                onBlur={() => setDescFocused(false)}
                placeholder="What's this meeting about?"
                placeholderTextColor={t.color.text.placeholder}
                multiline
                numberOfLines={5}
                textAlignVertical="top"
                editable={meetingState === "idle"}
              />
            </Section>

            <Section title="Tags">
              <SectionRule color={t.color.brand.coralDim} />
              <MetadataEditor hideTitle />
            </Section>
          </Card>

          {/* Card 2: Audio source */}
          <Card style={{ gap: t.spacing.md }}>
            <Section title="Audio source">
              <SectionRule color={t.color.brand.coralDim} />
              <AudioSourcePicker />
            </Section>
          </Card>

          {/* Card 2.5: Assist sensitivity. Three-step picker;
              local until Start lands, then carried into the
              start_meeting intent. Same `Section` shell as the
              other cards so it reads as part of the same form. */}
          <Card style={{ gap: t.spacing.md }}>
            <Section
              title="Assist sensitivity"
              subtitle="How aggressively the agent surfaces tips during the meeting."
            >
              <SectionRule color={t.color.brand.coralDim} />
              <View style={{ flexDirection: "row", gap: t.spacing.sm }}>
                {(["aggressive", "moderate", "minimal"] as const).map((v) => {
                  const active = assistSensitivity === v;
                  return (
                    <Pressable
                      key={v}
                      onPress={() => setAssistSensitivity(v)}
                      style={({ pressed }) => ({
                        flex: 1,
                        paddingHorizontal: t.spacing.md,
                        paddingVertical: t.spacing.sm,
                        borderRadius: t.radius.pill,
                        borderWidth: 1,
                        borderColor: active ? t.color.brand.coral : t.color.border.strong,
                        backgroundColor: active ? t.color.brand.coral : "transparent",
                        opacity: pressed ? 0.7 : 1,
                        alignItems: "center",
                      })}
                    >
                      <Text
                        style={{
                          ...t.type.bodySmall,
                          fontWeight: "600",
                          color: active ? t.color.text.onCoral : t.color.text.primary,
                          textTransform: "uppercase",
                          letterSpacing: 0.5,
                        }}
                      >
                        {v}
                      </Text>
                    </Pressable>
                  );
                })}
              </View>
            </Section>
          </Card>

          {/* Card 3: Attachments — meetings + artifacts as two clearly
              labeled rows (MEETINGS / ARTIFACTS) so the user can see
              both attach affordances without having to discover them. */}
          <Card style={{ gap: t.spacing.md }}>
            <Section
              title="Attachments"
              subtitle="Past meetings and artifacts to link to this one."
            >
              <SectionRule color={t.color.brand.coralDim} />
              <View style={{ gap: t.spacing.md }}>
                <View style={{ gap: t.spacing.sm }}>
                  <MonoLabel tone="secondary">MEETINGS</MonoLabel>
                  <MeetingAttachStrip selected={stagedMeetings} onChange={setStagedMeetings} />
                </View>

                <View style={{ gap: t.spacing.sm }}>
                  <MonoLabel tone="secondary">ARTIFACTS</MonoLabel>
                  <View
                    style={{
                      flexDirection: "row",
                      flexWrap: "wrap",
                      gap: t.spacing.sm,
                      alignItems: "center",
                    }}
                  >
                    {stagedArtifacts.map((a) => (
                      <Chip
                        key={a.id}
                        label={a.name}
                        tone="neutral"
                        onRemove={() =>
                          setPendingArtifactAttachments(
                            pendingArtifactAttachments.filter((id) => id !== a.id),
                          )
                        }
                      />
                    ))}
                    <Pressable
                      onPress={() => setArtifactPickerOpen(true)}
                      style={({ pressed }) => [dashedPill, pressed && { opacity: 0.6 }]}
                    >
                      <Text
                        style={{
                          ...t.type.bodySmall,
                          color: t.color.brand.coral,
                          fontFamily: t.font.sansSemi,
                        }}
                      >
                        {stagedArtifacts.length === 0 ? "+ Attach artifact" : "+ Add"}
                      </Text>
                    </Pressable>
                  </View>
                </View>
              </View>
            </Section>
          </Card>

          {/* Start CTA — tall, coral, Bebas Neue label. Press feedback
              uses the design-system scale spring. */}
          <Animated.View style={startAnimStyle}>
            <Pressable
              onPress={onStart}
              disabled={!canStart}
              onPressIn={startPress.onPressIn}
              onPressOut={startPress.onPressOut}
              style={({ pressed }) => [
                {
                  marginTop: t.spacing.sm,
                  backgroundColor: canStart ? t.color.brand.coral : t.color.action.primary,
                  paddingVertical: t.spacing.lg,
                  minHeight: 64,
                  borderRadius: t.radius.xl,
                  alignItems: "center",
                  justifyContent: "center",
                  ...t.shadow.card,
                },
                !canStart && { opacity: 0.5 },
                pressed && canStart && { backgroundColor: t.color.brand.coralDeep },
              ]}
            >
              {wsStatus !== "open" ? (
                <View
                  style={{
                    flexDirection: "row",
                    gap: t.spacing.sm,
                    alignItems: "center",
                  }}
                >
                  <ActivityIndicator color={t.color.text.onCoral} />
                  <Text
                    style={{
                      fontFamily: t.font.display,
                      fontSize: 24,
                      letterSpacing: 2,
                      color: t.color.text.onCoral,
                      lineHeight: 28,
                    }}
                  >
                    {startLabel}
                  </Text>
                </View>
              ) : (
                <Text
                  style={{
                    fontFamily: t.font.display,
                    fontSize: 24,
                    letterSpacing: 2,
                    color: t.color.text.onCoral,
                    lineHeight: 28,
                  }}
                >
                  {startLabel}
                </Text>
              )}
            </Pressable>
          </Animated.View>
        </ScrollView>
      </KeyboardAvoidingView>

      <ArtifactPicker
        visible={artifactPickerOpen}
        onClose={() => setArtifactPickerOpen(false)}
        initialSelected={pendingArtifactAttachments}
        onConfirm={(ids) => {
          setPendingArtifactAttachments(ids);
          setArtifactPickerOpen(false);
        }}
        multi
      />
    </SafeAreaView>
  );
}

// Thin coral horizontal rule rendered immediately after a Section
// title. We don't modify the Section primitive (read-only); instead
// we drop this 32×1 sliver as the first child so it sits flush under
// the uppercase caption.
function SectionRule({ color }: { color: string }) {
  return <View style={[styles.rule, { backgroundColor: color }]} />;
}

const styles = StyleSheet.create({
  rule: {
    height: 1,
    width: 32,
    marginTop: -4,
    marginBottom: 8,
  },
});
