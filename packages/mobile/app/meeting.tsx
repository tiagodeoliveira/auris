// Active-meeting screen. Presented as a modal above the tabs so it
// can't be accidentally dismissed by tapping a tab. Auto-pops when
// meetingState flips back to idle (server-authoritative — could be
// because the user tapped Stop here, or because another client
// stopped the meeting).
//
// Phase C redesign: "Listening room" treatment. The AurisMark sits
// in the top-left and animates (ripple while recording, spin while
// warming up) — the brand mark IS the recording indicator. Tabs
// are PWA-style mono pills with a coral underline that slides
// between selections. Item rows lead with a coral "▸ " marker. The
// mic peak meter is the proper microphone-shaped MicActivityIcon
// rather than a thin bar. Reanimated `FadeInDown` slides new items
// in by 3pt.

import { router } from "expo-router";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  FlatList,
  Image,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  Text,
  TextInput,
  View,
  type LayoutChangeEvent,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import Animated, {
  cancelAnimation,
  Easing,
  FadeInDown,
  useAnimatedStyle,
  useSharedValue,
  withDelay,
  withRepeat,
  withTiming,
} from "react-native-reanimated";
import Markdown from "react-native-markdown-display";

import { requestMicPermission, useAudioCapture } from "@/src/audio/audio-capture";
import { LOCAL_MIC_ID } from "@/src/ui/AudioSourcePicker";
import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { haptics } from "@/src/lib/haptics";
import { useAppStore } from "@/src/store";
import { duration, easing } from "@/src/theme/motion";
import { useTheme } from "@/src/theme/useTheme";
import { MonoLabel } from "@/src/ui/components";
import { AurisMark } from "@/src/ui/AurisMark";
import { MetadataEditor } from "@/src/ui/MetadataEditor";
import { MicActivityIcon } from "@/src/ui/MicActivityIcon";
import { ArtifactPicker } from "@/src/ui/artifacts";
import { PhotoAttachButton } from "@/src/ui/PhotoAttachButton";
import {
  addPhoto,
  canAddPhoto,
  removePhoto,
  type StagedPhoto,
} from "@/src/ui/meeting-detail/chat-photo-staging";
import { ArtifactsApi } from "@/src/wire/artifacts-api";
import { ChatAttachmentsApi } from "@/src/wire/chat-attachments-api";
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
  const t = useTheme();
  const meetingState = useAppStore((s) => s.meetingState);
  const audioSourceDeviceId = useAppStore((s) => s.audioSourceDeviceId);
  const availableModes = useAppStore((s) => s.availableModes);
  const currentMode = useAppStore((s) => s.currentMode);
  const setCurrentMode = useAppStore((s) => s.setCurrentMode);
  const itemsByMode = useAppStore((s) => s.itemsByMode);
  const liveInterim = useAppStore((s) => s.liveTranscriptInterim);
  const status = useAppStore((s) => s.status);
  const metadata = useAppStore((s) => s.metadata);
  const send = useAppStore((s) => s.send);
  const currentMeetingId = useAppStore((s) => s.currentMeetingId);
  const attachedArtifactIds = useAppStore((s) => s.attachedArtifactIds);
  const meetingStartedAt = useAppStore((s) => s.meetingStartedAt);

  const audio = useAudioCapture();
  const [micRequested, setMicRequested] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);

  // Item-expand state. Two sets so cross-client auto-expand doesn't
  // override an explicit local collapse — same pattern as the PWA's
  // items-mirror.
  const [expandedIds, setExpandedIds] = useState<Set<string>>(() => new Set());
  const [manuallyCollapsed, setManuallyCollapsed] = useState<Set<string>>(() => new Set());

  // Server-authoritative dismissal — when meeting goes idle we pop
  // back to the tabs. Both the local "Stop" button and a stop from
  // another client (Mac, PWA) end up here.
  //
  // `audio` is intentionally NOT in the dep list: `useAudioCapture()`
  // returns a fresh object literal on every render, so including it
  // would re-fire the effect on every zustand re-render that follows
  // the idle transition (status, currentMeetingId, etc. all settle in
  // sequence), each one calling router.back() on an already-popped
  // stack and producing the GO_BACK toast. canGoBack() is a defensive
  // second layer in case something else triggers a re-fire.
  useEffect(() => {
    if (meetingState !== "idle") return;
    void audio.stop();
    if (router.canGoBack()) router.back();
  }, [meetingState]); // eslint-disable-line react-hooks/exhaustive-deps

  // Auto-prompt mic + start capture when the meeting becomes active —
  // but ONLY when this phone is the meeting's audio source. If the
  // source is a remote device (PWA-via-glasses, Mac), that device
  // streams /audio itself; opening a second mic here would compete
  // for the iOS audio session (the recent "Failed to start recording"
  // toast was exactly that race) and pollute the server with
  // duplicate frames. Mirrors what the Mac's `reconcileAudioSource`
  // already does.
  useEffect(() => {
    if (meetingState !== "active" || micRequested) return;
    if (audioSourceDeviceId !== LOCAL_MIC_ID) return;
    setMicRequested(true);
    void (async () => {
      const permission = await requestMicPermission();
      if (permission === "denied") {
        Alert.alert(
          "Microphone access denied",
          "Open Settings → Privacy & Security → Microphone to grant access.",
        );
        return;
      }
      if (permission === "granted") {
        await audio.start();
      }
    })();
  }, [meetingState, micRequested, audio, audioSourceDeviceId]);

  // ─── Elapsed clock ─────────────────────────────────────────────
  // Starts once on mount; we don't pause it when the user pauses the
  // mic — the meeting duration keeps running, matching the Mac
  // overlay's behavior.
  const [elapsedMs, setElapsedMs] = useState(0);
  useEffect(() => {
    const startedAt = Date.now();
    const id = setInterval(() => setElapsedMs(Date.now() - startedAt), 1000);
    return () => clearInterval(id);
  }, []);

  const items = itemsByMode[currentMode] ?? [];
  const showLiveRow = currentMode === "transcript" && liveInterim.trim().length > 0;
  const meetingTitle = (metadata.title ?? "").trim();

  function isEffectivelyExpanded(item: Item): boolean {
    if (manuallyCollapsed.has(item.id)) return false;
    if (expandedIds.has(item.id)) return true;
    return !!item.detail && item.detail.length > 0;
  }

  function toggleExpanded(item: Item) {
    if (isEffectivelyExpanded(item)) {
      setExpandedIds((prev) => {
        const next = new Set(prev);
        next.delete(item.id);
        return next;
      });
      setManuallyCollapsed((prev) => new Set(prev).add(item.id));
    } else {
      setExpandedIds((prev) => new Set(prev).add(item.id));
      setManuallyCollapsed((prev) => {
        const next = new Set(prev);
        next.delete(item.id);
        return next;
      });
      if (!item.detail || item.detail.length === 0) {
        send({ type: "expand_item", item_id: item.id });
      }
    }
  }

  async function commitArtifactPicks(picked: string[]) {
    setPickerOpen(false);
    if (!currentMeetingId) return;
    const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
    if (!api) return;
    const before = new Set(attachedArtifactIds);
    const after = new Set(picked);
    const toAttach = [...after].filter((id) => !before.has(id));
    const toDetach = [...before].filter((id) => !after.has(id));
    await Promise.all([
      ...toAttach.map((id) =>
        api.attach(currentMeetingId, id).catch((e) => {
          console.warn("[meeting] attach failed", id, e);
        }),
      ),
      ...toDetach.map((id) =>
        api.detach(currentMeetingId, id).catch((e) => {
          console.warn("[meeting] detach failed", id, e);
        }),
      ),
    ]);
  }

  // ─── AurisMark animate mode ────────────────────────────────────
  // recording        → ripple (active "we are listening" pulse)
  // warming up mic   → spin
  // otherwise        → none
  const markAnimate: "none" | "ripple" | "spin" = audio.isRecording
    ? "ripple"
    : meetingState === "active" && !audio.isRecording
      ? "spin"
      : "none";

  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: t.color.bg.canvas }}>
      {/* ─── Top brand zone ──────────────────────────────────────── */}
      <View
        style={{
          flexDirection: "row",
          alignItems: "center",
          paddingHorizontal: t.spacing.lg,
          paddingTop: t.spacing.sm,
          paddingBottom: t.spacing.sm,
          gap: t.spacing.md,
        }}
      >
        <AurisMark size={28} variant="mono" background={false} animate={markAnimate} />
        <View style={{ flex: 1, flexDirection: "row", alignItems: "baseline", gap: t.spacing.sm }}>
          {meetingTitle ? (
            <Text
              numberOfLines={1}
              style={{
                flex: 1,
                fontFamily: t.font.display,
                fontSize: 20,
                letterSpacing: 1,
                color: t.color.text.primary,
              }}
            >
              {meetingTitle.toUpperCase()}
            </Text>
          ) : (
            <View style={{ flex: 1 }}>
              <MonoLabel tone="primary" style={{ letterSpacing: 2 }}>
                AURIS · LISTENING
              </MonoLabel>
            </View>
          )}
          <Text
            style={{
              ...t.type.mono,
              color: t.color.text.secondary,
            }}
          >
            {formatElapsed(elapsedMs)}
          </Text>
        </View>
      </View>
      {/* Coral hairline below brand zone — the "listening room" thread. */}
      <View style={{ height: 1, backgroundColor: t.color.brand.coral, opacity: 0.65 }} />

      {/* ─── Tags row (compact MetadataEditor, owned by Agent B) ── */}
      <View
        style={{
          paddingHorizontal: t.spacing.lg,
          paddingTop: t.spacing.xs,
          paddingBottom: t.spacing.sm,
          borderBottomWidth: 1,
          borderBottomColor: t.color.border.soft,
        }}
      >
        <MetadataEditor compact />
      </View>

      {/* ─── Mode tabs ───────────────────────────────────────────── */}
      {/*
        Quick asks is glasses-only; on mobile the same prompts surface
        as a chip row above the chat input (see ChatPane below). Filter
        the mode out of the tab picker so it doesn't double up.
      */}
      <ModeTabsBar
        modes={availableModes.filter((m) => m.id !== "quick_asks")}
        currentMode={currentMode}
        onSelect={setCurrentMode}
      />

      {currentMode === "chat" ? (
        <ChatPane
          items={items}
          quickAsks={itemsByMode.quick_asks ?? []}
          onSend={async (text, photos) => {
            haptics.select();
            // Upload each staged photo first, then reference the
            // server-assigned ids on the chat intent — the server
            // resolves images from `attachment_ids`, it does NOT
            // auto-correlate staged uploads (same contract the Mac
            // client uses).
            const attachmentIds: string[] = [];
            if (photos.length > 0) {
              const api = ChatAttachmentsApi.from(serverUrl, () => auth0.getAccessToken());
              if (!api || !currentMeetingId) {
                Alert.alert("Could not attach photo", "No active meeting to attach to.");
                throw new Error("no upload target");
              }
              try {
                for (const p of photos) {
                  const blob = await fetch(p.uri).then((r) => r.blob());
                  attachmentIds.push(await api.upload(currentMeetingId, blob, p.mime));
                }
              } catch (e) {
                Alert.alert("Photo upload failed", e instanceof Error ? e.message : String(e));
                throw e;
              }
            }
            send({ type: "chat", text, attachment_ids: attachmentIds });
          }}
        />
      ) : (
        <FlatList
          style={{ flex: 1 }}
          data={items}
          keyExtractor={(it) => it.id}
          renderItem={({ item, index }) => (
            <ItemRow
              item={item}
              mode={currentMode}
              expanded={isEffectivelyExpanded(item)}
              onToggle={() => toggleExpanded(item)}
              index={index}
            />
          )}
          ListEmptyComponent={
            <View style={{ padding: t.spacing.xl, alignItems: "center" }}>
              <Text style={{ ...t.type.bodySmall, color: t.color.text.placeholder }}>
                {placeholderForMode(currentMode, meetingState)}
              </Text>
            </View>
          }
          ListFooterComponent={showLiveRow ? <LiveInterimRow text={liveInterim} /> : null}
          contentContainerStyle={{ paddingBottom: t.spacing.lg }}
        />
      )}

      <ControlsBar
        meetingState={meetingState}
        peak={audio.peak}
        isRecording={audio.isRecording}
        audioError={audio.error}
        attachedCount={attachedArtifactIds.length}
        onAttach={() => setPickerOpen(true)}
        // The stop button is two-tap (arm → confirm). The arm haptic
        // (medium impact) fires inside ControlsBar.handleStop on the
        // FIRST tap; the confirm haptic (warning notification) is
        // wired here on the actual `onStop` invocation so the
        // distinction reads in the hand as well as the eye.
        //
        // iOS-only note: the Mac client captures a screenshot when a
        // moment is marked; mobile does NOT. iOS provides no public
        // API to capture screen content from inside the app
        // (ReplayKit is broadcast-only and excludes voice-comm audio).
        // The moment is captured at the audio level only — the
        // server records the timestamp and the user's intent.
        onStop={() => {
          haptics.warning();
          send({ type: "stop_meeting" });
        }}
        onMarkMoment={() => {
          haptics.light();
          // t = ms offset from meeting start. A null startedAt (e.g.
          // this client joined via snapshot) sends the t==0 sentinel
          // and the server computes the offset from its meeting clock.
          send({
            type: "mark_moment",
            t: meetingStartedAt ? Math.max(0, Date.now() - meetingStartedAt) : 0,
          });
        }}
      />

      <ArtifactPicker
        visible={pickerOpen}
        onClose={() => setPickerOpen(false)}
        onConfirm={(ids) => {
          void commitArtifactPicks(ids);
        }}
        initialSelected={attachedArtifactIds}
      />
    </SafeAreaView>
  );
}

