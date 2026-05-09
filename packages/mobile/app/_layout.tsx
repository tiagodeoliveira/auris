// Root layout. Wraps the whole app in a Stack so we can push a
// modal sign-in screen above the tab bar when there's no active
// session. Auth gating itself lands in Phase 1 (MOBILE-PLAN §6.1);
// for now both the (tabs) group and the login screen are reachable.

import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";

export default function RootLayout() {
  return (
    <>
      <StatusBar style="auto" />
      <Stack>
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen name="login" options={{ presentation: "modal", title: "Sign in" }} />
      </Stack>
    </>
  );
}
