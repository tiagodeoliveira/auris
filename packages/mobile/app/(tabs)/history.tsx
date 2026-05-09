// History tab — Phase 5. Read-only browse of past meetings.
// Bucketed by relative date (today / yesterday / this week / older);
// tapping a row pushes /meeting/[id] for the full detail.

import { router, type Href, useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import {
  ActivityIndicator,
  FlatList,
  Pressable,
  RefreshControl,
  SafeAreaView,
  StyleSheet,
  Text,
  View,
} from "react-native";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import {
  formatDateShort,
  formatDuration,
  pickMeetingTitle,
  relativeBucket,
} from "@/src/lib/meetings";
import { MeetingsApi, type MeetingSummary } from "@/src/wire/meetings-api";

type Row = { kind: "header"; bucket: string } | { kind: "meeting"; meeting: MeetingSummary };

export default function HistoryScreen() {
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (mode: "initial" | "refresh") => {
    if (mode === "initial") setLoading(true);
    else setRefreshing(true);
    setError(null);
    try {
      const api = MeetingsApi.from(serverUrl, () => auth0.getAccessToken());
      if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
      const list = await api.list();
      setMeetings(list);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  // Refetch every time the tab comes into focus — simplest "freshness"
  // model for a single-server, single-user setup. Heavier: subscribe
  // to ws meeting_state_changed events and update locally; not worth
  // the wiring cost for a read-only browse view.
  useFocusEffect(
    useCallback(() => {
      void load("initial");
    }, [load]),
  );

  // Bucket meetings into header + row pairs the FlatList can render
  // as a single flat data array (cheaper than SectionList for our
  // shape, and the bucket boundaries follow server order so no
  // re-sort needed).
  const rows: Row[] = (() => {
    const out: Row[] = [];
    let lastBucket: string | null = null;
    for (const m of meetings) {
      const bucket = relativeBucket(m.started_at);
      if (bucket !== lastBucket) {
        out.push({ kind: "header", bucket });
        lastBucket = bucket;
      }
      out.push({ kind: "meeting", meeting: m });
    }
    return out;
  })();

  if (loading && meetings.length === 0) {
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
        data={rows}
        keyExtractor={(r, i) => (r.kind === "header" ? `h-${r.bucket}-${i}` : r.meeting.id)}
        renderItem={({ item }) =>
          item.kind === "header" ? (
            <Text style={styles.bucketHeader}>{item.bucket}</Text>
          ) : (
            <MeetingRow meeting={item.meeting} />
          )
        }
        refreshControl={
          <RefreshControl refreshing={refreshing} onRefresh={() => void load("refresh")} />
        }
        ListEmptyComponent={
          <View style={styles.empty}>
            {error ? (
              <Text style={styles.errorText}>{error}</Text>
            ) : (
              <Text style={styles.emptyText}>No meetings yet.</Text>
            )}
          </View>
        }
        contentContainerStyle={meetings.length === 0 ? styles.emptyContent : undefined}
      />
    </SafeAreaView>
  );
}

function MeetingRow({ meeting }: { meeting: MeetingSummary }) {
  const title = pickMeetingTitle(meeting);
  const sub = `${formatDateShort(meeting.started_at)} · ${formatDuration(meeting.started_at, meeting.ended_at)}`;
  return (
    <Pressable style={styles.row} onPress={() => router.push(`/meeting/${meeting.id}` as Href)}>
      <Text style={styles.rowTitle} numberOfLines={1}>
        {title}
      </Text>
      <Text style={styles.rowSub}>{sub}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  center: { flex: 1, justifyContent: "center", alignItems: "center" },

  bucketHeader: {
    fontSize: 11,
    fontWeight: "600",
    letterSpacing: 0.5,
    textTransform: "uppercase",
    color: "#647386",
    paddingHorizontal: 16,
    paddingTop: 16,
    paddingBottom: 4,
  },

  row: {
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: "#eef2f7",
    gap: 2,
  },
  rowTitle: { fontSize: 15, color: "#17212e" },
  rowSub: { fontSize: 12, color: "#647386" },

  empty: { padding: 24, alignItems: "center" },
  emptyContent: { flex: 1, justifyContent: "center" },
  emptyText: { color: "#96a3b4", fontSize: 14 },
  errorText: { color: "#e5484d", fontSize: 14, textAlign: "center" },
});
