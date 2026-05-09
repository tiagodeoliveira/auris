// Root layout. Bootstraps auth on first mount and gates the tab
// surface behind a signed-in identity. While auth is bootstrapping
// we render nothing (avoids the "flash login → flash compose"
// double-render every time the app starts with a valid refresh
// token persisted).

import { Redirect, Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { useEffect } from "react";

import { useAppStore } from "@/src/store";

export default function RootLayout() {
  const bootstrap = useAppStore((s) => s.bootstrap);
  const authBootstrapped = useAppStore((s) => s.authBootstrapped);
  const identity = useAppStore((s) => s.identity);
  const connect = useAppStore((s) => s.connect);

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  // Auto-connect the WS once auth lands. The store's connect() is
  // idempotent — calling again on identity changes is fine.
  useEffect(() => {
    if (identity) connect();
  }, [identity, connect]);

  if (!authBootstrapped) {
    return null;
  }

  return (
    <>
      <StatusBar style="auto" />
      <Stack>
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen name="login" options={{ presentation: "modal", title: "Sign in" }} />
      </Stack>
      {!identity && <Redirect href="/login" />}
    </>
  );
}
