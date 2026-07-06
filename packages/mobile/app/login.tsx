// Login — the launch identity moment. Large breathing AurisMark over
// a Bebas Neue wordmark, a single coral CTA. No raster icon: the
// mark is drawn live as SVG so it scales without blur and pulses
// while the user reads the screen, hinting at the listening-room
// character before they're even signed in.
//
// `auth0Configured === false` collapses to a quiet mono red banner
// rather than a full alternate layout — the misconfiguration is a
// dev-only state and shouldn't dictate the brand surface for the
// happy path.

import Constants from "expo-constants";
import { router, useLocalSearchParams } from "expo-router";
import { useState } from "react";
import {
  Alert,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import { auth0Configured } from "@/src/config";
import { useAppStore } from "@/src/store";
import { AurisMark } from "@/src/ui/AurisMark";
import { useTheme } from "@/src/theme/useTheme";

// Source of truth: `app.json`'s `expo.version` field, surfaced at
// runtime via expo-constants. Avoids the trap of a hard-coded literal
// drifting from the actual built binary's version.
const APP_VERSION = `v${Constants.expoConfig?.version ?? "?"}`;

export default function LoginScreen() {
  const t = useTheme();
  const signIn = useAppStore((s) => s.signIn);
  const [busy, setBusy] = useState(false);
  // Surfaced when an Auth0 redirect lands on the not-found route with
  // an `error_description` query param (see app/+not-found.tsx). The
  // route forwards us here so the user has a way back into the
  // happy-path sign-in flow without the generic expo-router 404.
  const { error_description: errorDescription } = useLocalSearchParams<{
    error_description?: string;
  }>();

  const handleSignIn = async () => {
    setBusy(true);
    try {
      await signIn();
      // The root layout's `<Redirect>` watches for `identity` and
      // dismisses this modal automatically. As a belt-and-suspenders,
      // pop back to the tabs explicitly.
      router.replace("/");
    } catch (e) {
      Alert.alert("sign-in failed", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: t.color.bg.canvas }}>
      <KeyboardAvoidingView
        style={{ flex: 1 }}
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        <View style={[styles.column, { paddingHorizontal: t.spacing.xl }]}>
          <AurisMark size={96} variant="coral" animate="breathe" accessibilityLabel="Auris" />

          <Text
            style={{
              fontFamily: t.font.display,
              fontSize: 56,
              letterSpacing: 4,
              color: t.color.text.primary,
              marginTop: t.spacing.xl,
            }}
          >
            auris
          </Text>

          <Text
            style={{
              ...t.type.bodySmall,
              fontFamily: t.font.monoMedium,
              letterSpacing: 2,
              textTransform: "uppercase",
              color: t.color.text.secondary,
              marginTop: t.spacing.sm,
            }}
          >
            meeting companion
          </Text>

          {errorDescription && (
            <View
              style={{
                marginTop: t.spacing.xl,
                paddingVertical: t.spacing.md,
                paddingHorizontal: t.spacing.lg,
                borderRadius: t.radius.md,
                borderWidth: 1,
                borderColor: t.color.danger.base,
                backgroundColor: t.color.danger.tint,
                alignSelf: "stretch",
              }}
            >
              <Text
                style={{
                  ...t.type.mono,
                  color: t.color.danger.base,
                  textAlign: "center",
                }}
              >
                — {errorDescription}
              </Text>
            </View>
          )}

          {auth0Configured ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Sign in with Auth0"
              onPress={handleSignIn}
              disabled={busy}
              style={({ pressed }) => [
                styles.cta,
                {
                  marginTop: t.spacing.xxxl,
                  backgroundColor: t.color.brand.coral,
                  borderRadius: t.radius.xl,
                },
                pressed && !busy && { opacity: 0.85 },
                busy && { opacity: 0.6 },
              ]}
            >
              <Text
                style={{
                  fontFamily: t.font.display,
                  fontSize: 20,
                  letterSpacing: 2,
                  color: t.color.text.onCoral,
                }}
              >
                {busy ? "signing in…" : "sign in with auth0"}
              </Text>
            </Pressable>
          ) : (
            <View
              style={{
                marginTop: t.spacing.xxxl,
                paddingVertical: t.spacing.md,
                paddingHorizontal: t.spacing.lg,
                borderRadius: t.radius.md,
                borderWidth: 1,
                borderColor: t.color.danger.base,
                backgroundColor: t.color.danger.tint,
                alignSelf: "stretch",
              }}
            >
              <Text
                style={{
                  ...t.type.mono,
                  color: t.color.danger.base,
                  textAlign: "center",
                }}
              >
                — auth0 not configured; check .env
              </Text>
            </View>
          )}
        </View>

        <View style={[styles.footer, { paddingBottom: t.spacing.lg }]}>
          <Text
            style={{
              ...t.type.labelMono,
              color: t.color.text.muted,
              letterSpacing: 2,
            }}
          >
            auris · {APP_VERSION}
          </Text>
        </View>
      </KeyboardAvoidingView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  column: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
  },
  cta: {
    alignSelf: "stretch",
    height: 56,
    alignItems: "center",
    justifyContent: "center",
  },
  footer: {
    alignItems: "center",
  },
});
