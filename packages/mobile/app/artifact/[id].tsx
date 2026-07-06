// Artifact detail screen — Phase D editorial redesign. Drilled into
// from the Artifacts tab on row tap.
//
// Layout:
//   - Bebas Neue filename + coral underline anchor the header.
//   - Meta row carries status chip, mime chip, size + created-at in
//     mono. No emoji.
//   - Summary / long-summary live inside <Section> + <Card>.
//   - Sticky bottom action bar: armed-delete only.
//
// Attach-to-meeting deliberately omitted here: artifacts can only be
// attached during compose (before a meeting starts) or mid-meeting
// from the live meeting screen. After a meeting ends, attachment is
// frozen.
//
// Share/download is intentionally omitted: the server does not expose
// a `/artifacts/:id/download` endpoint. When that lands, drop a
// `<DownloadShareButton>` (cf. meeting/[id].tsx's `DownloadPdfButton`)
// into the action bar.

import { router, useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, Alert, ScrollView, StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { useTheme } from "@/src/theme/useTheme";
import { Card, Chip, IconButton, MonoLabel, Section } from "@/src/ui/components";
import { StatusChip, formatBytes } from "@/src/ui/artifacts";
import { ArtifactsApi, type Artifact } from "@/src/wire/artifacts-api";

/// Filenames in the artifact metadata can arrive percent-encoded
/// (e.g. `A%20Memory%20That%20Follows%20Me.pdf`) when the source
/// upload preserved a URL-encoded name. Decode for display, but fall
/// back to the raw name if decoding throws (malformed sequences).
function decodeName(raw: string): string {
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}

export default function ArtifactDetailScreen() {
  const t = useTheme();
  const { id } = useLocalSearchParams<{ id: string }>();
  const [artifact, setArtifact] = useState<Artifact | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useFocusEffect(
    useCallback(() => {
      let cancelled = false;
      void (async () => {
        setLoading(true);
        setError(null);
        try {
          const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
          if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
          const a = await api.get(id);
          if (!cancelled) setArtifact(a);
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

  async function onDelete() {
    try {
      const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
      if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
      await api.delete(id);
      // Back to the list — the focus effect there will re-fetch and
      // the row will be gone.
      router.back();
    } catch (e) {
      Alert.alert("Delete failed", e instanceof Error ? e.message : String(e));
    }
  }

  if (loading && !artifact) {
    return (
      <View style={[styles.center, { backgroundColor: t.color.bg.canvas }]}>
        <ActivityIndicator color={t.color.brand.coral} />
      </View>
    );
  }
  if (error) {
    return (
      <View style={[styles.center, { backgroundColor: t.color.bg.canvas }]}>
        <Text style={{ ...t.type.body, color: t.color.danger.base, textAlign: "center" }}>
          {error}
        </Text>
      </View>
    );
  }
  if (!artifact) return null;

  const hasLong = !!artifact.long_summary && artifact.long_summary.trim().length > 0;

  const displayName = decodeName(artifact.name);
  // Long filenames need to wrap — switch to a smaller display size
  // for anything past ~24 characters so the title doesn't overflow
  // a phone screen. Filenames mix case + extension so we keep the
  // sansSemi family rather than forcing Bebas Neue uppercase.
  const isLongName = displayName.length > 24;

  return (
    <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
      <ScrollView
        contentContainerStyle={{
          paddingHorizontal: t.spacing.lg,
          paddingTop: t.spacing.lg,
          paddingBottom: t.spacing.xxxl * 2,
          gap: t.spacing.md,
        }}
      >
        <View>
          <Text
            style={
              isLongName
                ? {
                    ...t.type.headline,
                    color: t.color.text.primary,
                  }
                : {
                    fontFamily: t.font.display,
                    fontSize: 32,
                    letterSpacing: 1.5,
                    lineHeight: 36,
                    color: t.color.text.primary,
                  }
            }
          >
            {displayName}
          </Text>
          <View
            style={{
              width: 32,
              height: 2,
              backgroundColor: t.color.brand.coral,
              marginTop: t.spacing.sm,
            }}
          />
        </View>

        <View style={[styles.metaRow, { gap: t.spacing.sm }]}>
          <StatusChip status={artifact.summary_status} />
          <Chip label={artifact.mime_type} tone="neutral" size="sm" />
          <MonoLabel tone="muted">{formatBytes(artifact.size_bytes)}</MonoLabel>
        </View>

        <MonoLabel tone="secondary">{formatDateLong(artifact.created_at)}</MonoLabel>

        {artifact.short_summary ? (
          <Section title="Summary">
            <Card>
              <Text style={{ ...t.type.body, color: t.color.text.primary }}>
                {artifact.short_summary}
              </Text>
            </Card>
          </Section>
        ) : null}

        {hasLong && (
          <Section title="Detailed summary">
            <Card>
              <Text style={{ ...t.type.body, color: t.color.text.primary }}>
                {artifact.long_summary}
              </Text>
            </Card>
          </Section>
        )}

        {artifact.summary_status === "pending" && (
          <Card variant="flat">
            <Text
              style={{
                ...t.type.bodySmall,
                color: t.color.status.pending,
                fontStyle: "italic",
              }}
            >
              ── summary is still being generated
            </Text>
          </Card>
        )}
        {artifact.summary_status === "failed" && (
          <Card variant="flat">
            <Text
              style={{
                ...t.type.bodySmall,
                color: t.color.danger.base,
                fontStyle: "italic",
              }}
            >
              ── summary generation failed on the server
            </Text>
          </Card>
        )}
      </ScrollView>

      <ActionBar onDelete={onDelete} />
    </SafeAreaView>
  );
}

/// Two-click armed delete. Pinned to the bottom of the screen above
/// the safe area so it's reachable on long pages. Attach-to-meeting
/// intentionally NOT here — see the file header.
function ActionBar({ onDelete }: { onDelete: () => void | Promise<void> }) {
  const t = useTheme();
  const [armed, setArmed] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  function handleDelete() {
    if (!armed) {
      setArmed(true);
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setArmed(false), 3000);
      return;
    }
    if (timerRef.current) clearTimeout(timerRef.current);
    setArmed(false);
    void onDelete();
  }

  return (
    <View
      style={{
        flexDirection: "row",
        alignItems: "center",
        paddingHorizontal: t.spacing.lg,
        paddingVertical: t.spacing.md,
        borderTopWidth: StyleSheet.hairlineWidth,
        borderTopColor: t.color.border.soft,
        backgroundColor: t.color.bg.elevated,
        gap: t.spacing.sm,
      }}
    >
      <IconButton
        glyph="×"
        label={armed ? "CONFIRM?" : "DELETE"}
        tone="danger"
        filled
        onPress={handleDelete}
      />
    </View>
  );
}

function formatDateLong(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  center: {
    flex: 1,
    justifyContent: "center",
    alignItems: "center",
    padding: 24,
  },
  metaRow: {
    flexDirection: "row",
    alignItems: "center",
    flexWrap: "wrap",
  },
});
