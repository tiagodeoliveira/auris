// Past-meeting detail. Read-only fetch from /meetings/:id; rendered
// as a normal stack screen (NOT the live-meeting fullscreen modal —
// that's app/meeting.tsx with no [id] segment, intentionally).
//
// Phase 5 lays out everything except moments (5.4 deferred — moment
// images need auth-aware blob fetch + expo-file-system to bridge to
// expo-image's URI prop, sized lift for a separate iteration).

import { useLocalSearchParams } from "expo-router";
import { useCallback, useState } from "react";
import { useFocusEffect } from "expo-router";
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import * as FileSystem from "expo-file-system";
import * as Sharing from "expo-sharing";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { formatDateLong, formatTokens, pickMeetingTitle } from "@/src/lib/meetings";
import { MeetingsApi, type MeetingDetail, type MeetingLlmUsage } from "@/src/wire/meetings-api";
import type { Item } from "@/src/wire/contract";

export default function MeetingDetailScreen() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [descriptionOpen, setDescriptionOpen] = useState(false);

  useFocusEffect(
    useCallback(() => {
      let cancelled = false;
      void (async () => {
        setLoading(true);
        setError(null);
        try {
          const api = MeetingsApi.from(serverUrl, () => auth0.getAccessToken());
          if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
          const d = await api.detail(id);
          if (!cancelled) setDetail(d);
        } catch (e) {
          if (!cancelled) setError(e instanceof Error ? e.message : String(e));
        } finally {
          if (!cancelled) setLoading(false);
        }
      })();
      return () => {
        cancelled = true;
      };
    }, [id]),
  );

  if (loading && !detail) {
    return (
      <View style={styles.center}>
        <ActivityIndicator />
      </View>
    );
  }
  if (error) {
    return (
      <View style={styles.center}>
        <Text style={styles.errorText}>{error}</Text>
      </View>
    );
  }
  if (!detail) return null;

  const title = pickMeetingTitle(detail);
  const description = detail.description?.trim();
  // Don't show the description block twice — if pickMeetingTitle
  // already returned the description's first line as the title,
  // skip the collapsible block entirely.
  const showDescription = description && description !== title;

  return (
    <ScrollView style={styles.root} contentContainerStyle={styles.content}>
      <View style={styles.titleRow}>
        <Text style={styles.title}>{title}</Text>
        <DownloadPdfButton meetingId={id} />
      </View>

      <View style={styles.timingRow}>
        <TimingCell label="Started" value={formatDateLong(detail.started_at)} />
        {detail.ended_at ? (
          <TimingCell label="Ended" value={formatDateLong(detail.ended_at)} />
        ) : (
          <TimingCell label="Status" value="in progress" inProgress />
        )}
      </View>

      {showDescription && (
        <DescriptionBlock text={description!} open={descriptionOpen} setOpen={setDescriptionOpen} />
      )}

      {Object.keys(detail.metadata).length > 0 && <MetadataBlock metadata={detail.metadata} />}

      {detail.llm_usage && detail.llm_usage.calls > 0 && <LlmUsageBlock usage={detail.llm_usage} />}

      <TranscriptBlock items={detail.transcript} />
    </ScrollView>
  );
}

