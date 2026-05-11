// Horizontal chip strip + modal picker for staging past-meeting
// attachments before `start_meeting`. Mirrors the compose-region
// pattern from the PWA / Mac clients. Linking a past meeting is
// optional — zero or more attachments are fine.

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

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
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
}: MeetingAttachStripProps): JSX.Element {
  const [pickerOpen, setPickerOpen] = useState(false);

  return (
    <View>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}>
        <View style={styles.row}>
          {selected.map((m) => (
            <View key={m.id} style={styles.chip}>
              <Text style={styles.chipText} numberOfLines={1}>
                {chipLabel(m)}
              </Text>
              <Pressable
                onPress={() => onChange(selected.filter((x) => x.id !== m.id))}
                hitSlop={8}
              >
                <Text style={styles.chipRemove}>×</Text>
              </Pressable>
            </View>
          ))}
          <Pressable style={styles.addBtn} onPress={() => setPickerOpen(true)}>
            <Text style={styles.addBtnText}>
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
}: PickerProps): JSX.Element {
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
      <View style={styles.modalRoot}>
        <View style={styles.modalHeader}>
          <Text style={styles.modalTitle}>Attach past meetings</Text>
          <Text style={styles.modalCounter}>{selected.size} selected</Text>
        </View>

        {loading ? (
          <View style={styles.center}>
            <ActivityIndicator />
          </View>
        ) : error ? (
          <View style={styles.center}>
            <Text style={styles.errorTitle}>Couldn't load meetings</Text>
            <Text style={styles.errorBody}>{error}</Text>
          </View>
        ) : library.length === 0 ? (
          <View style={styles.center}>
            <Text style={styles.empty}>No past meetings yet.</Text>
          </View>
        ) : (
          <FlatList
            data={library}
            keyExtractor={(m) => m.id}
            renderItem={({ item }) => (
              <Pressable
                onPress={() => toggle(item.id)}
                style={[
                  styles.row,
                  styles.pickerRow,
                  selected.has(item.id) && styles.pickerRowSelected,
                ]}
              >
                <Text style={styles.check}>{selected.has(item.id) ? "☑" : "☐"}</Text>
                <View style={styles.rowInfo}>
                  <Text style={styles.rowTitle} numberOfLines={1}>
                    {pickLabel(item)}
                  </Text>
                  <Text style={styles.rowSub}>{formatWhen(item.started_at)}</Text>
                </View>
              </Pressable>
            )}
          />
        )}

        <View style={styles.modalFooter}>
          <Pressable style={[styles.btn, styles.btnGhost]} onPress={onClose}>
            <Text style={styles.btnGhostText}>Cancel</Text>
          </Pressable>
          <Pressable style={[styles.btn, styles.btnPrimary]} onPress={confirm}>
            <Text style={styles.btnPrimaryText}>Attach</Text>
          </Pressable>
        </View>
      </View>
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
  row: { flexDirection: "row", alignItems: "center", gap: 6 },
  chip: {
    flexDirection: "row",
    alignItems: "center",
    paddingHorizontal: 10,
    paddingVertical: 6,
    backgroundColor: "#eef2f7",
    borderRadius: 8,
    borderWidth: 1,
    borderColor: "#d5dee9",
    gap: 6,
  },
  chipText: { fontSize: 13, color: "#17212e", maxWidth: 180 },
  chipRemove: { fontSize: 16, color: "#647386", paddingHorizontal: 4 },
  addBtn: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderWidth: 1,
    borderColor: "#2563eb",
    borderStyle: "dashed",
    borderRadius: 8,
  },
  addBtnText: { color: "#2563eb", fontSize: 13, fontWeight: "500" },
  modalRoot: { flex: 1, backgroundColor: "#fff" },
  modalHeader: {
    flexDirection: "row",
    justifyContent: "space-between",
    alignItems: "center",
    paddingHorizontal: 16,
    paddingTop: 56,
    paddingBottom: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: "#d5dee9",
  },
  modalTitle: { fontSize: 18, fontWeight: "600", color: "#17212e" },
  modalCounter: { fontSize: 13, color: "#647386" },
  modalFooter: {
    flexDirection: "row",
    justifyContent: "flex-end",
    gap: 10,
    paddingHorizontal: 16,
    paddingTop: 12,
    paddingBottom: 24,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: "#d5dee9",
  },
  btn: { paddingHorizontal: 16, paddingVertical: 10, borderRadius: 8 },
  btnGhost: { backgroundColor: "transparent" },
  btnGhostText: { color: "#17212e", fontSize: 15 },
  btnPrimary: { backgroundColor: "#2563eb" },
  btnPrimaryText: { color: "#fff", fontSize: 15, fontWeight: "600" },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    paddingHorizontal: 32,
    gap: 6,
  },
  errorTitle: { fontSize: 15, fontWeight: "600", color: "#17212e" },
  errorBody: { fontSize: 13, color: "#647386", textAlign: "center" },
  empty: { fontSize: 14, color: "#647386" },
  pickerRow: {
    paddingHorizontal: 16,
    paddingVertical: 12,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: "#eef2f7",
    gap: 12,
  },
  pickerRowSelected: { backgroundColor: "#e8efff" },
  check: { fontSize: 18 },
  rowInfo: { flex: 1 },
  rowTitle: { fontSize: 15, fontWeight: "500", color: "#17212e" },
  rowSub: { fontSize: 12, color: "#647386", marginTop: 2 },
});