// ─── Mode tabs bar ────────────────────────────────────────────────
// PWA-style: text-only tabs with a sliding 2pt coral underline.
// The underline x/width are shared values that animate in 150ms
// ease-out when `currentMode` changes. Tabs report their layout
// via onLayout so the indicator can find the right slot.

function ModeTabsBar({
  modes,
  currentMode,
  onSelect,
}: {
  modes: ModeOption[];
  currentMode: string;
  onSelect: (id: string) => void;
}) {
  const t = useTheme();
  // Map of tab id -> {x, width} captured from onLayout.
  const layoutsRef = useRef<Record<string, { x: number; width: number }>>({});
  const indicatorX = useSharedValue(0);
  const indicatorW = useSharedValue(0);

  // Force a re-evaluation pass after layouts settle. Without this
  // the very first render's indicator would be 0-width.
  const [, setLayoutTick] = useState(0);

  function captureLayout(id: string, e: LayoutChangeEvent) {
    const { x, width } = e.nativeEvent.layout;
    layoutsRef.current[id] = { x, width };
    if (id === currentMode) {
      indicatorX.value = withTiming(x, { duration: 150, easing: easing.standard });
      indicatorW.value = withTiming(width, { duration: 150, easing: easing.standard });
    }
    setLayoutTick((n) => n + 1);
  }

  useEffect(() => {
    const slot = layoutsRef.current[currentMode];
    if (!slot) return;
    indicatorX.value = withTiming(slot.x, { duration: 150, easing: easing.standard });
    indicatorW.value = withTiming(slot.width, { duration: 150, easing: easing.standard });
  }, [currentMode, indicatorX, indicatorW]);

  const indicatorStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: indicatorX.value }],
    width: indicatorW.value,
  }));

  return (
    <View>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={{
          paddingHorizontal: t.spacing.lg,
          paddingVertical: t.spacing.sm,
          gap: t.spacing.lg,
        }}
      >
        {modes.map((mode) => {
          const active = mode.id === currentMode;
          return (
            <Pressable
              key={mode.id}
              onPress={() => onSelect(mode.id)}
              onLayout={(e) => captureLayout(mode.id, e)}
              style={{ paddingVertical: t.spacing.xs + 2 }}
              hitSlop={6}
            >
              <Text
                style={{
                  ...t.type.labelMono,
                  fontSize: 10,
                  letterSpacing: 0.6,
                  textTransform: "uppercase",
                  color: active ? t.color.brand.coral : t.color.text.secondary,
                }}
              >
                {modeLabel(mode)}
              </Text>
            </Pressable>
          );
        })}
        <Animated.View
          pointerEvents="none"
          style={[
            {
              position: "absolute",
              // left: 0 — `target.x` from onLayout is already relative
              // to the padded content area, so adding the padding here
              // would double-count it and drift the underline right.
              left: 0,
              bottom: 4,
              height: 2,
              backgroundColor: t.color.brand.coral,
              borderRadius: 1,
            },
            indicatorStyle,
          ]}
        />
      </ScrollView>
      <View style={{ height: 1, backgroundColor: t.color.border.soft }} />
    </View>
  );
}

