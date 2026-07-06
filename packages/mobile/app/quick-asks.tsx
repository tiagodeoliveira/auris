//! Quick Asks editor screen — list the user's saved prompts and
//! add / edit / delete them. Server keeps the canonical library;
//! this screen reads `itemsByMode["quick_asks"]` and writes via the
//! `upsert_quick_ask` / `delete_quick_ask` intents.

import { Stack, router } from "expo-router";
import { useMemo, useState } from "react";
import {
  Alert,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  ScrollView,
  Text,
  TextInput,
  View,
} from "react-native";

import { useAppStore } from "@/src/store";
import { useTheme } from "@/src/theme/useTheme";
import type { Item } from "@/src/wire/contract";

const MODE = "quick_asks";

function newId(): string {
  // RN's global has crypto.randomUUID via expo-crypto on most setups,
  // but fall back to a timestamp-based id where it's absent.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const c: { randomUUID?: () => string } | undefined = (globalThis as any).crypto;
  if (c?.randomUUID) return c.randomUUID();
  return `qa-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

interface EditingState {
  id: string;
  label: string;
  text: string;
  position: number;
  isNew: boolean;
}

export default function QuickAsksScreen() {
  const t = useTheme();
  const send = useAppStore((s) => s.send);
  const itemsByMode = useAppStore((s) => s.itemsByMode);
  const asks = useMemo(() => (itemsByMode.quick_asks ?? []).slice(), [itemsByMode]);

  const [editing, setEditing] = useState<EditingState | null>(null);

  function startNew() {
    const maxPos = asks.reduce((acc, it) => Math.max(acc, Number(it.t) || 0), 0);
    setEditing({
      id: newId(),
      label: "",
      text: "",
      position: maxPos + 10,
      isNew: true,
    });
  }

  function startEdit(item: Item) {
    setEditing({
      id: item.id,
      label: item.text,
      text: item.detail ?? "",
      position: Number(item.t) || 0,
      isNew: false,
    });
  }

  function save() {
    if (!editing) return;
    const label = editing.label.trim();
    const text = editing.text.trim();
    if (!label || !text) return;
    send({
      type: "upsert_quick_ask",
      id: editing.id,
      label,
      text,
      position: editing.position,
    });
    setEditing(null);
  }

  function remove() {
    if (!editing || editing.isNew) return;
    Alert.alert("Delete quick ask?", `"${editing.label}" will be removed permanently.`, [
      { text: "Cancel", style: "cancel" },
      {
        text: "Delete",
        style: "destructive",
        onPress: () => {
          send({ type: "delete_quick_ask", id: editing.id });
          setEditing(null);
        },
      },
    ]);
  }

  return (
    <>
      <Stack.Screen options={{ title: "Quick Asks", headerBackTitle: "Back" }} />
      <KeyboardAvoidingView
        style={{ flex: 1, backgroundColor: t.color.bg.canvas }}
        behavior={Platform.OS === "ios" ? "padding" : undefined}
        keyboardVerticalOffset={Platform.OS === "ios" ? 90 : 0}
      >
        {editing ? (
          <ScrollView
            contentContainerStyle={{ padding: t.spacing.md, gap: t.spacing.md }}
            keyboardShouldPersistTaps="handled"
          >
            <View style={{ gap: t.spacing.xs }}>
              <Text style={{ ...t.type.caption, color: t.color.text.secondary }}>LABEL</Text>
              <TextInput
                value={editing.label}
                onChangeText={(v) => setEditing({ ...editing, label: v })}
                maxLength={40}
                placeholder="Short mnemonic"
                placeholderTextColor={t.color.text.placeholder}
                style={{
                  ...t.type.body,
                  color: t.color.text.primary,
                  borderWidth: 1,
                  borderColor: t.color.border.strong,
                  borderRadius: t.radius.md,
                  padding: t.spacing.sm + 2,
                  backgroundColor: t.color.bg.elevated,
                }}
              />
            </View>
            <View style={{ gap: t.spacing.xs }}>
              <Text style={{ ...t.type.caption, color: t.color.text.secondary }}>PROMPT</Text>
              <TextInput
                value={editing.text}
                onChangeText={(v) => setEditing({ ...editing, text: v })}
                multiline
                numberOfLines={8}
                placeholder="Multiline; markdown OK. This is what gets sent to chat."
                placeholderTextColor={t.color.text.placeholder}
                style={{
                  ...t.type.body,
                  color: t.color.text.primary,
                  borderWidth: 1,
                  borderColor: t.color.border.strong,
                  borderRadius: t.radius.md,
                  padding: t.spacing.sm + 2,
                  minHeight: 180,
                  textAlignVertical: "top",
                  fontFamily: t.font.mono,
                  backgroundColor: t.color.bg.elevated,
                }}
              />
            </View>
            <View style={{ flexDirection: "row", gap: t.spacing.sm }}>
              <Pressable
                onPress={save}
                style={({ pressed }) => ({
                  flex: 1,
                  paddingVertical: t.spacing.sm + 2,
                  alignItems: "center",
                  backgroundColor: t.color.brand.coral,
                  borderRadius: t.radius.md,
                  opacity: pressed ? 0.85 : 1,
                })}
              >
                <Text
                  style={{
                    color: t.color.text.onCoral,
                    ...t.type.body,
                    fontFamily: t.font.sansSemi,
                  }}
                >
                  Save
                </Text>
              </Pressable>
              <Pressable
                onPress={() => setEditing(null)}
                style={({ pressed }) => ({
                  paddingVertical: t.spacing.sm + 2,
                  paddingHorizontal: t.spacing.lg,
                  alignItems: "center",
                  borderWidth: 1,
                  borderColor: t.color.border.strong,
                  borderRadius: t.radius.md,
                  opacity: pressed ? 0.6 : 1,
                })}
              >
                <Text style={{ ...t.type.body, color: t.color.text.secondary }}>Cancel</Text>
              </Pressable>
              {!editing.isNew && (
                <Pressable
                  onPress={remove}
                  style={({ pressed }) => ({
                    paddingVertical: t.spacing.sm + 2,
                    paddingHorizontal: t.spacing.lg,
                    alignItems: "center",
                    borderWidth: 1,
                    borderColor: t.color.brand.coral,
                    borderRadius: t.radius.md,
                    opacity: pressed ? 0.6 : 1,
                  })}
                >
                  <Text
                    style={{
                      ...t.type.body,
                      color: t.color.brand.coral,
                    }}
                  >
                    Delete
                  </Text>
                </Pressable>
              )}
            </View>
          </ScrollView>
        ) : (
          <ScrollView contentContainerStyle={{ padding: t.spacing.md, gap: t.spacing.sm }}>
            <Pressable
              onPress={startNew}
              style={({ pressed }) => ({
                paddingVertical: t.spacing.sm + 2,
                paddingHorizontal: t.spacing.md,
                alignItems: "center",
                backgroundColor: t.color.brand.coral,
                borderRadius: t.radius.md,
                opacity: pressed ? 0.85 : 1,
                marginBottom: t.spacing.sm,
              })}
            >
              <Text
                style={{
                  color: t.color.text.onCoral,
                  ...t.type.body,
                  fontFamily: t.font.sansSemi,
                }}
              >
                + Add quick ask
              </Text>
            </Pressable>
            {asks.length === 0 && (
              <Text style={{ ...t.type.bodySmall, color: t.color.text.placeholder }}>
                — No quick asks yet. Add one above.
              </Text>
            )}
            {asks.map((ask) => (
              <Pressable
                key={ask.id}
                onPress={() => startEdit(ask)}
                style={({ pressed }) => ({
                  paddingVertical: t.spacing.sm + 2,
                  borderBottomWidth: 1,
                  borderBottomColor: t.color.border.soft,
                  opacity: pressed ? 0.6 : 1,
                })}
              >
                <Text style={{ ...t.type.body, fontFamily: t.font.sansSemi }}>{ask.text}</Text>
                <Text
                  numberOfLines={1}
                  style={{
                    ...t.type.bodySmall,
                    color: t.color.text.secondary,
                    marginTop: 2,
                  }}
                >
                  {(ask.detail ?? "").split("\n")[0]}
                </Text>
              </Pressable>
            ))}
          </ScrollView>
        )}
      </KeyboardAvoidingView>
    </>
  );
}
