// Artifacts tab — Phase D editorial redesign. Library of uploaded
// documents that can be attached to meetings. Each row is now a
// self-contained card (status chip + filename + mime + summary) with
// inline expand for `long_summary` and a two-click armed delete.
//
// Polling: while any artifact is in `pending` summary state the
// screen refetches every 2s so the chip flips to DONE without the
// user pulling-to-refresh. The interval is torn down when the tab
// loses focus or once nothing is pending.

import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, Alert, FlatList, StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { useTheme } from "@/src/theme/useTheme";
import { AurisMark } from "@/src/ui/AurisMark";
import { BrandedRefresh } from "@/src/ui/BrandedRefresh";
import { EmptyState, MonoLabel } from "@/src/ui/components";
import { ArtifactRow, UploadButton } from "@/src/ui/artifacts";
import { ArtifactsApi, type Artifact } from "@/src/wire/artifacts-api";

const POLL_MS = 2000;

export default function ArtifactsScreen() {
  const t = useTheme();
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Focus state drives polling. Use a ref so the interval callback
  // can read it without re-creating the interval on every state flip.
  const focusedRef = useRef(false);
  const pollTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const fetchList = useCallback(async (): Promise<Artifact[]> => {
    const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
    if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
    return api.list();
  }, []);

  const load = useCallback(
    async (mode: "initial" | "refresh" | "poll") => {
      if (mode === "initial") setLoading(true);
      else if (mode === "refresh") setRefreshing(true);
      if (mode !== "poll") setError(null);
      try {
        const list = await fetchList();
        setArtifacts(list);
      } catch (e) {
        // Polling errors are silent — don't clobber the visible list
        // with transient network blips. Initial/refresh surface them.
        if (mode !== "poll") {
          setError(e instanceof Error ? e.message : String(e));
        }
      } finally {
        if (mode === "initial") setLoading(false);
        else if (mode === "refresh") setRefreshing(false);
      }
    },
    [fetchList],
  );

  // Keep / tear down the poll timer based on whether any artifact is
  // still summarizing. Re-runs on every artifacts change, but the
  // timer reference is reused (clear + set only when needed).
  useEffect(() => {
    const hasPending = artifacts.some((a) => a.summary_status === "pending");
    if (hasPending && focusedRef.current) {
      if (pollTimerRef.current) return;
      pollTimerRef.current = setInterval(() => {
        void load("poll");
      }, POLL_MS);
    } else if (pollTimerRef.current) {
      clearInterval(pollTimerRef.current);
      pollTimerRef.current = null;
    }
    return () => {
      // Only clear here on unmount — let the effect itself handle the
      // common pending-flips-to-done transition.
    };
  }, [artifacts, load]);

  useFocusEffect(
    useCallback(() => {
      focusedRef.current = true;
      void load("initial");
      return () => {
        focusedRef.current = false;
        if (pollTimerRef.current) {
          clearInterval(pollTimerRef.current);
          pollTimerRef.current = null;
        }
      };
    }, [load]),
  );

  const handleUploaded = useCallback((artifact: Artifact) => {
    // Optimistic insert: newest first matches server ordering. The
    // poll loop will reconcile when the server-side summary finishes.
    setArtifacts((prev) => [artifact, ...prev.filter((a) => a.id !== artifact.id)]);
  }, []);

  const handleDeleted = useCallback(
    async (id: string) => {
      const prev = artifacts;
      setArtifacts((cur) => cur.filter((a) => a.id !== id));
      try {
        const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
        if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
        await api.delete(id);
      } catch (e) {
        // Restore — the row reappears so the user knows the action
        // didn't take. Alert spells out the failure.
        setArtifacts(prev);
        Alert.alert("Delete failed", e instanceof Error ? e.message : String(e));
      }
    },
    [artifacts],
  );

  if (loading && artifacts.length === 0) {
    return (
      <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
        <View style={{ paddingHorizontal: t.spacing.lg }}>
          <Header count={null} onUploaded={handleUploaded} />
        </View>
        <View style={styles.center}>
          <ActivityIndicator color={t.color.brand.coral} />
        </View>
      </SafeAreaView>
    );
  }

  if (artifacts.length === 0) {
    return (
      <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
        <View style={{ flex: 1, paddingHorizontal: t.spacing.lg }}>
          <Header count={0} onUploaded={handleUploaded} />
          {error ? (
            <View style={{ padding: t.spacing.xl, alignItems: "center" }}>
              <Text
                style={{
                  ...t.type.body,
                  color: t.color.danger.base,
                  textAlign: "center",
                }}
              >
                {error}
              </Text>
            </View>
          ) : (
            <View style={{ alignItems: "center" }}>
              <AurisMark size={56} variant="coral" animate="breathe" />
              <EmptyState
                title="No artifacts yet"
                body="── tap UPLOAD to give meetings extra context"
                action={<UploadButton onUploaded={handleUploaded} />}
              />
            </View>
          )}
        </View>
      </SafeAreaView>
    );
  }

  return (
    <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
      <FlatList
        data={artifacts}
        keyExtractor={(a) => a.id}
        renderItem={({ item }) => <ArtifactRow artifact={item} onDeleted={handleDeleted} />}
        ListHeaderComponent={<Header count={artifacts.length} onUploaded={handleUploaded} />}
        ItemSeparatorComponent={() => <View style={{ height: t.spacing.sm }} />}
        refreshControl={
          <BrandedRefresh refreshing={refreshing} onRefresh={() => void load("refresh")} />
        }
        contentContainerStyle={{
          paddingHorizontal: t.spacing.lg,
          paddingBottom: t.spacing.xxxl,
        }}
      />
    </SafeAreaView>
  );
}

function Header({
  count,
  onUploaded,
}: {
  count: number | null;
  onUploaded: (a: Artifact) => void;
}) {
  const t = useTheme();
  const caption =
    count === null
      ? null
      : count === 0
        ? null
        : `${count} artifact${count === 1 ? "" : "s"} in the library`;
  return (
    <View
      style={{
        paddingTop: t.spacing.xl,
        paddingBottom: t.spacing.lg,
        flexDirection: "row",
        alignItems: "flex-start",
        justifyContent: "space-between",
        gap: t.spacing.md,
      }}
    >
      <View style={{ flex: 1 }}>
        <Text
          style={{
            fontFamily: t.font.display,
            fontSize: 40,
            letterSpacing: 2,
            lineHeight: 44,
            color: t.color.text.primary,
          }}
        >
          LIBRARY
        </Text>
        <View
          style={{
            width: 48,
            height: 2,
            backgroundColor: t.color.brand.coral,
            marginTop: t.spacing.sm,
          }}
        />
        {caption && (
          <View style={{ marginTop: t.spacing.md }}>
            <MonoLabel tone="muted">{caption}</MonoLabel>
          </View>
        )}
      </View>
      <View style={{ paddingTop: t.spacing.sm }}>
        <UploadButton onUploaded={onUploaded} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  center: { flex: 1, justifyContent: "center", alignItems: "center" },
  emptyContent: { flexGrow: 1 },
});