// ─── Item row ─────────────────────────────────────────────────────
// `index < 6` rows skip the entering animation to avoid a noisy
// first-paint shuffle when the meeting screen mounts with backlog
// items already in the store. Only fresh insertions slide in.

function ItemRow({
  item,
  mode,
  expanded,
  onToggle,
  index,
}: {
  item: Item;
  mode: string;
  expanded: boolean;
  onToggle: () => void;
  index: number;
}) {
  const t = useTheme();
  const entering = index >= 6 ? FadeInDown.duration(180).springify().damping(16) : undefined;
  return (
    <Animated.View entering={entering}>
      <Pressable
        onPress={onToggle}
        style={{
          flexDirection: "row",
          paddingHorizontal: t.spacing.lg,
          paddingVertical: t.spacing.sm + 2,
          gap: t.spacing.md,
          borderBottomWidth: 1,
          borderBottomColor: t.color.border.soft,
        }}
      >
        <Text
          style={{
            ...t.type.mono,
            color: t.color.brand.coral,
            paddingTop: 2,
          }}
        >
          {formatT(item.t)}
        </Text>
        <View style={{ flex: 1 }}>
          <View style={{ flexDirection: "row", alignItems: "flex-start" }}>
            <Text
              style={{
                ...t.type.body,
                color: t.color.brand.coral,
                width: 16,
                paddingTop: 0,
              }}
            >
              {expanded ? "▾ " : "▸ "}
            </Text>
            <Text style={{ ...t.type.body, color: t.color.text.primary, flex: 1 }}>
              {assistTypeGlyph(mode, item)}
              {item.text}
            </Text>
          </View>
          {renderMeta(mode, item, t.color.text.secondary, t.type.labelMono)}
          {expanded ? (
            <View
              style={{
                marginTop: t.spacing.sm,
                paddingTop: t.spacing.sm,
                borderTopWidth: 1,
                borderTopColor: t.color.border.soft,
              }}
            >
              {item.detail && item.detail.length > 0 ? (
                <Text style={{ fontSize: 14, color: t.color.text.primary, lineHeight: 20 }}>
                  {item.detail}
                </Text>
              ) : (
                <Text
                  style={{
                    fontSize: 14,
                    color: t.color.text.placeholder,
                    fontStyle: "italic",
                  }}
                >
                  Expanding…
                </Text>
              )}
            </View>
          ) : null}
        </View>
      </Pressable>
    </Animated.View>
  );
}

