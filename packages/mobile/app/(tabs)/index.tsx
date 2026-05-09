// Compose tab — the default landing surface. Phase 2 will fill this
// in with a description input, extract-tags button, attach-artifact
// flow, and a Start button that pushes onto the (modal) meeting
// route.

import { SafeAreaView, StyleSheet, Text, View } from "react-native";

export default function ComposeScreen() {
  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Compose meeting</Text>
        <Text style={styles.hint}>
          Description input + extract tags + start button arrives in Phase 2.
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
