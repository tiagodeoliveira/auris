// KV chip editor for meeting metadata ("tags").
//
// Used in three places:
//   1. Compose screen — set/clear tags before starting a meeting
//   2. Live meeting — edit tags mid-session (compact mode)
//   3. Past meeting detail — edit tags retroactively
//
// Tag extraction itself is server-side and asynchronous: the server
// auto-extracts on `start_meeting` whenever a description is provided,
// and emits `metadata_changed` once the LLM returns. There's no
// client-side "Extract" affordance anymore — tags simply appear.
//
// Wires through the existing store:
//   - `metadata` (read) — current Record<string, string>
//   - `send({type: "set_metadata", key, value})` — add / edit / delete
//     (value=null deletes)
//
// All operations are idempotent on the server side; the server echoes
// the new state via `metadata_changed`, and the store's reducer mirrors
// it into `metadata`. No optimistic update needed because the round-trip
// is sub-100ms when the WS is open.

import { useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, TextInput, View } from "react-native";

import { useAppStore } from "@/src/store";
import { useTheme } from "@/src/theme/useTheme";
import { Chip } from "@/src/ui/components";

interface MetadataEditorProps {
  /** Optional title rendered above the chip row. Default: "Tags". */
  title?: string;
  /** Hide the title row entirely (for tight layouts like live meeting). */
  compact?: boolean;
  /**
   * Hide the inner "Tags" header text. Use when an outer `Section`
   * already renders the title (e.g. Compose's Tags card) to avoid
   * duplicating the label.
   */
  hideTitle?: boolean;
}

export function MetadataEditor({
  title = "Tags",
  compact = false,
  hideTitle = false,
}: MetadataEditorProps) {
  const t = useTheme();
  const metadata = useAppStore((s) => s.metadata);
  const send = useAppStore((s) => s.send);

  const entries = useMemo(
    () =>
      Object.entries(metadata)
        .filter(([, v]) => v != null && v !== "")
        .sort(([a], [b]) => a.localeCompare(b)),
    [metadata],
  );

  const [addingOpen, setAddingOpen] = useState(false);
  const [draftKey, setDraftKey] = useState("");
  const [draftValue, setDraftValue] = useState("");
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [editingValue, setEditingValue] = useState("");

  const commitDraft = () => {
    const k = draftKey.trim();
    const v = draftValue.trim();
    if (!k || !v) {
      setAddingOpen(false);
      setDraftKey("");
      setDraftValue("");
      return;
    }
    send({ type: "set_metadata", key: k, value: v });
    setAddingOpen(false);
    setDraftKey("");
    setDraftValue("");
  };

  const commitEdit = () => {
    if (!editingKey) return;
    const v = editingValue.trim();
    if (!v) {
      // Empty edit = delete the key.
      send({ type: "set_metadata", key: editingKey, value: null });
    } else {
      send({ type: "set_metadata", key: editingKey, value: v });
    }
    setEditingKey(null);
    setEditingValue("");
  };

  const removeKey = (key: string) => {
    send({ type: "set_metadata", key, value: null });
  };

  // Dashed coral pill — the signature affordance shared by Add, Attach
  // meeting, Attach artifact. Common style + token-aware.
  const dashedPillBase = {
    paddingHorizontal: t.spacing.md,
    paddingVertical: t.spacing.xs,
    borderRadius: t.radius.pill,
    borderWidth: 1,
    borderColor: t.color.brand.coral,
    borderStyle: "dashed" as const,
  };
  const dashedPillText = {
    ...t.type.bodySmall,
    color: t.color.brand.coral,
    fontFamily: t.font.sansSemi,
  };

  return (
    <View style={{ gap: t.spacing.sm }}>
      {!compact && !hideTitle && (
        <Text
          style={{
            ...t.type.caption,
            color: t.color.text.secondary,
            textTransform: "uppercase",
          }}
        >
          {title}
        </Text>
      )}

      <View style={styles.chipRow}>
        {entries.map(([key, value]) => {
          const isEditing = editingKey === key;
          if (isEditing) {
            return (
              <View
                key={key}
                style={[
                  styles.editingChip,
                  {
                    backgroundColor: t.color.bg.tint,
                    borderRadius: t.radius.pill,
                    paddingHorizontal: t.spacing.md,
                    paddingVertical: t.spacing.xs,
                    gap: t.spacing.xs,
                  },
                ]}
              >
                <Text
                  style={{
                    ...t.type.bodySmall,
                    color: t.color.text.secondary,
                    fontFamily: t.font.sansSemi,
                  }}
                >
                  {key}
                </Text>
                <Text style={{ ...t.type.bodySmall, color: t.color.text.placeholder }}>:</Text>
                <TextInput
                  style={{
                    ...t.type.bodySmall,
                    color: t.color.text.primary,
                    minWidth: 60,
                    paddingVertical: 0,
                  }}
                  value={editingValue}
                  onChangeText={setEditingValue}
                  onBlur={commitEdit}
                  onSubmitEditing={commitEdit}
                  autoFocus
                  returnKeyType="done"
                  placeholder="value"
                  placeholderTextColor={t.color.text.placeholder}
                />
                <Pressable
                  onPress={() => removeKey(key)}
                  hitSlop={6}
                  style={{ paddingHorizontal: 2, marginLeft: t.spacing.xs }}
                >
                  <Text style={{ fontSize: 16, color: t.color.text.secondary, fontWeight: "600" }}>
                    ×
                  </Text>
                </Pressable>
              </View>
            );
          }
          return (
            <Pressable
              key={key}
              onPress={() => {
                setEditingKey(key);
                setEditingValue(value);
              }}
              style={({ pressed }) => [pressed && { opacity: 0.7 }]}
            >
              <Chip label={`${key}: ${value}`} tone="neutral" onRemove={() => removeKey(key)} />
            </Pressable>
          );
        })}

        {addingOpen ? (
          <View
            style={[
              styles.editingChip,
              {
                backgroundColor: t.color.bg.tint,
                borderRadius: t.radius.pill,
                paddingHorizontal: t.spacing.md,
                paddingVertical: t.spacing.xs,
                gap: t.spacing.xs,
              },
            ]}
          >
            <TextInput
              style={{
                ...t.type.bodySmall,
                color: t.color.text.primary,
                minWidth: 60,
                paddingVertical: 0,
              }}
              value={draftKey}
              onChangeText={setDraftKey}
              placeholder="key"
              placeholderTextColor={t.color.text.placeholder}
              autoFocus
              returnKeyType="next"
            />
            <Text style={{ ...t.type.bodySmall, color: t.color.text.placeholder }}>:</Text>
            <TextInput
              style={{
                ...t.type.bodySmall,
                color: t.color.text.primary,
                minWidth: 60,
                paddingVertical: 0,
              }}
              value={draftValue}
              onChangeText={setDraftValue}
              onSubmitEditing={commitDraft}
              onBlur={commitDraft}
              placeholder="value"
              placeholderTextColor={t.color.text.placeholder}
              returnKeyType="done"
            />
          </View>
        ) : (
          <Pressable
            onPress={() => setAddingOpen(true)}
            style={({ pressed }) => [dashedPillBase, pressed && { opacity: 0.6 }]}
          >
            <Text style={dashedPillText}>+ ADD</Text>
          </Pressable>
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  chipRow: {
    flexDirection: "row",
    flexWrap: "wrap",
    gap: 8,
    alignItems: "center",
  },
  editingChip: {
    flexDirection: "row",
    alignItems: "center",
    minWidth: 120,
  },
});
