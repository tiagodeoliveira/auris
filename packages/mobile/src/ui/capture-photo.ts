// Shared camera/library capture: permission-gate, launch the picker,
// downscale to a JPEG. Native + permission-gated, so it's verified
// on-device (EAS build), not in the node-env unit runner.
//
// Returns null when the user cancels or denies permission (callers
// treat that as "no photo", not an error). Throws only on real
// failures, which callers surface via Alert.

import { Alert } from "react-native";
import * as ImagePicker from "expo-image-picker";
import * as ImageManipulator from "expo-image-manipulator";

export interface CapturedPhoto {
  uri: string;
  mime: string;
}

export async function capturePhoto(
  source: "camera" | "library",
): Promise<CapturedPhoto | null> {
  const perm =
    source === "camera"
      ? await ImagePicker.requestCameraPermissionsAsync()
      : await ImagePicker.requestMediaLibraryPermissionsAsync();
  if (!perm.granted) {
    Alert.alert(
      "Permission needed",
      source === "camera"
        ? "Camera access is required to take a photo."
        : "Photo library access is required to choose a photo.",
    );
    return null;
  }
  const result =
    source === "camera"
      ? await ImagePicker.launchCameraAsync({ quality: 1 })
      : await ImagePicker.launchImageLibraryAsync({
          mediaTypes: ["images"],
          quality: 1,
        });
  if (result.canceled) return null;
  const asset = result.assets[0];
  if (!asset) return null;

  // Downscale + JPEG-compress before upload: a raw 12MP photo is far
  // too large for the upload + vision prompt.
  const out = await ImageManipulator.manipulateAsync(
    asset.uri,
    [{ resize: { width: 1280 } }],
    { compress: 0.7, format: ImageManipulator.SaveFormat.JPEG },
  );
  return { uri: out.uri, mime: "image/jpeg" };
}
