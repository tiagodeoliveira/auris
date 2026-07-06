// Past-meeting detail. Read-only fetch from /meetings/:id; rendered
// as a normal stack screen (NOT the live-meeting fullscreen modal —
// that's app/meeting.tsx with no [id] segment, intentionally).
//
// Phase E layout: editorial header (Bebas Neue display title, coral
// underline, mono timing line, PDF action) followed by the tag /
// description / LLM-usage blocks; then a tab strip in PWA style with
// an animated coral underline; then the section footer with the
// moments timeline and attached-artifacts placeholder. The whole page
// is a single ScrollView so the user can scroll past the tab body
// into the moments / artifacts sections — the tabs are not pinned.

import { useLocalSearchParams } from "expo-router";
import { useCallback, useState } from "react";
import { useFocusEffect } from "expo-router";
import {
  ActivityIndicator,
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from "react-native";
// expo-file-system v55 moved the procedural API (cacheDirectory,
// downloadAsync) to the `/legacy` subpath; the top-level export is
// now the new File/Directory class-based API. The legacy surface is
// supported through the v55 line — small surgical migration only.
import * as FileSystem from "expo-file-system/legacy";
import * as Sharing from "expo-sharing";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { formatTokens, pickMeetingTitle } from "@/src/lib/meetings";
import { useTheme } from "@/src/theme/useTheme";
import { MeetingsApi, type MeetingDetail, type MeetingLlmUsage } from "@/src/wire/meetings-api";
import type { Item } from "@/src/wire/contract";

import { BrandedRefresh } from "@/src/ui/BrandedRefresh";
import { Card, IconButton, MonoLabel, Section } from "@/src/ui/components";
import { MetadataEditor } from "@/src/ui/MetadataEditor";
import {
  AttachedArtifacts,
  ChatPanel,
  ItemList,
  MomentsTimeline,
  TabBar,
  type TabDescriptor,
} from "@/src/ui/meeting-detail";

// Order matches the PWA's post-meeting view: Transcript →
// Highlights → Actions → Open Questions → Summary → Chat. Transcript
// is the default landing tab because it's the most data-dense and
// the one users scroll for after a meeting ends. Moments sits inline
// below the tabs since it reads as a different chapter rather than a
// tab body.
const TAB_ORDER = [
  "transcript",
  "assist",
  "highlights",
  "actions",
  "open_questions",
  "summary",
  "chat",
] as const;

const TAB_LABELS: Record<string, string> = {
  transcript: "TRANSCRIPT",
  assist: "ASSIST",
  highlights: "HIGHLIGHTS",
  actions: "ACTIONS",
  open_questions: "QUESTIONS",
  summary: "SUMMARY",
  chat: "CHAT",
};

export default function MeetingDetailScreen() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const t = useTheme();
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [descriptionOpen, setDescriptionOpen] = useState(false);
  const [activeTab, setActiveTab] = useState<string>("transcript");
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");

  // Rename: optimistically set the `title` tag, revert + alert on a
  // server error. Mirrors the PWA meetings modal. `commitRename` is
  // idempotent against blur+submit both firing (it always closes the
  // editor; the no-op guards short-circuit a second call).
  const beginRename = useCallback(() => {
    if (!detail) return;
    const current = pickMeetingTitle(detail);
    setTitleDraft(current === "Untitled meeting" ? "" : current);
    setEditingTitle(true);
  }, [detail]);

  const commitRename = useCallback(async () => {
    if (!editingTitle) return;
    setEditingTitle(false);
    if (!detail) return;
    const next = titleDraft.trim();
    if (!next || next === pickMeetingTitle(detail)) return;
    const api = MeetingsApi.from(serverUrl, () => auth0.getAccessToken());
    if (!api) return;
    const prev = detail;
    setDetail({ ...detail, metadata: { ...detail.metadata, title: next } });
    try {
      await api.rename(detail.id, next);
    } catch (e) {
      setDetail(prev);
      Alert.alert("Rename failed", e instanceof Error ? e.message : String(e));
    }
  }, [detail, editingTitle, titleDraft]);

  const fetchDetail = useCallback(
    async (opts: { isRefresh?: boolean } = {}) => {
      if (!opts.isRefresh) setLoading(true);
      setError(null);
      try {
        const api = MeetingsApi.from(serverUrl, () => auth0.getAccessToken());
        if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
        const d = await api.detail(id);
        setDetail(d);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (opts.isRefresh) setRefreshing(false);
        else setLoading(false);
      }
    },
    [id],
  );

  useFocusEffect(
    useCallback(() => {
      let cancelled = false;
      void (async () => {
        if (cancelled) return;
        await fetchDetail();
      })();
      return () => {
        cancelled = true;
      };
    }, [fetchDetail]),
  );

  const onRefresh = useCallback(() => {
    setRefreshing(true);
    void fetchDetail({ isRefresh: true });
  }, [fetchDetail]);

  if (loading && !detail) {
    return (
      <View style={[styles.center, { backgroundColor: t.color.bg.canvas }]}>
        <ActivityIndicator color={t.color.brand.coral} />
      </View>
    );
  }
  if (error && !detail) {
    return (
      <View style={[styles.center, { backgroundColor: t.color.bg.canvas }]}>
        <Text style={[styles.errorText, { ...t.type.body, color: t.color.danger.base }]}>
          {error}
        </Text>
      </View>
    );
  }
  if (!detail) return null;

  const rawTitle = pickMeetingTitle(detail);
  const title = rawTitle === "Untitled meeting" ? "UNTITLED MEETING" : rawTitle;
  const description = detail.description?.trim();
  const showDescription = description && description !== rawTitle;

  // Per-tab item payloads. `items_by_mode` is the canonical source
  // for mode-segmented items; transcript can come from either the
  // top-level `transcript` array (always present) or
  // `items_by_mode.transcript` (sometimes also present). Prefer the
  // top-level since it's part of the stable MeetingDetail contract.
  const itemsByMode = detail.items_by_mode ?? {};
  const transcriptItems: Item[] = detail.transcript ?? [];
  const assistItems: Item[] = itemsByMode.assist ?? [];
  const highlightsItems: Item[] = itemsByMode.highlights ?? [];
  const actionsItems: Item[] = itemsByMode.actions ?? [];
  const openQuestionsItems: Item[] = itemsByMode.open_questions ?? [];
  const summaryItems: Item[] = itemsByMode.summary ?? [];
  const chatItems: Item[] = itemsByMode.chat ?? [];
  const moments = detail.moments ?? [];

  const tabs: TabDescriptor[] = TAB_ORDER.map((id) => ({
    id,
    label: TAB_LABELS[id] ?? id.toUpperCase(),
  }));

  function renderTabBody(): React.ReactNode {
    switch (activeTab) {
      case "transcript":
        return <ItemList items={transcriptItems} mode="transcript" />;
      case "assist":
        return <ItemList items={assistItems} mode="assist" />;
      case "highlights":
        return <ItemList items={highlightsItems} mode="highlights" />;
      case "actions":
        return <ItemList items={actionsItems} mode="actions" />;
      case "open_questions":
        return <ItemList items={openQuestionsItems} mode="open_questions" />;
      case "summary":
        return <ItemList items={summaryItems} mode="summary" />;
      case "chat":
        return <ChatPanel items={chatItems} />;
      default:
        return null;
    }
  }

  return (
    <ScrollView
      style={[styles.root, { backgroundColor: t.color.bg.canvas }]}
      contentContainerStyle={[
        styles.content,
        {
          paddingHorizontal: t.spacing.lg,
          paddingTop: t.spacing.lg,
          paddingBottom: t.spacing.xxl,
          gap: t.spacing.lg,
        },
      ]}
      refreshControl={<BrandedRefresh refreshing={refreshing} onRefresh={onRefresh} />}
    >
      {/* ─── Header zone (no card — typography carries the weight) ── */}
      <View>
        <View style={[styles.headerTopRow, { gap: t.spacing.md }]}>
          <View style={{ flex: 1 }}>
            {editingTitle ? (
              <TextInput
                value={titleDraft}
                onChangeText={setTitleDraft}
                autoFocus
                maxLength={200}
                returnKeyType="done"
                onSubmitEditing={() => void commitRename()}
                onBlur={() => void commitRename()}
                placeholder="Meeting title"
                placeholderTextColor={t.color.text.secondary}
                style={{
                  ...t.type.display,
                  color: t.color.text.primary,
                  fontSize: 32,
                  lineHeight: 36,
                  letterSpacing: 1.5,
                  padding: 0,
                  borderBottomWidth: 1,
                  borderBottomColor: t.color.brand.coral,
                }}
              />
            ) : (
              <Text
                style={{
                  ...t.type.display,
                  color: t.color.text.primary,
                  fontSize: 32,
                  lineHeight: 36,
                  letterSpacing: 1.5,
                }}
                numberOfLines={3}
              >
                {title}
              </Text>
            )}
            {!editingTitle && (
              <View
                style={[
                  styles.coralRule,
                  {
                    marginTop: t.spacing.sm,
                    backgroundColor: t.color.brand.coral,
                  },
                ]}
              />
            )}
          </View>
          <View style={{ marginTop: 4, flexDirection: "row", gap: t.spacing.sm }}>
            {!editingTitle && (
              <IconButton
                glyph="✎"
                onPress={beginRename}
                tone="neutral"
                accessibilityLabel="Rename meeting"
              />
            )}
            <DownloadPdfButton meetingId={id} />
          </View>
        </View>

        <View style={{ marginTop: t.spacing.md }}>
          <MonoLabel>{buildTimingLine(detail.started_at, detail.ended_at)}</MonoLabel>
        </View>
      </View>

      {/* ─── Wrap-up status banner ────────────────────────────────── */}
      {/*
        Sits between header and tags so the user reads it before
        scrolling into the meeting content. Two variants: `failed`
        (red, extractor errored — actions/open_questions may be
        incomplete) and `running` (orange, still in flight). The
        `success` and null states render nothing.
      */}
      {(detail.wrap_up_status === "failed" || detail.wrap_up_status === "running") && (
        <WrapUpBanner status={detail.wrap_up_status} />
      )}

      {/* ─── Tags ──────────────────────────────────────────────────── */}
      {/*
        MetadataEditor renders its own "TAGS" header, so we don't wrap
        it in <Section> (would double up the heading).
      */}
      <View>
        <MetadataEditor />
      </View>

      {/* ─── Description (collapsible) ─────────────────────────────── */}
      {showDescription && (
        <Card padding="md" variant="flat">
          <DescriptionBlock
            text={description!}
            open={descriptionOpen}
            setOpen={setDescriptionOpen}
          />
        </Card>
      )}

      {/* ─── LLM Usage ─────────────────────────────────────────────── */}
      {detail.llm_usage && detail.llm_usage.calls > 0 && (
        <Section title="LLM Usage">
          <Card padding="md" variant="flat">
            <LlmUsageBlock usage={detail.llm_usage} />
          </Card>
        </Section>
      )}

      {/* ─── Tab strip + body ──────────────────────────────────────── */}
      <View style={{ gap: t.spacing.md }}>
        <View style={{ marginHorizontal: -t.spacing.lg }}>
          <TabBar tabs={tabs} activeId={activeTab} onSelect={setActiveTab} />
        </View>
        <View style={{ marginHorizontal: -t.spacing.lg, minHeight: 200 }}>{renderTabBody()}</View>
      </View>

      {/* ─── Moments (only when present — empty state is rendered by
            MomentsTimeline itself if you'd rather always show it; the
            current call hides the section entirely when there are no
            moments, since it's optional metadata). ─────────────────── */}
      {moments.length > 0 && (
        <View style={{ marginHorizontal: -t.spacing.lg }}>
          <MomentsTimeline moments={moments} />
        </View>
      )}

      {/* ─── Attached artifacts ─────────────────────────────────────── */}
      <View style={{ marginHorizontal: -t.spacing.lg }}>
        <AttachedArtifacts meetingId={id} />
      </View>
    </ScrollView>
  );
}

