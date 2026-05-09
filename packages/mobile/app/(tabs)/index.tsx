// Compose tab — Phase 1 demo state. Surfaces the WS connection
// status + the current meeting state coming back from the server's
// snapshot, so we can confirm end-to-end auth → connect → snapshot
// works on a real device. Phase 2 (start-meeting form) lands here.

import { SafeAreaView, StyleSheet, Text, View } from "react-native";

import { serverUrl } from "@/src/config";
import { useAppStore } from "@/src/store";

export default function ComposeScreen() {
  const wsStatus = useAppStore((s) => s.wsStatus);
  const meetingState = useAppStore((s) => s.meetingState);
  const meetingId = useAppStore((s) => s.currentMeetingId);
  const identity = useAppStore((s) => s.identity);
  const devices = useAppStore((s) => s.devices);

  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Compose meeting</Text>
        <Text style={styles.hint}>Phase 2 lands the description input + start button.</Text>

        <View style={styles.diagBlock}>
          <Text style={styles.diagHeader}>Connection</Text>
          <Diag label="server" value={serverUrl} />
          <Diag label="ws" value={wsStatus} />
          <Diag label="signed in" value={identity?.email ?? identity?.sub ?? "no"} />
          <Diag label="meeting" value={`${meetingState}${meetingId ? ` (${meetingId})` : ""}`} />
          <Diag label="devices" value={String(devices.length)} />
        </View>
      </View>
    </SafeAreaView>
  );
}

function Diag({ label, value }: { label: string; value: string }) {
  return (
    <View style={styles.diagRow}>
      <Text style={styles.diagLabel}>{label}</Text>
      <Text style={styles.diagValue}>{value}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  body: { flex: 1, padding: 16, gap: 12 },
  title: { fontSize: 22, fontWeight: "600" },
  hint: { color: "#647386", fontSize: 14, lineHeight: 20 },
  diagBlock: {
    marginTop: 12,
    padding: 12,
    borderWidth: 1,
    borderColor: "#d5dee9",
    borderRadius: 8,
    gap: 4,
  },
  diagHeader: {
    fontSize: 11,
    fontWeight: "600",
    letterSpacing: 0.5,
    textTransform: "uppercase",
    color: "#647386",
    marginBottom: 4,
  },
  diagRow: { flexDirection: "row", gap: 8 },
  diagLabel: { color: "#647386", fontSize: 13, width: 90 },
  diagValue: { color: "#17212e", fontSize: 13, flex: 1 },
});