function LiveInterimRow({ text }: { text: string }) {
  const t = useTheme();
  return (
    <View
      style={{
        flexDirection: "row",
        paddingHorizontal: t.spacing.lg,
        paddingVertical: t.spacing.sm + 2,
        gap: t.spacing.md,
        opacity: 0.65,
      }}
    >
      <Text
        style={{
          ...t.type.mono,
          color: t.color.brand.coral,
          paddingTop: 2,
        }}
      >
        [ ⋯ ]
      </Text>
      <View style={{ flex: 1, flexDirection: "row", alignItems: "flex-start" }}>
        <Text style={{ ...t.type.body, color: t.color.brand.coral, width: 16 }}>▸ </Text>
        <Text
          style={{
            ...t.type.body,
            color: t.color.text.primary,
            fontStyle: "italic",
            flex: 1,
          }}
        >
          {text}
        </Text>
      </View>
    </View>
  );
}

/// Chat-mode pane. Bubbles for each Q+A turn, role-aligned, with a
/// bottom-anchored input + send button. Items in chat mode arrive
/// from the agent loop tagged with `meta.role: "user" | "assistant"
/// | "assistant-pending"`.
function ChatPane({
  items,
  quickAsks,
  onSend,
}: {
  items: Item[];
  quickAsks: Item[];
  onSend: (text: string, photos: StagedPhoto[]) => Promise<void>;
}) {
  const t = useTheme();
  const [draft, setDraft] = useState("");
  const [photos, setPhotos] = useState<StagedPhoto[]>([]);
  const [sending, setSending] = useState(false);
  const listRef = useRef<FlatList<Item>>(null);

  useEffect(() => {
    if (items.length === 0) return;
    listRef.current?.scrollToEnd({ animated: true });
  }, [items.length]);

  // Lock the chat input across BOTH the optimistic placeholder phase
  // (`meta.role == "assistant-pending"`) AND the active streaming
  // phase (`meta.streaming == true`, set by the server's agent fire
  // for the assistant bubble while emitting deltas; flipped false
  // on terminal). Unlocks the moment the terminal ItemUpdated lands.
  const chatStreaming = useMemo(() => {
    return items.some((it) => {
      const m = it.meta as { role?: string; streaming?: boolean } | undefined;
      return m?.role === "assistant-pending" || m?.streaming === true;
    });
  }, [items]);

  // Safety net for WS-reconnect-mid-stream: a snapshot may carry
  // `streaming === true` items whose deltas were already consumed
  // during the disconnect window, leaving `chatStreaming` stuck
  // forever. After 60s of continuous lock, release locally; a
  // late-arriving terminal `streaming: false` is harmless.
  const [forceClearStreaming, setForceClearStreaming] = useState(false);
  useEffect(() => {
    if (!chatStreaming) {
      setForceClearStreaming(false);
      return;
    }
    const handle = setTimeout(() => setForceClearStreaming(true), 60_000);
    return () => clearTimeout(handle);
  }, [chatStreaming]);
  const inputLocked = chatStreaming && !forceClearStreaming;

  const handleSend = async () => {
    if (inputLocked || sending) return;
    const text = draft.trim();
    if (!text && photos.length === 0) return;
    setSending(true);
    try {
      await onSend(text, photos);
      setDraft("");
      setPhotos([]);
    } catch {
      // Parent surfaced the error; keep the draft + staged photos so
      // the user can retry without re-picking.
    } finally {
      setSending(false);
    }
  };

  /// Tap on a saved quick-ask chip: send its full prompt verbatim
  /// as a chat message. The label is what we render; `detail` holds
  /// the actual prompt (server packs them that way).
  const handleChipPick = (ask: Item) => {
    if (inputLocked || sending) return;
    const prompt = (ask.detail ?? "").trim();
    if (!prompt) return;
    haptics.select();
    void onSend(prompt, []);
  };

  return (
    <KeyboardAvoidingView
      style={{ flex: 1 }}
      behavior={Platform.OS === "ios" ? "padding" : undefined}
      keyboardVerticalOffset={Platform.OS === "ios" ? 80 : 0}
    >
      <FlatList
        ref={listRef}
        style={{ flex: 1 }}
        data={items}
        keyExtractor={(it) => it.id}
        renderItem={({ item, index }) => <ChatBubble item={item} index={index} />}
        ListEmptyComponent={
          <View style={{ padding: t.spacing.xl, alignItems: "center" }}>
            <Text style={{ ...t.type.bodySmall, color: t.color.text.placeholder }}>
              ─ ask the agent anything
            </Text>
          </View>
        }
        contentContainerStyle={{ paddingTop: t.spacing.sm, paddingBottom: t.spacing.sm }}
      />
      {quickAsks.length > 0 && (
        // `flexGrow: 0` prevents the ScrollView from claiming
        // leftover vertical space above the input row, which would
        // otherwise stretch its children (no fixed cross-axis size)
        // into tall stadium-shaped capsules. `alignItems: center`
        // pins chip height to their own content rather than the
        // ScrollView's vertical bounds — belt-and-suspenders against
        // the same default-stretch behavior at the contentContainer
        // level.
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          style={{ flexGrow: 0 }}
          contentContainerStyle={{
            paddingHorizontal: t.spacing.md,
            paddingTop: t.spacing.xs,
            paddingBottom: t.spacing.xs,
            gap: t.spacing.xs,
            alignItems: "center",
          }}
        >
          {quickAsks.map((ask) => (
            <Pressable
              key={ask.id}
              onPress={() => handleChipPick(ask)}
              style={({ pressed }) => ({
                paddingHorizontal: t.spacing.md,
                paddingVertical: t.spacing.xs,
                borderRadius: t.radius.pill,
                borderWidth: 1,
                borderColor: t.color.brand.coral,
                opacity: pressed ? 0.6 : 1,
              })}
            >
              <Text style={{ color: t.color.brand.coral, ...t.type.bodySmall }}>{ask.text}</Text>
            </Pressable>
          ))}
        </ScrollView>
      )}
      {photos.length > 0 && (
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          style={{ flexGrow: 0 }}
          contentContainerStyle={{
            paddingHorizontal: t.spacing.md,
            paddingTop: t.spacing.xs,
            gap: t.spacing.sm,
            alignItems: "center",
          }}
        >
          {photos.map((p) => (
            <Pressable
              key={p.id}
              onPress={() => setPhotos((cur) => removePhoto(cur, p.id))}
              accessibilityLabel="Remove photo"
            >
              <Image
                source={{ uri: p.uri }}
                style={{
                  width: 56,
                  height: 56,
                  borderRadius: t.radius.md,
                  borderWidth: 1,
                  borderColor: t.color.border.soft,
                }}
              />
              <View
                style={{
                  position: "absolute",
                  top: -6,
                  right: -6,
                  width: 20,
                  height: 20,
                  borderRadius: 10,
                  backgroundColor: t.color.bg.canvas,
                  alignItems: "center",
                  justifyContent: "center",
                }}
              >
                <Text style={{ fontSize: 12, color: t.color.text.primary }}>✕</Text>
              </View>
            </Pressable>
          ))}
        </ScrollView>
      )}
      <View
        style={{
          flexDirection: "row",
          gap: t.spacing.sm,
          paddingHorizontal: t.spacing.md,
          paddingVertical: t.spacing.sm + 2,
          borderTopWidth: 1,
          borderTopColor: t.color.border.soft,
          backgroundColor: t.color.bg.canvas,
        }}
      >
        <PhotoAttachButton
          disabled={inputLocked || sending || !canAddPhoto(photos)}
          onPicked={(photo) => setPhotos((cur) => addPhoto(cur, photo))}
        />
        <TextInput
          style={{
            flex: 1,
            minHeight: 40,
            paddingHorizontal: t.spacing.md,
            paddingVertical: t.spacing.sm + 2,
            borderWidth: 1,
            borderColor: t.color.border.strong,
            borderRadius: t.radius.md + 2,
            ...t.type.body,
            color: t.color.text.primary,
            backgroundColor: t.color.bg.elevated,
          }}
          value={draft}
          onChangeText={setDraft}
          placeholder="Ask the agent…"
          placeholderTextColor={t.color.text.placeholder}
          returnKeyType="send"
          onSubmitEditing={() => void handleSend()}
          editable={!inputLocked}
        />
        <Pressable
          style={({ pressed }) => ({
            paddingHorizontal: t.spacing.lg,
            justifyContent: "center",
            backgroundColor: t.color.brand.coral,
            borderRadius: t.radius.md + 2,
            opacity: inputLocked || sending || (!draft.trim() && photos.length === 0) ? 0.5 : pressed ? 0.85 : 1,
          })}
          disabled={inputLocked || sending || (!draft.trim() && photos.length === 0)}
          onPress={() => void handleSend()}
        >
          <Text
            style={{
              color: t.color.text.onCoral,
              ...t.type.body,
              fontFamily: t.font.sansSemi,
            }}
          >
            Send
          </Text>
        </Pressable>
      </View>
    </KeyboardAvoidingView>
  );
}

