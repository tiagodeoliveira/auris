// Sign-in screen. Phase 1 wires the actual Auth0 PKCE flow via
// `expo-auth-session`. For now this is just a placeholder that the
// root Stack can present as a modal.

import { SafeAreaView, StyleSheet, Text, View } from "react-native";

export default function LoginScreen() {
  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Sign in</Text>
        <Text style={styles.hint}>Auth0 PKCE flow via expo-auth-session arrives in Phase 1.</Text>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  body: { flex: 1, padding: 16, gap: 8, justifyContent: "center" },
  title: { fontSize: 22, fontWeight: "600", textAlign: "center" },
  hint: { color: "#647386", fontSize: 14, lineHeight: 20, textAlign: "center" },
});
