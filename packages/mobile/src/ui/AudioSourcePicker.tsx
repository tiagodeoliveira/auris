// Audio-source picker for the compose flow.
//
// Mirrors `packages/pwa/src/ui/compose-audio-source.ts`:
//   - Lists devices with the `audio_capture` capability
//   - Adds a "This phone (microphone)" entry at the top for local mic
//   - Auto-selects the first online device when nothing is picked,
//     falling back to the local mic when no remote is available
//
// Visual treatment (Phase B):
//   - Trigger: elevated card surface, MonoLabel "AUDIO SOURCE" header,
//     selection name in body, chevron on the right. Online remote
//     selections get a coral status dot; the local-mic selection
//     shows the MicActivityIcon inline.
//   - Sheet footer: when no audio-capable remote has ever registered,
//     a small "— open the Mac app to pair a system-audio source"
//     hint sits under the options. It's advisory now, not a block —
//     the local mic is always selectable.

import { useEffect, useMemo, useState } from "react";
import { FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import { useAppStore } from "@/src/store";
import { useTheme } from "@/src/theme/useTheme";
import { AurisMark } from "@/src/ui/AurisMark";
import { MicActivityIcon } from "@/src/ui/MicActivityIcon";
import { MonoLabel, Sheet } from "@/src/ui/components";
import type { Device } from "@/src/wire/contract";

/// Sentinel device id used to represent the local microphone.
/// Distinct from any server-issued UUID; the start_meeting handler
/// consumer can substitute the real device id once the mobile mic
/// is wired as a registered device (Phase D).
const LOCAL_MIC_ID = "__local_mic__";

interface DeviceOption {
  id: string;
  hostname: string;
  online: boolean;
  /** When true this is the synthetic local-mic row, not a server device. */
  local?: boolean;
}

export function AudioSourcePicker() {
  const t = useTheme();
  const devices = useAppStore((s) => s.devices);
  const audioSourceDeviceId = useAppStore((s) => s.audioSourceDeviceId);
  const setAudioSourceDeviceId = useAppStore((s) => s.setAudioSourceDeviceId);

  const [sheetOpen, setSheetOpen] = useState(false);

  // Server-side audio-capable devices. We keep offline ones in the
  // list (disabled) so users can see why the picker isn't empty.
  const audioCapable = useMemo(
    () => devices.filter((d) => d.capabilities.includes("audio_capture")),
    [devices],
  );

  const onlineRemote = audioCapable.filter((d) => d.online);

  // The local mic is always available — it's the phone itself.
  const localOption: DeviceOption = {
    id: LOCAL_MIC_ID,
    hostname: "This phone (microphone)",
    online: true,
    local: true,
  };

  const remoteOptions: DeviceOption[] = audioCapable.map((d) => ({
    id: d.id,
    hostname: d.hostname,
    online: d.online,
  }));

  // Online remote devices first (so the sheet's grouped feel reads
  // top-down: connected Macs, then local mic, then offline devices).
  const onlineRemoteOpts = remoteOptions.filter((o) => o.online);
  const offlineRemoteOpts = remoteOptions.filter((o) => !o.online);
  const allOptions: DeviceOption[] = [...onlineRemoteOpts, localOption, ...offlineRemoteOpts];

  // Auto-seed: if nothing is picked, default to the first online
  // device (preferring a remote source if one is online, otherwise
  // the local mic). Deferred so we don't trigger a re-render mid-
  // render of a subscribed parent.
  useEffect(() => {
    if (audioSourceDeviceId !== null) return;
    const firstOnlineRemote = onlineRemote[0];
    const pick = firstOnlineRemote ? firstOnlineRemote.id : LOCAL_MIC_ID;
    setAudioSourceDeviceId(pick);
    // We intentionally only re-evaluate when the selection becomes
    // null or when the online-remote set changes. `setAudioSourceDeviceId`
    // is stable from zustand.
  }, [audioSourceDeviceId, onlineRemote.length]); // eslint-disable-line react-hooks/exhaustive-deps

  // If the picked device disappeared from the device list, fall
  // back to local mic. Mirrors the PWA's "clear stale pick" branch.
  useEffect(() => {
    if (audioSourceDeviceId === null || audioSourceDeviceId === LOCAL_MIC_ID) return;
    const stillPresent = audioCapable.some((d) => d.id === audioSourceDeviceId);
    if (!stillPresent) {
      setAudioSourceDeviceId(LOCAL_MIC_ID);
    }
  }, [audioSourceDeviceId, audioCapable]); // eslint-disable-line react-hooks/exhaustive-deps

  // Resolve the currently-picked option for the closed-state label.
  const selected = allOptions.find((o) => o.id === audioSourceDeviceId) ?? localOption;

  // When no remote audio-capable client has ever registered, we still
  // render the picker (the local mic is a valid source). The Mac-app
  // pairing tip moves into the sheet footer below the options.
  const showPairingHint = audioCapable.length === 0;

  // Card-style trigger using the elevated surface so the picker reads
  // as a "field" inside the parent Card. Dark mode keeps the hairline
  // border because the elevated bg is only slightly lighter than the
  // canvas — without an edge the field would dissolve.
  return (
    <View>
      <Pressable
        onPress={() => setSheetOpen(true)}
        style={({ pressed }) => [
          {
            flexDirection: "row",
            alignItems: "center",
            paddingVertical: t.spacing.md,
            paddingHorizontal: t.spacing.lg,
            backgroundColor: t.color.bg.elevated,
            borderRadius: t.radius.lg,
            borderWidth: 1,
            borderColor: t.color.border.strong,
            gap: t.spacing.md,
          },
          pressed && { opacity: 0.7 },
        ]}
        accessibilityLabel={`Audio source: ${selected.hostname}`}
      >
        {/* Left: status indicator. Local mic gets MicActivityIcon
            (small, idle since no peak data here); remote online gets
            a 6pt coral dot; offline gets a muted dot. */}
        {selected.local ? (
          <MicActivityIcon size={20} peak={0} isRecording={false} />
        ) : (
          <View
            style={{
              width: 8,
              height: 8,
              borderRadius: 4,
              backgroundColor: selected.online ? t.color.status.ok : t.color.text.muted,
            }}
          />
        )}

        <View style={{ flex: 1, gap: 2 }}>
          <MonoLabel tone="secondary">AUDIO SOURCE</MonoLabel>
          <Text
            style={{
              ...t.type.body,
              color: t.color.text.primary,
              fontFamily: t.font.sansMedium,
            }}
            numberOfLines={1}
          >
            {selected.hostname}
            {selected.online ? "" : " (offline)"}
          </Text>
        </View>
        <Text
          style={{
            fontSize: 22,
            color: t.color.text.secondary,
            lineHeight: 22,
          }}
        >
          ›
        </Text>
      </Pressable>

      <Sheet visible={sheetOpen} onClose={() => setSheetOpen(false)} title="Audio source">
        <FlatList
          data={allOptions}
          keyExtractor={(o) => o.id}
          renderItem={({ item }) => (
            <OptionRow
              option={item}
              checked={item.id === audioSourceDeviceId}
              onPress={() => {
                if (!item.online) return;
                setAudioSourceDeviceId(item.id);
                setSheetOpen(false);
              }}
            />
          )}
          ListFooterComponent={
            showPairingHint ? (
              <View
                style={{
                  flexDirection: "row",
                  alignItems: "center",
                  gap: t.spacing.md,
                  paddingHorizontal: t.spacing.lg,
                  paddingVertical: t.spacing.lg,
                }}
              >
                <AurisMark size={16} variant="mono" background={false} color={t.color.text.muted} />
                <MonoLabel tone="muted" style={{ flex: 1 }}>
                  {"— open the Mac app to pair a system-audio source"}
                </MonoLabel>
              </View>
            ) : null
          }
          style={{ marginHorizontal: -t.spacing.lg }}
        />
      </Sheet>
    </View>
  );
}

function OptionRow({
  option,
  checked,
  onPress,
}: {
  option: DeviceOption;
  checked: boolean;
  onPress: () => void;
}) {
  const t = useTheme();
  const disabled = !option.online;
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      style={({ pressed }) => [
        {
          flexDirection: "row",
          alignItems: "center",
          gap: t.spacing.md,
          paddingHorizontal: t.spacing.lg,
          paddingVertical: t.spacing.md,
          borderBottomWidth: StyleSheet.hairlineWidth,
          borderBottomColor: t.color.border.soft,
        },
        checked && { backgroundColor: t.color.action.primaryDim },
        disabled && { opacity: 0.45 },
        pressed && !disabled && { opacity: 0.7 },
      ]}
    >
      {/* Status pip — coral dot for online remote, mic icon for
          local-mic option, muted dot for offline. */}
      {option.local ? (
        <MicActivityIcon size={18} peak={0} isRecording={false} />
      ) : (
        <View
          style={{
            width: 8,
            height: 8,
            borderRadius: 4,
            backgroundColor: option.online ? t.color.status.ok : t.color.text.muted,
          }}
        />
      )}
      <View style={{ flex: 1, gap: 2 }}>
        <Text
          style={{
            ...t.type.body,
            color: t.color.text.primary,
            fontFamily: t.font.sansMedium,
          }}
        >
          {option.hostname}
          {disabled ? " (offline)" : ""}
        </Text>
        {option.local && <MonoLabel tone="muted">CAPTURED ON THIS DEVICE</MonoLabel>}
      </View>
      <Text
        style={{
          fontSize: 18,
          color: checked ? t.color.brand.coral : t.color.text.muted,
          width: 20,
          textAlign: "center",
        }}
      >
        {checked ? "●" : "○"}
      </Text>
    </Pressable>
  );
}

/// Re-exported sentinel for callers that need to detect the local-mic
/// pick (e.g. the start_meeting payload constructor — it omits the
/// `audio_source_device_id` field when the user picked the phone mic,
/// matching the PWA's "no remote source" semantics).
export { LOCAL_MIC_ID };

// Suppress an unused-import error if the Device import is later
// dropped: kept for callers that want to type their own selectors.
export type AudioCapableDevice = Device;
