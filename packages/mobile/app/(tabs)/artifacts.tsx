// Artifacts tab — Phase 5. Read-only list of uploaded artifacts.
// Upload paths (5.7-5.9) and live attach (5.10-5.11) land in a
// later iteration alongside the camera + document picker deps.

import { useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import {
  ActivityIndicator,
  FlatList,
  RefreshControl,
  SafeAreaView,
  StyleSheet,
  Text,
  View,
} from "react-native";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { ArtifactsApi, type Artifact } from "@/src/wire/artifacts-api";

export default function ArtifactsScreen() {
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (mode: "initial" | "refresh") => {
    if (mode === "initial") setLoading(true);
    else setRefreshing(true);
    setError(null);
    try {
      const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
      if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
      const list = await api.list();
      setArtifacts(list);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useFocusEffect(
    useCallback(() => {
      void load("initial");
    }, [load]),
  );

  if (loading && artifacts.length === 0) {
    return (
      <SafeAreaView style={styles.root}>
        <View style={styles.center}>
          <ActivityIndicator />
        </View>
      </SafeAreaView>
    );
  }

  return (
    <SafeAreaView style={styles.root}>
      <FlatList
        data={artifacts}
        keyExtractor={(a) => a.id}
        renderItem={({ item }) => <ArtifactRow artifact={item} />}
        refreshControl={
          <RefreshControl refreshing={refreshing} onRefresh={() => void load("refresh")} />
        }
        ListEmptyComponent={
          <View style={styles.empty}>
            {error ? (
              <Text style={styles.errorText}>{error}</Text>
            ) : (
              <Text style={styles.emptyText}>
                No artifacts yet. Upload from the PWA or Mac for now — mobile upload UI lands in a
                future iteration.
              </Text>
            )}
          </View>
        }
        contentContainerStyle={artifacts.length === 0 ? styles.emptyContent : undefined}
      />
    </SafeAreaView>
  );
}

function ArtifactRow({ artifact }: { artifact: Artifact }) {
  const summary = artifact.short_summary ?? statusFallback(artifact.summary_status);
  return (
    <View style={styles.row}>
      <Text style={styles.rowTitle} numberOfLines={1}>
        {artifact.name}
      </Text>
      <Text style={styles.rowMime}>{artifact.mime_type}</Text>
      <Text style={styles.rowSummary} numberOfLines={2}>
        {summary}
      </Text>
    </View>
  );
}

function statusFallback(status: string): string {
  switch (status) {
    case "pending":
      return "(summary pending)";
    case "failed":
      return "(summary failed)";
    default:
      return "";
  }
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  center: { flex: 1, justifyContent: "center", alignItems: "center" },

  row: {
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: "#eef2f7",
    gap: 4,
  },
  rowTitle: { fontSize: 15, color: "#17212e" },
  rowMime: { fontSize: 11, color: "#647386", fontFamily: "Menlo" },
  rowSummary: { fontSize: 13, color: "#647386", lineHeight: 18 },

  empty: { padding: 24, alignItems: "center", gap: 8 },
  emptyContent: { flex: 1, justifyContent: "center" },
  emptyText: { color: "#96a3b4", fontSize: 14, textAlign: "center", lineHeight: 20 },
  errorText: { color: "#e5484d", fontSize: 14, textAlign: "center" },
});
