// Catch-all for routes expo-router can't match.
//
// Two distinct shapes:
//
//   1. Auth0 redirect failure. When the dashboard is misconfigured
//      (wrong audience, missing callback URL, etc.) the OS dispatches
//      the failure URL as a deep link. expo-auth-session can't claim
//      it because the session is no longer pending, so it falls
//      through to the router which lands here. We forward to /login
//      with the `error_description` query param attached so the
//      login screen can surface it inline rather than the user
//      staring at a generic "Unmatched Route" page.
//
//   2. Everything else — genuine missing routes. Render a quiet
//      brand-aligned 404 with a back button.
//
// The Auth0 detection looks for either `error` (the OAuth2 query
// param) or `error_description` (Auth0's longer-form variant). We
// don't try to differentiate between failure modes — any auth-shape
// error means "send the user back to login".

import { router, Stack, useLocalSearchParams } from "expo-router";
import { useEffect } from "react";
import { StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import { useTheme } from "@/src/theme/useTheme";
import { AurisMark } from "@/src/ui/AurisMark";
import { EmptyState, IconButton } from "@/src/ui/components";

export default function NotFoundScreen() {
  const t = useTheme();
  const params = useLocalSearchParams<{
    error?: string;
    error_description?: string;
  }>();

  const isAuthError = !!(params.error || params.error_description);

  // Redirect inside an effect so the initial render commits first —
  // calling `router.replace` during render trips expo-router's
  // "navigation before mount" warning on some versions.
  useEffect(() => {
    if (!isAuthError) return;
    router.replace({
      pathname: "/login",
      params: {
        error_description: params.error_description ?? params.error ?? "sign-in failed",
      },
    });
  }, [isAuthError, params.error, params.error_description]);

  if (isAuthError) {
    // Render nothing visible while the redirect dispatches — the
    // login screen takes over within a frame.
    return null;
  }

  return (
    <SafeAreaView style={[styles.root, { backgroundColor: t.color.bg.canvas }]}>
      <Stack.Screen options={{ title: "Not found", headerShown: false }} />
      <View style={styles.center}>
        <AurisMark size={56} variant="coral" animate="breathe" />
        <EmptyState
          title="Page not found"
          body="── that route doesn't exist."
          action={
            <IconButton
              glyph="←"
              label="GO BACK"
              onPress={() => {
                if (router.canGoBack()) {
                  router.back();
                } else {
                  router.replace("/");
                }
              }}
            />
          }
        />
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  center: { flex: 1, alignItems: "center", justifyContent: "center" },
});
