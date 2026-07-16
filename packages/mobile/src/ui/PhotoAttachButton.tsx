// Camera/library capture affordance for the chat input. Presents an
// action sheet (Take Photo / Choose from Library), downscales the
// result to a JPEG, and hands a StagedPhoto back via onPicked. All
// native + permission-gated, so this is verified on-device (EAS
// build), not in the node-env unit runner — hence the thin,
// imperative shape.

import { useState } from "react";
import { ActionSheetIOS, Alert, Platform, Pressable, Text } from "react-native";

import { useTheme } from "@/src/theme/useTheme";
import { capturePhoto } from "@/src/ui/capture-photo";
import type { StagedPhoto } from "@/src/ui/meeting-detail/chat-photo-staging";

interface Props {
  disabled?: boolean;
  onPicked: (photo: StagedPhoto) => void;
}

let seq = 0;

export function PhotoAttachButton({ disabled, onPicked }: Props) {
  const t = useTheme();
  const [busy, setBusy] = useState(false);

  async function runPicker(source: "camera" | "library") {
    setBusy(true);
    try {
      const photo = await capturePhoto(source);
      if (!photo) return;
      onPicked({ id: `p${seq++}`, uri: photo.uri, mime: photo.mime });
    } catch (e) {
      Alert.alert("Could not add photo", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  function present() {
    if (disabled || busy) return;
    if (Platform.OS === "ios") {
      ActionSheetIOS.showActionSheetWithOptions(
        {
          options: ["Take Photo", "Choose from Library", "Cancel"],
          cancelButtonIndex: 2,
        },
        (i) => {
          if (i === 0) void runPicker("camera");
          else if (i === 1) void runPicker("library");
        },
      );
    } else {
      Alert.alert("Add photo", undefined, [
        { text: "Take Photo", onPress: () => void runPicker("camera") },
        { text: "Choose from Library", onPress: () => void runPicker("library") },
        { text: "Cancel", style: "cancel" },
      ]);
    }
  }

  return (
    <Pressable
      onPress={present}
      disabled={disabled || busy}
      accessibilityLabel="Attach photo"
      style={({ pressed }) => ({
        paddingHorizontal: t.spacing.md,
        justifyContent: "center",
        opacity: disabled || busy ? 0.4 : pressed ? 0.6 : 1,
      })}
    >
      <Text style={{ fontSize: 20 }}>📷</Text>
    </Pressable>
  );
}
