// History tab — Phase D editorial redesign. Read-only browse of past
// meetings, bucketed by relative date (today / yesterday / this week
// / older). Tapping a row pushes /meeting/[id] for the full detail.
//
// Editorial treatment:
//   - Bebas Neue display title + coral underline anchor the page so
//     the screen reads as a "notebook" rather than a phone list.
//   - Each meeting is a <Card>-style row with breathing room.
//   - Bucket separators are MonoLabel + short coral hairline.
//   - The empty state pulls the brand mark in for warmth.
//
// The native tab header is suppressed here — the in-screen header
// owns the "what page is this" affordance.

import * as Haptics from "expo-haptics";
import { router, type Href, useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import {
  ActivityIndicator,
  Alert,
  Pressable,
  SectionList,
  StyleSheet,
  Text,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import {
  formatDateShort,
  formatDuration,
  pickMeetingTitle,
  relativeBucket,
} from "@/src/lib/meetings";
import { useTheme } from "@/src/theme/useTheme";
import { AurisMark } from "@/src/ui/AurisMark";
import { BrandedRefresh } from "@/src/ui/BrandedRefresh";
import { EmptyState, MonoLabel } from "@/src/ui/components";
import { MeetingsApi, type MeetingSummary } from "@/src/wire/meetings-api";

interface Section {
  bucket: string;
  data: MeetingSummary[];
}

export default function HistoryScreen() {
  const t = useTheme();
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Optimistic delete with revert-on-failure. Mirrors the Mac client
  // (see SettingsView.swift:412 deleteMeeting). Snappy local removal,
  // server call in the background. If the server rejects, re-insert
  // the row at its original index and surface an Alert.
  const handleDelete = useCallback(
    async (id: string) => {
      const idx = meetings.findIndex((m) => m.id === id);
      if (idx < 0) return;
      const removed = meetings[idx];
      setMeetings((prev) => prev.filter((m) => m.id !== id));
      try {
        const api = MeetingsApi.from(serverUrl, () => auth0.getAccessToken());
        if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
        await api.delete(id);
      } catch (e) {
        setMeetings((prev) => {
          const next = prev.slice();
          next.splice(idx, 0, removed);
          return next;
        });
        Alert.alert("Couldn't delete meeting", e instanceof Error ? e.message : String(e));
      }
    },
    [meetings],
  );

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

  // Bucket meetings into SectionList sections so the bucket headers
  // stick to the top of the viewport as the user scrolls between
  // TODAY / YESTERDAY / THIS WEEK / OLDER. `meetings` is already
  // sorted by the server (most-recent-first), so iterating once and
  // appending into the trailing section preserves order.
  const sections: Section[] = (() => {
    const out: Section[] = [];
    let current: Section | null = null;
    for (const m of meetings) {
      const bucket = relativeBucket(m.started_at);
      if (!current || current.bucket !== bucket) {
        current = { bucket, data: [] };
        out.push(current);
      }
      current.data.push(m);
    }
    return out;
  })();

  if (loading && meetings.length === 0) {
    return (
      <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
        <View style={{ paddingHorizontal: t.spacing.lg }}>
          <Header count={null} />
        </View>
        <View style={styles.center}>
          <ActivityIndicator color={t.color.brand.coral} />
        </View>
      </SafeAreaView>
    );
  }

  if (meetings.length === 0) {
    return (
      <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
        <View style={{ flex: 1, paddingHorizontal: t.spacing.lg }}>
          <Header count={0} />
          <View style={styles.emptyWrap}>
            {error ? (
              <Text style={[styles.errorText, { color: t.color.danger.base }]}>{error}</Text>
            ) : (
              <View style={{ alignItems: "center" }}>
                <AurisMark size={56} variant="coral" animate="breathe" />
                <EmptyState title="No meetings yet" body="── tap Compose to capture one." />
              </View>
            )}
          </View>
        </View>
      </SafeAreaView>
    );
  }

  return (
    <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
      <SectionList
        sections={sections}
        keyExtractor={(m) => m.id}
        renderItem={({ item }) => <MeetingRow meeting={item} onDelete={handleDelete} />}
        renderSectionHeader={({ section }) => <BucketHeader label={section.bucket} />}
        ListHeaderComponent={<Header count={meetings.length} />}
        ItemSeparatorComponent={() => <View style={{ height: t.spacing.sm }} />}
        stickySectionHeadersEnabled
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

function Header({ count }: { count: number | null }) {
  const t = useTheme();
  const caption =
    count === null
      ? null
      : count === 0
        ? null
        : `${count} meeting${count === 1 ? "" : "s"} · most recent first`;
  return (
    <View
      style={{
        paddingTop: t.spacing.xl,
        paddingBottom: t.spacing.lg,
      }}
    >
      <Text
        style={{
          fontFamily: t.font.display,
          fontSize: 40,
          letterSpacing: 2,
          lineHeight: 44,
          color: t.color.text.primary,
        }}
      >
        MEETINGS
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
  );
}

function BucketHeader({ label }: { label: string }) {
  const t = useTheme();
  // Background painted explicitly: when SectionList sticks this
  // header to the viewport top, rows scroll BENEATH it. Without a
  // solid background the row text would bleed through.
  return (
    <View
      style={{
        paddingTop: t.spacing.lg,
        paddingBottom: t.spacing.sm,
        backgroundColor: t.color.bg.canvas,
      }}
    >
      <MonoLabel tone="secondary">{label}</MonoLabel>
      <View
        style={{
          width: 24,
          height: 1,
          backgroundColor: t.color.brand.coral,
          marginTop: t.spacing.xs,
        }}
      />
    </View>
  );
}

function MeetingRow({
  meeting,
  onDelete,
}: {
  meeting: MeetingSummary;
  onDelete: (id: string) => void;
}) {
  const t = useTheme();
  const title = pickMeetingTitle(meeting);
  const sub = `${formatDateShort(meeting.started_at)} · ${formatDuration(meeting.started_at, meeting.ended_at)}`;
  const ringColor = t.scheme === "dark" ? t.color.border.hairline : "transparent";
  // Long-press is the touch equivalent of Mac's `.contextMenu` on the
  // row. Haptic feedback fires immediately so the user knows the
  // long-press registered; the Alert is the destructive confirm step.
  const handleLongPress = () => {
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
    Alert.alert("Delete meeting?", "This can't be undone.", [
      { text: "Cancel", style: "cancel" },
      { text: "Delete", style: "destructive", onPress: () => onDelete(meeting.id) },
    ]);
  };
  return (
    <Pressable
      onPress={() => router.push(`/meeting/${meeting.id}` as Href)}
      onLongPress={handleLongPress}
      delayLongPress={400}
      style={({ pressed }) => [
        {
          backgroundColor: t.color.bg.elevated,
          borderRadius: t.radius.lg,
          paddingVertical: t.spacing.md,
          paddingHorizontal: t.spacing.lg,
          gap: t.spacing.xs,
          borderWidth: t.scheme === "dark" ? 1 : 0,
          borderColor: ringColor,
          ...(t.scheme === "light" ? t.shadow.card : null),
        },
        pressed && { opacity: 0.92 },
      ]}
    >
      <Text
        style={{
          ...t.type.body,
          fontFamily: t.font.sansSemi,
          color: t.color.text.primary,
        }}
        numberOfLines={1}
      >
        {title}
      </Text>
      <Text
        style={{
          ...t.type.mono,
          color: t.color.text.secondary,
        }}
      >
        {sub}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  center: { flex: 1, justifyContent: "center", alignItems: "center" },
  emptyWrap: { padding: 24, alignItems: "center" },
  emptyContent: { flexGrow: 1 },
  errorText: { fontSize: 14, textAlign: "center" },
});
