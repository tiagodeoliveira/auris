// Upload affordance for the Artifacts tab. Wraps `expo-document-picker`
// so the surrounding screen only deals with success/failure callbacks,
// not the picker shape.
//
// Phase D refresh: leans on the brand-coral filled IconButton so the
// CTA reads as the primary action across the screen. Behaviour
// (picker invocation, error surface, busy state) is unchanged.

import { useState } from "react";
import { ActivityIndicator, Alert } from "react-native";
import * as DocumentPicker from "expo-document-picker";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { ArtifactsApi, type Artifact } from "@/src/wire/artifacts-api";
import { IconButton } from "@/src/ui/components";

interface UploadButtonProps {
  /// Called with the freshly created artifact once the server returns
  /// 201. The list screen uses this to insert the row optimistically
  /// (status: pending) and kick the polling loop.
  onUploaded: (artifact: Artifact) => void;
  /// Compact form for use in headers; full button shows label.
  compact?: boolean;
}

export function UploadButton({ onUploaded, compact = false }: UploadButtonProps) {
  const [busy, setBusy] = useState(false);

  async function pickAndUpload() {
    if (busy) return;
    let result: DocumentPicker.DocumentPickerResult;
    try {
      result = await DocumentPicker.getDocumentAsync({
        type: "*/*",
        copyToCacheDirectory: true,
        multiple: false,
      });
    } catch (e) {
      Alert.alert("Upload failed", e instanceof Error ? e.message : String(e));
      return;
    }
    if (result.canceled) return;
    const asset = result.assets[0];
    if (!asset) return;

    setBusy(true);
    try {
      const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
      if (!api) throw new Error("Server URL is not a valid ws:// or wss:// URL");
      const artifact = await api.upload({
        uri: asset.uri,
        name: asset.name,
        // `expo-document-picker` may return undefined for mimeType on
        // some platforms — fall back to a generic content type so the
        // multipart payload is still well-formed.
        type: asset.mimeType ?? "application/octet-stream",
      });
      onUploaded(artifact);
    } catch (e) {
      Alert.alert("Upload failed", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (busy) {
    return <ActivityIndicator style={{ marginHorizontal: 8 }} />;
  }
  return (
    <IconButton
      glyph="+"
      label={compact ? undefined : "UPLOAD"}
      tone="brand"
      filled
      onPress={() => void pickAndUpload()}
      accessibilityLabel="Upload artifact"
    />
  );
}
