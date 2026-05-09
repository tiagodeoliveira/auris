import { SafeAreaView, StyleSheet, Text, View } from "react-native";

export default function SettingsScreen() {
  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Settings</Text>
        <Text style={styles.hint}>
          Account (sign out), theme picker (light / dark / system), and any mobile-specific
          preferences (e.g., camera-attach default for moments) land in Phase 6.
        </Text>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  body: { flex: 1, padding: 16, gap: 8 },
  title: { fontSize: 22, fontWeight: "600" },
  hint: { color: "#647386", fontSize: 14, lineHeight: 20 },
});
