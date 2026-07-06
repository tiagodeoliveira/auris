// Attached-artifacts section for the past-meeting detail screen.
//
// Renders the result of `GET /meetings/:id/artifacts` as a list of
// thin, tappable rows that route to `/artifact/[id]`. Past meetings
// are read-only: artifacts can only be attached during compose or
// live meeting (see docs/cross-surface-coordination.md), so there's
// no "+ Attach" affordance here. Rows are intentionally lighter than
// the library's `<ArtifactRow>` (no delete arm, no long-summary
// expansion) because the user is reading meeting history, not
// curating their library.

import { router, useFocusEffect, type Href } from "expo-router";
import { useCallback, useState } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { useTheme } from "@/src/theme/useTheme";
import { AurisMark } from "@/src/ui/AurisMark";
import { StatusChip, formatBytes } from "@/src/ui/artifacts";
import { Card, Chip, MonoLabel } from "@/src/ui/components";
import { ArtifactsApi, type Artifact } from "@/src/wire/artifacts-api";

interface AttachedArtifactsProps {
  meetingId: string;
}

type FetchState =
  | { kind: "loading" }
  | { kind: "loaded"; artifacts: Artifact[] }
  | { kind: "error"; message: string };

export function AttachedArtifacts({ meetingId }: AttachedArtifactsProps) {
  const t = useTheme();
  const [state, setState] = useState<FetchState>({ kind: "loading" });

  useFocusEffect(
    useCallback(() => {
      let cancelled = false;
      void (async () => {
        if (!cancelled) setState({ kind: "loading" });
        try {
          const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
          if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
          const artifacts = await api.listForMeeting(meetingId);
          if (!cancelled) setState({ kind: "loaded", artifacts });
        } catch (e) {
          if (!cancelled) {
            setState({
              kind: "error",
              message: e instanceof Error ? e.message : String(e),
            });
          }
        }
      })();
      return () => {
        cancelled = true;
      };
    }, [meetingId]),
  );

  return (
    <View
      style={{
        paddingHorizontal: t.spacing.lg,
        paddingTop: t.spacing.xl,
      }}
    >
      <View style={[styles.header, { marginBottom: t.spacing.md }]}>
        <MonoLabel>ATTACHED ARTIFACTS</MonoLabel>
        <View style={[styles.headerRule, { backgroundColor: t.color.brand.coral }]} />
      </View>

      {state.kind === "loading" && (
        <View style={{ paddingVertical: t.spacing.xl, alignItems: "center" }}>
          <ActivityIndicator color={t.color.brand.coral} />
        </View>
      )}

      {state.kind === "error" && (
        <Text
          style={{
            ...t.type.mono,
            color: t.color.danger.base,
            textAlign: "center",
            paddingVertical: t.spacing.md,
          }}
        >
          ── {state.message}
        </Text>
      )}

      {state.kind === "loaded" && state.artifacts.length === 0 && (
        <View
          style={[
            styles.body,
            {
              paddingVertical: t.spacing.xl,
              paddingHorizontal: t.spacing.lg,
              gap: t.spacing.sm,
            },
          ]}
        >
          <View style={{ marginBottom: t.spacing.xs }}>
            <AurisMark size={48} variant="mono" background={false} animate="breathe" />
          </View>
          <Text
            style={{
              ...t.type.subtitle,
              color: t.color.text.primary,
              textAlign: "center",
            }}
          >
            no attached artifacts
          </Text>
          <Text
            style={{
              ...t.type.body,
              color: t.color.text.secondary,
              textAlign: "center",
            }}
          >
            ── nothing was attached to this meeting.
          </Text>
        </View>
      )}

      {state.kind === "loaded" && state.artifacts.length > 0 && (
        <View style={{ gap: t.spacing.sm }}>
          {state.artifacts.map((a) => (
            <AttachedArtifactRow key={a.id} artifact={a} />
          ))}
        </View>
      )}
    </View>
  );
}

/// Thinner read-only row than the library `ArtifactRow`: no
/// delete-arm, no long-summary expansion, no chevron. Tapping the
/// row navigates to the artifact detail screen.
function AttachedArtifactRow({ artifact }: { artifact: Artifact }) {
  const t = useTheme();
  return (
    <Pressable
      onPress={() => router.push(`/artifact/${artifact.id}` as Href)}
      style={({ pressed }) => [{ opacity: pressed ? 0.85 : 1 }]}
    >
      <Card padding="md" variant="elevated">
        <View style={[styles.headerLine, { gap: t.spacing.sm }]}>
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
        <View style={[styles.metaLine, { gap: t.spacing.sm, marginTop: t.spacing.xs }]}>
          <Chip label={artifact.mime_type} tone="neutral" size="sm" />
          <MonoLabel tone="muted">{formatBytes(artifact.size_bytes)}</MonoLabel>
        </View>
      </Card>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  header: {
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
  },
  headerRule: {
    flex: 1,
    height: 1,
  },
  body: {
    alignItems: "center",
    justifyContent: "center",
  },
  headerLine: {
    flexDirection: "row",
    alignItems: "center",
  },
  metaLine: {
    flexDirection: "row",
    alignItems: "center",
  },
});
