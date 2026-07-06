// Reusable artifact picker. Other phases (compose-attach, mid-meeting
// attach) drive this modal to let the user choose artifacts from
// their library.
//
// Mirrors the PWA's `pickArtifacts` semantics: rows with
// `summary_status: "done"` are selectable; pending/failed are shown
// but un-toggleable. Built on top of the shared `<Sheet>` primitive
// so visual chrome stays consistent with other modals in the app.
//
// Phase D refresh: rows are styled around the editorial palette,
// the checkbox replaces unicode glyphs with a coral-filled circle,
// and the right-action confirms with a mono "ATTACH (N)" label.

import { useEffect, useState } from "react";
import { ActivityIndicator, FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { useTheme } from "@/src/theme/useTheme";
import { Chip, MonoLabel, Sheet } from "@/src/ui/components";
import { ArtifactsApi, type Artifact } from "@/src/wire/artifacts-api";
import { StatusChip, formatBytes } from "./ArtifactRow";

/**
 * Reusable artifact-picker modal. Used by:
 *   - Phase C (compose screen) to stage attachments before
 *     `start_meeting`
 *   - Phase E or mid-meeting flows to attach during a live meeting
 *
 * Selection state lives inside the picker; on confirm the chosen
 * IDs are passed back. Cancel / backdrop close discards the choice.
 */
export interface ArtifactPickerProps {
  visible: boolean;
  onClose: () => void;
  onConfirm: (selectedIds: string[]) => void;
  /** Pre-select these artifact IDs (for editing an existing attachment list). */
  initialSelected?: string[];
  /** Allow multiple selection (default true) or single (false). */
  multi?: boolean;
}

export function ArtifactPicker({
  visible,
  onClose,
  onConfirm,
  initialSelected = [],
  multi = true,
}: ArtifactPickerProps) {
  const t = useTheme();
  const [library, setLibrary] = useState<Artifact[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set(initialSelected));
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-seed selection + reload library each time the picker opens.
  // Closing leaves the cache around but it's discarded next open;
  // cheap to refetch and keeps the list fresh after a recent upload.
  useEffect(() => {
    if (!visible) return;
    setSelected(new Set(initialSelected));
    setLoading(true);
    setError(null);
    const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
    if (!api) {
      setError("Server URL is not configured. Open Settings.");
      setLoading(false);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const list = await api.list();
        if (!cancelled) setLibrary(list);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // initialSelected is a fresh array reference each parent render;
    // the visible toggle is what actually drives the open lifecycle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible]);

  function toggle(a: Artifact) {
    if (a.summary_status !== "done") return; // un-toggleable pending/failed rows
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(a.id)) {
        next.delete(a.id);
      } else {
        if (!multi) next.clear();
        next.add(a.id);
      }
      return next;
    });
  }

  function confirm() {
    onConfirm(Array.from(selected));
  }

  return (
    <Sheet
      visible={visible}
      onClose={onClose}
      title="Attach artifacts"
      maxHeight={640}
      rightAction={
        <Pressable onPress={confirm} hitSlop={8}>
          <MonoLabel tone="brand">{`ATTACH (${selected.size})`}</MonoLabel>
        </Pressable>
      }
    >
      {loading ? (
        <View style={[styles.center, { paddingVertical: t.spacing.xxl }]}>
          <ActivityIndicator color={t.color.brand.coral} />
        </View>
      ) : error ? (
        <View style={[styles.center, { paddingVertical: t.spacing.xxl, gap: t.spacing.xs }]}>
          <Text style={{ ...t.type.subtitle, color: t.color.text.primary }}>
            Couldn't load artifacts
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
        <View style={[styles.center, { paddingVertical: t.spacing.xxl }]}>
          <Text style={{ ...t.type.body, color: t.color.text.secondary }}>
            No artifacts in your library yet.
          </Text>
        </View>
      ) : (
        <FlatList
          data={library}
          keyExtractor={(a) => a.id}
          renderItem={({ item }) => (
            <PickerRow
              artifact={item}
              checked={selected.has(item.id)}
              onToggle={() => toggle(item)}
            />
          )}
          style={{ marginHorizontal: -t.spacing.lg }}
        />
      )}
    </Sheet>
  );
}

function PickerRow({
  artifact,
  checked,
  onToggle,
}: {
  artifact: Artifact;
  checked: boolean;
  onToggle: () => void;
}) {
  const t = useTheme();
  const selectable = artifact.summary_status === "done";
  return (
    <Pressable
      onPress={onToggle}
      disabled={!selectable && !checked}
      style={({ pressed }) => [
        styles.pickerRow,
        {
          paddingHorizontal: t.spacing.lg,
          paddingVertical: t.spacing.md,
          gap: t.spacing.md,
          borderBottomColor: t.color.border.soft,
        },
        checked && { backgroundColor: t.color.brand.coralDim },
        !selectable && styles.pickerRowDisabled,
        pressed && selectable && { opacity: 0.7 },
      ]}
    >
      <View
        style={[
          styles.check,
          {
            borderColor: checked ? t.color.brand.coral : t.color.border.strong,
            backgroundColor: checked ? t.color.brand.coral : "transparent",
          },
        ]}
      >
        {checked && (
          // Inner cream dot for a clear "selected" affordance — keeps
          // the visual language consistent with the AurisMark.
          <View
            style={{
              width: 8,
              height: 8,
              borderRadius: 4,
              backgroundColor: t.color.text.onCoral,
            }}
          />
        )}
      </View>
      <View style={[styles.info, { gap: t.spacing.xs }]}>
        <View style={[styles.infoHead, { gap: t.spacing.sm }]}>
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
        {artifact.short_summary ? (
          <Text style={{ ...t.type.bodySmall, color: t.color.text.secondary }} numberOfLines={2}>
            {artifact.short_summary}
          </Text>
        ) : artifact.summary_status === "pending" ? (
          <Text
            style={{
              ...t.type.bodySmall,
              color: t.color.status.pending,
              fontStyle: "italic",
            }}
          >
            ── summary generating
          </Text>
        ) : artifact.summary_status === "failed" ? (
          <Text
            style={{
              ...t.type.bodySmall,
              color: t.color.danger.base,
              fontStyle: "italic",
            }}
          >
            ── summary failed
          </Text>
        ) : null}
        <View style={[styles.infoMeta, { gap: t.spacing.sm }]}>
          <Chip label={artifact.mime_type} tone="neutral" size="sm" />
          <MonoLabel tone="muted">{formatBytes(artifact.size_bytes)}</MonoLabel>
        </View>
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  center: {
    alignItems: "center",
    justifyContent: "center",
  },
  pickerRow: {
    flexDirection: "row",
    borderBottomWidth: StyleSheet.hairlineWidth,
    alignItems: "flex-start",
  },
  pickerRowDisabled: {
    opacity: 0.45,
  },
  check: {
    width: 20,
    height: 20,
    borderRadius: 10,
    borderWidth: 1.5,
    alignItems: "center",
    justifyContent: "center",
    marginTop: 2,
  },
  info: { flex: 1 },
  infoHead: { flexDirection: "row", alignItems: "center" },
  infoMeta: {
    flexDirection: "row",
    alignItems: "center",
  },
});