/// Build the editorial timing line shown under the title.
///
/// Format: `STARTED FRI · MAY 15, 2026 · 19:46 → ENDED 19:47 · 1 MIN`
/// (uppercase, mono — rendered by MonoLabel at the call site, so the
/// strings here are mixed-case for readability and uppercased downstream).
function buildTimingLine(startedAt: string, endedAt: string | null): string {
  const start = new Date(startedAt);
  const startDay = start.toLocaleString(undefined, { weekday: "short" });
  const startDate = start.toLocaleString(undefined, {
    month: "long",
    day: "numeric",
    year: "numeric",
  });
  const startTime = start.toLocaleString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });

  if (!endedAt) {
    return `Started ${startDay} · ${startDate} · ${startTime} → in progress`;
  }
  const end = new Date(endedAt);
  const endTime = end.toLocaleString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  const durMs = end.getTime() - start.getTime();
  const dur = formatDurationShort(durMs);
  return `Started ${startDay} · ${startDate} · ${startTime} → Ended ${endTime} · ${dur}`;
}

function formatDurationShort(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds} sec`;
  const mins = Math.floor(seconds / 60);
  if (mins < 60) return `${mins} min`;
  const hours = Math.floor(mins / 60);
  const remMin = mins % 60;
  return remMin > 0 ? `${hours} hr ${remMin} min` : `${hours} hr`;
}

/// Download the PDF export, then hand off to the native share sheet
/// so the user can save it to Files / iCloud / Drive / etc. The
/// server's `/meetings/:id/export.pdf` route requires the Auth0
/// bearer token, so we use FileSystem.downloadAsync with explicit
/// headers (a vanilla URL handoff to Sharing wouldn't carry auth).
function DownloadPdfButton({ meetingId }: { meetingId: string }) {
  const [state, setState] = useState<"idle" | "working" | "failed">("idle");
  const label = state === "working" ? "GENERATING…" : state === "failed" ? "FAILED" : "PDF";

  async function onPress() {
    if (state === "working") return;
    setState("working");
    try {
      const token = await auth0.getAccessToken();
      const restBase = serverUrl.replace(/^ws/, "http").replace(/\/$/, "");
      const url = `${restBase}/meetings/${encodeURIComponent(meetingId)}/export.pdf`;
      const target = `${FileSystem.cacheDirectory}meeting-${meetingId}.pdf`;
      const result = await FileSystem.downloadAsync(url, target, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (result.status !== 200) {
        throw new Error(`server returned ${result.status}`);
      }
      if (await Sharing.isAvailableAsync()) {
        await Sharing.shareAsync(target, {
          mimeType: "application/pdf",
          dialogTitle: "Meeting export",
          UTI: "com.adobe.pdf",
        });
      }
      setState("idle");
    } catch (e) {
      console.warn("[meeting/detail] pdf download failed:", e);
      setState("failed");
      setTimeout(() => setState("idle"), 2500);
    }
  }

  return (
    <IconButton
      glyph="↓"
      label={label}
      onPress={onPress}
      tone="action"
      filled
      disabled={state === "working"}
      accessibilityLabel="Download meeting PDF"
    />
  );
}

/// Wrap-up extractor status banner. Surfaces the two states the
/// user needs feedback on (`running` and `failed`) — success and
/// legacy (null) render nothing. Mirrors the PWA banner palette:
/// red/coral for failed, orange/amber for running.
function WrapUpBanner({ status }: { status: "running" | "failed" }) {
  const t = useTheme();
  const isFailed = status === "failed";
  const bg = isFailed ? "rgba(220, 80, 60, 0.12)" : "rgba(230, 160, 30, 0.12)";
  const border = isFailed ? "rgba(220, 80, 60, 0.5)" : "rgba(230, 160, 30, 0.5)";
  const label = isFailed
    ? "Wrap-up extraction failed — actions + open questions for this meeting may be incomplete."
    : "Wrap-up extraction still running — pull to refresh to see actions + open questions when ready.";
  return (
    <View
      style={{
        padding: t.spacing.md,
        backgroundColor: bg,
        borderRadius: 6,
        borderWidth: 1,
        borderColor: border,
      }}
    >
      <Text
        style={{
          ...t.type.body,
          color: t.color.text.primary,
        }}
      >
        {label}
      </Text>
    </View>
  );
}

/// Description block with chevron disclosure. Shows a single-line
/// snippet alongside the heading when collapsed; reveals the full
/// prose in a max-height-clipped block when open. Mirrors the
/// PWA's `meetings-detail-description` pattern.
function DescriptionBlock({
  text,
  open,
  setOpen,
}: {
  text: string;
  open: boolean;
  setOpen: (v: boolean) => void;
}) {
  const t = useTheme();
  return (
    <View style={{ gap: t.spacing.sm }}>
      <Pressable
        style={[styles.descriptionHead, { gap: t.spacing.sm }]}
        onPress={() => setOpen(!open)}
      >
        <Text
          style={{
            ...t.type.mono,
            color: t.color.text.secondary,
            width: 12,
          }}
        >
          {open ? "▾" : "▸"}
        </Text>
        <MonoLabel>DESCRIPTION</MonoLabel>
        {!open && (
          <Text
            style={{
              ...t.type.bodySmall,
              color: t.color.text.secondary,
              flex: 1,
            }}
            numberOfLines={1}
          >
            {text.replace(/\s+/g, " ").trim()}
          </Text>
        )}
      </Pressable>
      {open && (
        <View
          style={[
            styles.descriptionBody,
            {
              backgroundColor: t.color.bg.tint,
              borderRadius: t.radius.md,
              padding: t.spacing.md,
            },
          ]}
        >
          <Text
            style={{
              ...t.type.bodySmall,
              color: t.color.text.primary,
            }}
          >
            {text}
          </Text>
        </View>
      )}
    </View>
  );
}

function LlmUsageBlock({ usage }: { usage: MeetingLlmUsage }) {
  // input_tokens and cached_input_tokens are disjoint buckets in
  // rig 0.36's mapping (input = fresh-billable, cached = read at
  // 0.10× rate). Same labelling we just landed on PWA + Mac.
  return (
    <View>
      <UsageRow label="calls" value={String(usage.calls)} />
      <UsageRow label="input (billable)" value={formatTokens(usage.input_tokens)} />
      <UsageRow label="cached read (0.10×)" value={formatTokens(usage.cached_input_tokens)} />
      <UsageRow label="output tokens" value={formatTokens(usage.output_tokens)} />
      {usage.model_id && <UsageRow label="model" value={usage.model_id} />}
      {usage.provider && <UsageRow label="provider" value={usage.provider} />}
    </View>
  );
}

function UsageRow({ label, value }: { label: string; value: string }) {
  const t = useTheme();
  return (
    <View style={[styles.kvRow, { paddingVertical: 3 }]}>
      <Text
        style={{
          ...t.type.mono,
          color: t.color.text.secondary,
          minWidth: 160,
          textTransform: "uppercase",
          letterSpacing: 0.4,
        }}
      >
        {label}
      </Text>
      <Text
        style={{
          ...t.type.monoMedium,
          color: t.color.text.primary,
          flex: 1,
          textAlign: "right",
        }}
      >
        {value}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  content: {},
  center: {
    flex: 1,
    justifyContent: "center",
    alignItems: "center",
    padding: 24,
  },
  errorText: {
    textAlign: "center",
  },

  // Header
  headerTopRow: {
    flexDirection: "row",
    alignItems: "flex-start",
  },
  coralRule: {
    width: 48,
    height: 2,
    borderRadius: 1,
  },

  // Description disclosure
  descriptionHead: {
    flexDirection: "row",
    alignItems: "baseline",
  },
  descriptionBody: {
    maxHeight: 280,
  },

  // Key/value rows (LLM usage)
  kvRow: {
    flexDirection: "row",
    alignItems: "center",
  },
});
