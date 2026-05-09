import { Pressable, SafeAreaView, StyleSheet, Text, View } from "react-native";

import { useAppStore } from "@/src/store";

export default function SettingsScreen() {
  const identity = useAppStore((s) => s.identity);
  const signOut = useAppStore((s) => s.signOut);

  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Settings</Text>

        <View style={styles.section}>
          <Text style={styles.sectionHeader}>Account</Text>
          {identity ? (
            <>
              <Text style={styles.identityLine}>
                {identity.name ?? identity.email ?? "Signed in"}
              </Text>
              {identity.email && identity.email !== identity.name && (
                <Text style={styles.identitySub}>{identity.email}</Text>
              )}
              <Pressable style={styles.button} onPress={() => void signOut()}>
                <Text style={styles.buttonText}>Sign out</Text>
              </Pressable>
            </>
          ) : (
            <Text style={styles.hint}>Not signed in.</Text>
          )}
        </View>

        <Text style={styles.hint}>Theme picker + per-device preferences land in Phase 6.</Text>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  body: { flex: 1, padding: 16, gap: 16 },
  title: { fontSize: 22, fontWeight: "600" },
  hint: { color: "#647386", fontSize: 14, lineHeight: 20 },
  section: {
    padding: 12,
    borderWidth: 1,
    borderColor: "#d5dee9",
    borderRadius: 8,
    gap: 6,
  },
  sectionHeader: {
    fontSize: 11,
    fontWeight: "600",
    letterSpacing: 0.5,
    textTransform: "uppercase",
    color: "#647386",
  },
  identityLine: { fontSize: 16, fontWeight: "500", color: "#17212e" },
  identitySub: { fontSize: 13, color: "#647386" },
  button: {
    marginTop: 8,
    paddingVertical: 10,
    paddingHorizontal: 16,
    borderRadius: 8,
    borderWidth: 1,
    borderColor: "#e5484d",
    alignSelf: "flex-start",
  },
  buttonText: { color: "#e5484d", fontSize: 14, fontWeight: "600" },
});