function ChatBubble({ item, index }: { item: Item; index: number }) {
  const t = useTheme();
  const role = (item.meta?.role as string | undefined) ?? "assistant";
  const streaming = (item.meta as { streaming?: boolean } | undefined)?.streaming === true;
  const isUser = role === "user";
  // Render TypingDots either for the server's optimistic placeholder
  // OR during the brief window where the role has flipped to
  // "assistant" + streaming=true but the first token hasn't landed
  // yet (usually ≤500ms before deltas start arriving).
  const pending =
    role === "assistant-pending" ||
    (role === "assistant" && streaming && (item.text ?? "").length === 0);
  const isAssistant = role === "assistant";
  // Screenshots the message rode (user bubbles only). We show the
  // count, not the images.
  const attachmentIds = (item.meta as { attachment_ids?: string[] } | undefined)?.attachment_ids;
  const attachmentCount = Array.isArray(attachmentIds) ? attachmentIds.length : 0;

  // Only new bubbles animate in (older ones present on mount don't).
  const entering = index >= 6 ? FadeInDown.duration(180).springify().damping(16) : undefined;

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
    <Animated.View
      entering={entering}
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
        {isAssistant || pending ? (
          pending ? (
            // Three-dot staggered typing indicator. Sits inside the
            // same bubble chrome the real assistant reply will use,
            // so when the server replaces the placeholder with the
            // final text the bubble layout doesn't jump.
            <TypingDots color={t.color.text.secondary} />
          ) : (
            <Markdown style={markdownStyles}>{item.text}</Markdown>
          )
        ) : (
          <>
            <Text
              style={{
                ...t.type.body,
                color: isUser ? t.color.text.onCoral : t.color.text.primary,
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
        )}
      </View>
    </Animated.View>
  );
}

/// Three-dot "agent is thinking" indicator. Each dot pulses (scale +
/// opacity) with a staggered offset so the eye reads it as activity.
/// Lives inside the same chat-bubble chrome that the real assistant
/// reply will use, so when the server swaps the pending placeholder
/// for the final text only the content swaps — bubble layout holds.
function TypingDots({ color }: { color: string }) {
  return (
    <View
      style={{
        flexDirection: "row",
        alignItems: "center",
        gap: 4,
        paddingVertical: 4,
      }}
      accessibilityLabel="Agent is thinking"
    >
      <TypingDot color={color} delay={0} />
      <TypingDot color={color} delay={150} />
      <TypingDot color={color} delay={300} />
    </View>
  );
}

function TypingDot({ color, delay }: { color: string; delay: number }) {
  // Drive both opacity and a tiny lift from one shared progress
  // value. `withRepeat(withTiming, -1)` loops forever; the per-dot
  // `withDelay` staggers the start so the dots pulse in sequence.
  const progress = useSharedValue(0);
  useEffect(() => {
    progress.value = withDelay(
      delay,
      withRepeat(withTiming(1, { duration: 1200, easing: Easing.inOut(Easing.cubic) }), -1, false),
    );
    return () => {
      cancelAnimation(progress);
    };
  }, [delay, progress]);

  const style = useAnimatedStyle(() => {
    // Sharp peak at ~25% of the cycle, fade to baseline by 60%.
    const v = progress.value;
    const opacity = v < 0.25 ? 0.3 + v * 2.8 : v < 0.6 ? 1.0 - (v - 0.25) * 1.7 : 0.3;
    const lift = v < 0.25 ? -v * 8 : 0;
    return { opacity, transform: [{ translateY: lift }] };
  });

  return (
    <Animated.View
      style={[
        {
          width: 6,
          height: 6,
          borderRadius: 3,
          backgroundColor: color,
        },
        style,
      ]}
    />
  );
}

/// Emoji prefix for assist-mode items, distinguishing the four
/// sub-types at a glance:
///   📖 definition / ❓ question / 🧠 memory / 💡 coach
/// Empty string for non-assist modes (so the prefix is a no-op).
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

function renderMeta(
  mode: string,
  item: Item,
  secondaryColor: string,
  labelMono: { fontFamily: string; fontSize: number; letterSpacing: number; lineHeight: number },
): React.ReactNode {
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
  return (
    <Text
      style={{
        ...labelMono,
        color: secondaryColor,
        marginTop: 4,
        marginLeft: 16, // align under the body, past the "▸ " marker
        textTransform: "uppercase",
      }}
    >
      {text}
    </Text>
  );
}

// ─── Controls bar ─────────────────────────────────────────────────
function ControlsBar({
  meetingState,
  peak,
  isRecording,
  audioError,
  attachedCount,
  onAttach,
  onStop,
  onMarkMoment,
}: {
  meetingState: string;
  peak: number;
  isRecording: boolean;
  audioError: string | null;
  attachedCount: number;
  onAttach: () => void;
  onStop: () => void;
  onMarkMoment: () => void;
}) {
  const t = useTheme();

  // Stop is two-tap: first tap arms the button for 3s; a second
  // tap inside that window actually stops. Visual transforms into
  // a wider "CONFIRM?" pill that blinks while armed.
  const [stopArmed, setStopArmed] = useState(false);
  const armTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const blinkOpacity = useSharedValue(1);
  useEffect(() => {
    if (stopArmed) {
      blinkOpacity.value = withTiming(0.4, {
        duration: 500,
        easing: Easing.inOut(Easing.cubic),
      });
      const id = setInterval(() => {
        blinkOpacity.value = withTiming(blinkOpacity.value > 0.7 ? 0.4 : 1, {
          duration: 500,
          easing: Easing.inOut(Easing.cubic),
        });
      }, 500);
      return () => {
        clearInterval(id);
        cancelAnimation(blinkOpacity);
        blinkOpacity.value = 1;
      };
    }
    blinkOpacity.value = 1;
    return undefined;
  }, [stopArmed, blinkOpacity]);

  const blinkStyle = useAnimatedStyle(() => ({ opacity: blinkOpacity.value }));

  useEffect(
    () => () => {
      if (armTimer.current) clearTimeout(armTimer.current);
    },
    [],
  );

  function handleStop() {
    if (stopArmed) {
      if (armTimer.current) clearTimeout(armTimer.current);
      setStopArmed(false);
      // `onStop` is responsible for the confirm haptic — keep the
      // notification next to the actual destructive send rather than
      // splitting it across handlers.
      onStop();
      return;
    }
    // First tap arms; medium impact says "you've armed something, a
    // second tap commits". Same vocabulary as the delete-arm in
    // ArtifactRow.
    haptics.medium();
    setStopArmed(true);
    armTimer.current = setTimeout(() => setStopArmed(false), 3000);
  }

  return (
    <View
      style={{
        borderTopWidth: 1,
        borderTopColor: t.color.border.soft,
        backgroundColor: t.color.bg.canvas,
      }}
    >
      {/* Mic activity icon — replaces the old thin peak meter. */}
      <View
        style={{
          alignItems: "center",
          paddingTop: t.spacing.sm + 2,
          paddingBottom: t.spacing.xs,
        }}
      >
        <MicActivityIcon size={48} peak={peak} isRecording={isRecording} isPaused={false} />
      </View>

      {audioError ? (
        <View
          style={{
            marginHorizontal: t.spacing.lg,
            marginTop: t.spacing.sm,
            paddingHorizontal: t.spacing.md,
            paddingVertical: t.spacing.sm,
            backgroundColor: t.color.danger.tint,
            borderRadius: t.radius.md,
          }}
        >
          <Text
            style={{ fontSize: 13, color: t.color.danger.base, lineHeight: 18 }}
            numberOfLines={2}
          >
            {audioError}
          </Text>
        </View>
      ) : null}

      {/* Attach affordance — dashed coral pill, self-aligned center. */}
      <View
        style={{
          paddingHorizontal: t.spacing.lg,
          paddingTop: t.spacing.sm,
          alignItems: "center",
        }}
      >
        <Pressable
          style={({ pressed }) => ({
            paddingHorizontal: t.spacing.md,
            paddingVertical: t.spacing.xs + 2,
            borderRadius: t.radius.pill,
            borderWidth: 1,
            borderColor: t.color.brand.coral,
            borderStyle: "dashed",
            opacity: pressed ? 0.6 : 1,
          })}
          onPress={onAttach}
        >
          <Text
            style={{
              ...t.type.bodySmall,
              color: t.color.brand.coral,
              fontFamily: t.font.sansSemi,
            }}
          >
            {attachedCount === 0
              ? "+ attach artifact"
              : `+ attach artifact (${attachedCount} attached)`}
          </Text>
        </Pressable>
      </View>

      {/* Action row: Moment / Pause-Resume / Stop. */}
      <View
        style={{
          flexDirection: "row",
          gap: t.spacing.sm,
          paddingHorizontal: t.spacing.lg,
          paddingVertical: t.spacing.md,
        }}
      >
        {/* MOMENT — amber pill */}
        <Pressable
          onPress={onMarkMoment}
          style={({ pressed }) => ({
            flex: 1,
            paddingVertical: t.spacing.md,
            borderRadius: t.radius.md + 2,
            alignItems: "center",
            borderWidth: 1,
            borderColor: t.color.amber.base,
            backgroundColor: t.color.amber.tint,
            opacity: pressed ? 0.75 : 1,
          })}
        >
          <Text
            style={{
              fontFamily: t.font.display,
              fontSize: 18,
              letterSpacing: 1.5,
              color: t.color.amber.text,
            }}
          >
            ◆ MOMENT
          </Text>
        </Pressable>

        {/* STOP — danger; arms to CONFIRM? on first tap */}
        <Animated.View style={[{ flex: stopArmed ? 1.6 : 1 }, blinkStyle]}>
          <Pressable
            onPress={handleStop}
            style={({ pressed }) => ({
              paddingVertical: t.spacing.md,
              borderRadius: t.radius.md + 2,
              alignItems: "center",
              borderWidth: 1,
              borderColor: t.color.danger.base,
              backgroundColor: stopArmed ? t.color.danger.base : t.color.bg.elevated,
              opacity: pressed ? 0.75 : 1,
            })}
          >
            <Text
              style={{
                fontFamily: t.font.display,
                fontSize: 18,
                letterSpacing: 1.5,
                color: stopArmed ? t.color.text.onCoral : t.color.danger.base,
              }}
            >
              {stopArmed ? "CONFIRM?" : "■ STOP"}
            </Text>
          </Pressable>
        </Animated.View>
      </View>
    </View>
  );
}

function placeholderForMode(mode: string, state: string): string {
  if (state === "idle") return "— meeting ended";
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

function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60)
    .toString()
    .padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}
