// Horizontal chip strip + modal picker for staging past-meeting
// attachments before `start_meeting`. Mirrors the compose-region
// pattern from the PWA / Mac clients. Linking a past meeting is
// optional — zero or more attachments are fine.
//
// Visual treatment (Phase B):
//   - Staged-meeting chips use the neutral Chip tone (theme-aware)
//   - "+ Attach meeting" / "+ Add" is a dashed coral pill — the
//     signature affordance shared with MetadataEditor and the
//     artifact attach row
//   - Modal surfaces use theme tokens so dark mode renders cleanly

import { useEffect, useState } from "react";
import {
  ActivityIndicator,
  FlatList,
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { useTheme } from "@/src/theme/useTheme";
import { Chip } from "@/src/ui/components";
import { MeetingsApi, MeetingsApiError, type MeetingSummary } from "@/src/wire/meetings-api";

export interface MeetingAttachStripProps {
  /// Current staged set; the strip displays one chip per item.
  selected: MeetingSummary[];
  onChange: (next: MeetingSummary[]) => void;
  /// Hide this meeting id from the picker (a meeting can't attach
  /// to itself; server enforces with a CHECK constraint).
  excludeMeetingId?: string | null;
}

export function MeetingAttachStrip({
  selected,
  onChange,
  excludeMeetingId,
}: MeetingAttachStripProps) {
  const t = useTheme();
  const [pickerOpen, setPickerOpen] = useState(false);

  const dashedPill = {
    paddingHorizontal: t.spacing.md,
    paddingVertical: t.spacing.xs,
    borderRadius: t.radius.pill,
    borderWidth: 1,
    borderColor: t.color.brand.coral,
    borderStyle: "dashed" as const,
  };

  return (
    <View>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}>
        <View style={styles.row}>
          {selected.map((m) => (
            <Chip
              key={m.id}
              label={chipLabel(m)}
              tone="neutral"
              onRemove={() => onChange(selected.filter((x) => x.id !== m.id))}
            />
          ))}
          <Pressable
            style={({ pressed }) => [dashedPill, pressed && { opacity: 0.6 }]}
            onPress={() => setPickerOpen(true)}
          >
            <Text
              style={{
                ...t.type.bodySmall,
                color: t.color.brand.coral,
                fontFamily: t.font.sansSemi,
              }}
            >
              {selected.length === 0 ? "+ Attach meeting" : "+ Add"}
            </Text>
          </Pressable>
        </View>
      </ScrollView>

      <MeetingPickerModal
        visible={pickerOpen}
        onClose={() => setPickerOpen(false)}
        initiallySelectedIds={selected.map((m) => m.id)}
        excludeMeetingId={excludeMeetingId}
        onConfirm={(picked) => {
          onChange(picked);
          setPickerOpen(false);
        }}
      />
    </View>
  );
}

interface PickerProps {
  visible: boolean;
  onClose: () => void;
  initiallySelectedIds: string[];
  excludeMeetingId?: string | null;
  onConfirm: (picked: MeetingSummary[]) => void;
}

