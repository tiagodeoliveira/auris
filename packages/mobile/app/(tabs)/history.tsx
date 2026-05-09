import { SafeAreaView, StyleSheet, Text, View } from "react-native";

export default function HistoryScreen() {
  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Meeting history</Text>
        <Text style={styles.hint}>
          Past meetings list (bucketed by day) + tap-into detail arrives in Phase 5.
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