/// Download the PDF export, then hand off to the native share sheet
/// so the user can save it to Files / iCloud / Drive / etc. The
/// server's `/meetings/:id/export.pdf` route requires the Auth0
/// bearer token, so we use FileSystem.downloadAsync with explicit
/// headers (a vanilla URL handoff to Sharing wouldn't carry auth).
function DownloadPdfButton({ meetingId }: { meetingId: string }) {
  const [state, setState] = useState<"idle" | "working" | "failed">("idle");
  const label = state === "working" ? "↓ generating…" : state === "failed" ? "↓ failed" : "↓ PDF";

  async function onPress() {
    if (state === "working") return;
    setState("working");
    try {
      const token = await auth0.getAccessToken();
      const restBase = serverUrl.replace(/^ws/, "http").replace(/\/$/, "");
      const url = `${restBase}/meetings/${encodeURIComponent(meetingId)}/export.pdf`;
      // expo-file-system writes to a cache path; expo-sharing then
      // points the share sheet at that file. Cache lives until the
      // OS evicts — fine for an export the user reviews once.
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
    <Pressable
      style={[styles.downloadBtn, state === "working" && styles.downloadBtnDisabled]}
      onPress={onPress}
    >
      <Text style={styles.downloadBtnText}>{label}</Text>
    </Pressable>
  );
}

function TimingCell({
  label,
  value,
  inProgress,
}: {
  label: string;
  value: string;
  inProgress?: boolean;
}) {
  return (
    <View style={styles.timingCell}>
      <Text style={styles.timingLabel}>{label}</Text>
      <Text style={[styles.timingValue, inProgress && styles.timingValueInProgress]}>{value}</Text>
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
  return (
    <View style={styles.block}>
      <Pressable style={styles.descriptionHead} onPress={() => setOpen(!open)}>
        <Text style={styles.descriptionChevron}>{open ? "▾" : "▸"}</Text>
        <Text style={styles.blockHeader}>DESCRIPTION</Text>
        {!open && (
          <Text style={styles.descriptionSnippet} numberOfLines={1}>
            {text.replace(/\s+/g, " ").trim()}
          </Text>
        )}
      </Pressable>
      {open && (
        <View style={styles.descriptionBody}>
          <Text style={styles.descriptionText}>{text}</Text>
        </View>
      )}
    </View>
  );
}

function MetadataBlock({ metadata }: { metadata: Record<string, string> }) {
  const keys = Object.keys(metadata).sort();
  return (
    <View style={styles.block}>
      <Text style={styles.blockHeader}>METADATA</Text>
      {keys.map((k) => (
        <View key={k} style={styles.kvRow}>
          <Text style={styles.kvKey}>{k}</Text>
          <Text style={styles.kvValue}>{metadata[k]}</Text>
        </View>
      ))}
    </View>
  );
}

function LlmUsageBlock({ usage }: { usage: MeetingLlmUsage }) {
  // input_tokens and cached_input_tokens are disjoint buckets in
  // rig 0.36's mapping (input = fresh-billable, cached = read at
  // 0.10× rate). Same labelling we just landed on PWA + Mac.
  return (
    <View style={styles.block}>
      <Text style={styles.blockHeader}>LLM USAGE</Text>
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
  return (
    <View style={styles.kvRow}>
      <Text style={styles.kvKey}>{label}</Text>
      <Text style={styles.kvValue}>{value}</Text>
    </View>
  );
}

function TranscriptBlock({ items }: { items: Item[] }) {
  if (items.length === 0) {
    return (
      <View style={styles.block}>
        <Text style={styles.blockHeader}>TRANSCRIPT</Text>
        <Text style={styles.transcriptEmpty}>(no transcript captured)</Text>
      </View>
    );
  }
  return (
    <View style={styles.block}>
      <Text style={styles.blockHeader}>TRANSCRIPT</Text>
      {items.map((it) => (
        <View key={it.id} style={styles.transcriptRow}>
          <Text style={styles.transcriptTime}>{formatT(it.t)}</Text>
          <View style={styles.transcriptBody}>
            <Text style={styles.transcriptText}>{it.text}</Text>
            {typeof it.meta?.speaker === "string" && (
              <Text style={styles.transcriptSpeaker}>SPEAKER · {String(it.meta.speaker)}</Text>
            )}
          </View>
        </View>
      ))}
    </View>
  );
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
  root: { flex: 1 },
  content: { padding: 16, gap: 14 },
  center: { flex: 1, justifyContent: "center", alignItems: "center", padding: 24 },
  errorText: { color: "#e5484d", fontSize: 14, textAlign: "center" },

  titleRow: { flexDirection: "row", alignItems: "center", gap: 12 },
  title: { fontSize: 22, fontWeight: "600", color: "#17212e", flex: 1 },

  downloadBtn: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    backgroundColor: "#fff",
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: "#d5dee9",
    borderRadius: 6,
  },
  downloadBtnDisabled: { opacity: 0.6 },
  downloadBtnText: {
    fontFamily: "Menlo",
    fontSize: 12,
    color: "#17212e",
    letterSpacing: 0.3,
  },

  timingRow: { flexDirection: "row", gap: 24 },
  timingCell: { flexDirection: "column", gap: 1 },
  timingLabel: {
    fontSize: 10,
    fontWeight: "600",
    letterSpacing: 0.5,
    textTransform: "uppercase",
    color: "#647386",
  },
  timingValue: { fontSize: 13, color: "#17212e" },
  timingValueInProgress: { color: "#9a6b00" },

  block: { gap: 6 },
  blockHeader: {
    fontSize: 11,
    fontWeight: "600",
    letterSpacing: 0.5,
    textTransform: "uppercase",
    color: "#647386",
    marginTop: 6,
  },

  // Description disclosure
  descriptionHead: {
    flexDirection: "row",
    alignItems: "baseline",
    gap: 8,
  },
  descriptionChevron: { fontSize: 11, color: "#647386", width: 12 },
  descriptionSnippet: { fontSize: 13, color: "#647386", flex: 1 },
  descriptionBody: {
    backgroundColor: "#f4f7fb",
    borderWidth: 1,
    borderColor: "#d5dee9",
    borderRadius: 8,
    padding: 10,
    maxHeight: 280,
  },
  descriptionText: { fontSize: 13, color: "#17212e", lineHeight: 19 },

  // Key/value rows
  kvRow: { flexDirection: "row", gap: 10 },
  kvKey: { fontSize: 13, color: "#647386", minWidth: 130, fontFamily: "Menlo" },
  kvValue: { fontSize: 13, color: "#17212e", flex: 1 },

  // Transcript
  transcriptEmpty: { color: "#96a3b4", fontSize: 13, fontStyle: "italic" },
  transcriptRow: {
    flexDirection: "row",
    paddingVertical: 6,
    gap: 12,
  },
  transcriptTime: {
    fontFamily: "Menlo",
    fontSize: 12,
    color: "#647386",
    paddingTop: 2,
  },
  transcriptBody: { flex: 1 },
  transcriptText: { fontSize: 14, color: "#17212e", lineHeight: 20 },
  transcriptSpeaker: {
    fontSize: 11,
    fontWeight: "600",
    letterSpacing: 0.5,
    color: "#647386",
    marginTop: 2,
  },
});