function MeetingPickerModal({
  visible,
  onClose,
  initiallySelectedIds,
  excludeMeetingId,
  onConfirm,
}: PickerProps) {
  const t = useTheme();
  const [library, setLibrary] = useState<MeetingSummary[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set(initiallySelectedIds));
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!visible) return;
    setSelected(new Set(initiallySelectedIds));
    setLoading(true);
    setError(null);
    const api = MeetingsApi.from(serverUrl, () => auth0.getAccessToken());
    if (!api) {
      setError("Server URL or token missing.");
      setLoading(false);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const all = await api.list();
        if (cancelled) return;
        setLibrary(excludeMeetingId ? all.filter((m) => m.id !== excludeMeetingId) : all);
      } catch (e) {
        if (cancelled) return;
        setError(e instanceof MeetingsApiError ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // initiallySelectedIds is a new array each render; visible toggle is the
    // real trigger we care about. Same for excludeMeetingId (stable across
    // a given modal lifetime).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible]);

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function confirm() {
    const picked = library.filter((m) => selected.has(m.id));
    onConfirm(picked);
  }

  return (
    <Modal visible={visible} animationType="slide" onRequestClose={onClose}>
      <SafeAreaView style={[styles.modalRoot, { backgroundColor: t.color.bg.canvas }]}>
        <View
          style={[
            styles.modalHeader,
            {
              borderBottomColor: t.color.border.soft,
              paddingHorizontal: t.spacing.lg,
              paddingTop: t.spacing.lg,
              paddingBottom: t.spacing.md,
            },
          ]}
        >
          <Text
            style={{
              ...t.type.title,
              color: t.color.text.primary,
            }}
          >
            Attach past meetings
          </Text>
          <Text
            style={{
              ...t.type.labelMono,
              textTransform: "uppercase",
              color: t.color.text.secondary,
            }}
          >
            {`${selected.size} selected`}
          </Text>
        </View>

        {loading ? (
          <View style={styles.center}>
            <ActivityIndicator color={t.color.brand.coral} />
          </View>
        ) : error ? (
          <View style={styles.center}>
            <Text style={{ ...t.type.subtitle, color: t.color.text.primary }}>
              Couldn't load meetings
            </Text>
            <Text
              style={{
                ...t.type.bodySmall,
                color: t.color.text.secondary,
                textAlign: "center",
              }}
            >
              {error}
            </Text>
          </View>
        ) : library.length === 0 ? (
          <View style={styles.center}>
            <Text style={{ ...t.type.body, color: t.color.text.secondary }}>
              {"— no past meetings yet"}
            </Text>
          </View>
        ) : (
          <FlatList
            data={library}
            keyExtractor={(m) => m.id}
            renderItem={({ item }) => {
              const isSel = selected.has(item.id);
              return (
                <Pressable
                  onPress={() => toggle(item.id)}
                  style={({ pressed }) => [
                    styles.pickerRow,
                    {
                      paddingHorizontal: t.spacing.lg,
                      paddingVertical: t.spacing.md,
                      borderBottomColor: t.color.border.soft,
                      gap: t.spacing.md,
                    },
                    isSel && { backgroundColor: t.color.action.primaryDim },
                    pressed && !isSel && { opacity: 0.7 },
                  ]}
                >
                  <Text
                    style={{
                      fontSize: 18,
                      color: isSel ? t.color.brand.coral : t.color.text.muted,
                      width: 22,
                    }}
                  >
                    {isSel ? "●" : "○"}
                  </Text>
                  <View style={{ flex: 1 }}>
                    <Text
                      style={{
                        ...t.type.body,
                        color: t.color.text.primary,
                        fontFamily: t.font.sansMedium,
                      }}
                      numberOfLines={1}
                    >
                      {pickLabel(item)}
                    </Text>
                    <Text
                      style={{
                        ...t.type.labelMono,
                        textTransform: "uppercase",
                        color: t.color.text.muted,
                        marginTop: 2,
                      }}
                    >
                      {formatWhen(item.started_at)}
                    </Text>
                  </View>
                </Pressable>
              );
            }}
          />
        )}

        <View
          style={[
            styles.modalFooter,
            {
              borderTopColor: t.color.border.soft,
              paddingHorizontal: t.spacing.lg,
              paddingTop: t.spacing.md,
              paddingBottom: t.spacing.xl,
              gap: t.spacing.sm,
            },
          ]}
        >
          <Pressable
            style={({ pressed }) => [
              styles.btn,
              { paddingHorizontal: t.spacing.lg, paddingVertical: t.spacing.md },
              pressed && { opacity: 0.6 },
            ]}
            onPress={onClose}
          >
            <Text style={{ ...t.type.bodyMedium, color: t.color.text.primary }}>Cancel</Text>
          </Pressable>
          <Pressable
            style={({ pressed }) => [
              styles.btn,
              {
                backgroundColor: t.color.brand.coral,
                paddingHorizontal: t.spacing.lg,
                paddingVertical: t.spacing.md,
                borderRadius: t.radius.md,
              },
              pressed && { opacity: 0.85 },
            ]}
            onPress={confirm}
          >
            <Text
              style={{
                ...t.type.bodyMedium,
                color: t.color.text.onCoral,
                fontFamily: t.font.sansSemi,
              }}
            >
              Attach
            </Text>
          </Pressable>
        </View>
      </SafeAreaView>
    </Modal>
  );
}

function chipLabel(m: MeetingSummary): string {
  const label = pickLabel(m);
  return label.length > 32 ? label.slice(0, 29) + "…" : label;
}

function pickLabel(m: MeetingSummary): string {
  const desc = (m.description ?? "").trim();
  if (desc) return desc;
  const t = m.metadata?.title;
  if (t && t.trim()) return t.trim();
  return "Meeting";
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

const styles = StyleSheet.create({
  row: { flexDirection: "row", alignItems: "center", gap: 8 },
  modalRoot: { flex: 1 },
  modalHeader: {
    flexDirection: "row",
    justifyContent: "space-between",
    alignItems: "center",
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  modalFooter: {
    flexDirection: "row",
    justifyContent: "flex-end",
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  btn: { borderRadius: 8 },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    paddingHorizontal: 32,
    gap: 6,
  },
  pickerRow: {
    flexDirection: "row",
    alignItems: "center",
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
});
