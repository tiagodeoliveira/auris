import { router } from "expo-router";
import { useState } from "react";
import { Alert, Pressable, SafeAreaView, StyleSheet, Text, View } from "react-native";

import { auth0Configured } from "@/src/config";
import { useAppStore } from "@/src/store";

export default function LoginScreen() {
  const signIn = useAppStore((s) => s.signIn);
  const [busy, setBusy] = useState(false);

  if (!auth0Configured) {
    return (
      <SafeAreaView style={styles.root}>
        <View style={styles.body}>
          <Text style={styles.title}>Auth0 not configured</Text>
          <Text style={styles.hint}>
            The build is missing one or more `EXPO_PUBLIC_AUTH0_*` values. See
            `.github/workflows/README.md` for the EAS env-var setup.
          </Text>
        </View>
      </SafeAreaView>
    );
  }

  const handleSignIn = async () => {
    setBusy(true);
    try {
      await signIn();
      // The root layout's `<Redirect>` watches for `identity` and
      // dismisses this modal automatically. As a belt-and-suspenders,
      // pop back to the tabs explicitly.
      router.replace("/");
    } catch (e) {
      Alert.alert("Sign-in failed", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SafeAreaView style={styles.root}>
      <View style={styles.body}>
        <Text style={styles.title}>Sign in</Text>
        <Text style={styles.hint}>Auth0 universal login opens in the system browser.</Text>
        <Pressable
          style={[styles.button, busy && styles.buttonDisabled]}
          onPress={handleSignIn}
          disabled={busy}
        >
          <Text style={styles.buttonText}>{busy ? "Signing in…" : "Sign in with Auth0"}</Text>
        </Pressable>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  body: { flex: 1, padding: 16, gap: 12, justifyContent: "center" },
  title: { fontSize: 22, fontWeight: "600", textAlign: "center" },
  hint: { color: "#647386", fontSize: 14, lineHeight: 20, textAlign: "center" },
  button: {
    marginTop: 8,
    backgroundColor: "#2563eb",
    paddingVertical: 14,
    borderRadius: 10,
    alignItems: "center",
  },
  buttonDisabled: { opacity: 0.6 },
  buttonText: { color: "#fff", fontSize: 16, fontWeight: "600" },
});
