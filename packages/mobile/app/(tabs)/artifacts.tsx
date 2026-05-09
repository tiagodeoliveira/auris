import { SafeAreaView, StyleSheet, Text, View } from "react-native";

export default function ArtifactsScreen() {
  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Artifacts</Text>
        <Text style={styles.hint}>
          Library of uploaded PDFs / images / text. Upload (camera, files, paste) +
          attach-to-meeting arrives in Phase 5.
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
