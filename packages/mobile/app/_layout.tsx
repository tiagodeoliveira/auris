// Root layout. Bootstraps auth on first mount and gates the tab
// surface behind a signed-in identity. While auth or fonts are
// bootstrapping we render nothing (avoids the "flash login → flash
// compose" double-render every time the app starts with a valid
// refresh token persisted).
//
// Font loading: Space Grotesk + JetBrains Mono + Bebas Neue back the
// entire type ramp in `theme/tokens.ts`. Strings used here MUST match
// the family names in tokens.ts exactly — a typo will silently fall
// back to the system font and break the look across the app.

import { BebasNeue_400Regular, useFonts as useBebasNeue } from "@expo-google-fonts/bebas-neue";
import {
  JetBrainsMono_400Regular,
  JetBrainsMono_500Medium,
  JetBrainsMono_600SemiBold,
  useFonts as useJetBrainsMono,
} from "@expo-google-fonts/jetbrains-mono";
import {
  SpaceGrotesk_400Regular,
  SpaceGrotesk_500Medium,
  SpaceGrotesk_600SemiBold,
  SpaceGrotesk_700Bold,
  useFonts as useSpaceGrotesk,
} from "@expo-google-fonts/space-grotesk";
import { Redirect, Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import * as SystemUI from "expo-system-ui";
import { useEffect } from "react";
import { GestureHandlerRootView } from "react-native-gesture-handler";

import { useAppStore } from "@/src/store";
import { useTheme } from "@/src/theme/useTheme";

export default function RootLayout() {
  const bootstrap = useAppStore((s) => s.bootstrap);
  const authBootstrapped = useAppStore((s) => s.authBootstrapped);
  const identity = useAppStore((s) => s.identity);
  const connect = useAppStore((s) => s.connect);
  // Resolve the active canvas color so we can paint the native root
  // window background to match. Without this, Android shows a brief
  // white flash on cold-start in dark mode while the JS bundle boots
  // and the React tree hasn't yet painted over the system default.
  const theme = useTheme();
  const canvasColor = theme.color.bg.canvas;

  // Load brand fonts. Three useFonts calls (one per family) because
  // the @expo-google-fonts packages export one hook per family. All
  // three must resolve before we render the tree; otherwise Text
  // styled with these families falls back to system, then snaps
  // when the fonts arrive — visible jank.
  const [spaceGroteskLoaded] = useSpaceGrotesk({
    SpaceGrotesk_400Regular,
    SpaceGrotesk_500Medium,
    SpaceGrotesk_600SemiBold,
    SpaceGrotesk_700Bold,
  });
  const [jetBrainsMonoLoaded] = useJetBrainsMono({
    JetBrainsMono_400Regular,
    JetBrainsMono_500Medium,
    JetBrainsMono_600SemiBold,
  });
  const [bebasNeueLoaded] = useBebasNeue({ BebasNeue_400Regular });

  const fontsLoaded = spaceGroteskLoaded && jetBrainsMonoLoaded && bebasNeueLoaded;

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  // Auto-connect the WS once auth lands. The store's connect() is
  // idempotent — calling again on identity changes is fine.
  useEffect(() => {
    if (identity) connect();
  }, [identity, connect]);

  // Repaint the native root window whenever the canvas color changes
  // (OS dark-mode flip OR an explicit themeOverride from Settings).
  // No-op on iOS where the StatusBar/SafeAreaView dance already
  // covers the window background; load-bearing on Android.
  useEffect(() => {
    void SystemUI.setBackgroundColorAsync(canvasColor).catch((e: unknown) => {
      console.warn("[layout] SystemUI.setBackgroundColorAsync failed:", e);
    });
  }, [canvasColor]);

  if (!authBootstrapped || !fontsLoaded) {
    return null;
  }

  return (
    // GestureHandlerRootView is required for react-native-gesture-handler
    // gestures (pinch-zoom on moment screenshots) to fire. Without it,
    // gestures register but never emit events. flex:1 so it fills the
    // root window.
    <GestureHandlerRootView style={{ flex: 1 }}>
      <StatusBar style="auto" />
      <Stack
        // Theme the native nav header so drill-in screens
        // (meeting/[id], artifact/[id]) don't render a light strip
        // above a dark body. screenOptions feeds every child Stack.Screen
        // unless overridden per screen.
        screenOptions={{
          headerStyle: { backgroundColor: canvasColor },
          headerTintColor: theme.color.text.primary,
          headerTitleStyle: { color: theme.color.text.primary },
          headerShadowVisible: false,
        }}
      >
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen name="login" options={{ presentation: "modal", title: "Sign in" }} />
        <Stack.Screen name="pair" options={{ presentation: "modal", title: "Pair glasses" }} />
        <Stack.Screen
          name="meeting"
          options={{
            presentation: "fullScreenModal",
            // The live meeting screen renders its own in-screen header
            // (the "TESTING WITH …" strip with elapsed timer) above the
            // mode tabs. The native nav header here would just duplicate
            // that with a generic "Meeting" label; hiding it lets the
            // SafeAreaView paint the notch area with the theme canvas
            // and removes the light strip across the top.
            headerShown: false,
            // gestureEnabled: false so a stray swipe-down doesn't
            // dismiss the meeting view while it's live. Server-side
            // stop is the canonical exit — see meeting.tsx's
            // useEffect on meetingState === "idle".
            gestureEnabled: false,
          }}
        />
        <Stack.Screen
          name="meeting/[id]"
          options={{
            // Not a modal — pushes onto the tab stack like a normal
            // drill-in. Header back button returns to History.
            title: "Meeting",
            // Override the default expo-router back label (which
            // would render the literal segment name "(tabs)").
            headerBackTitle: "History",
          }}
        />
        <Stack.Screen
          name="artifact/[id]"
          options={{
            title: "Artifact",
            headerBackTitle: "Artifacts",
          }}
        />
      </Stack>
      {!identity && <Redirect href="/login" />}
    </GestureHandlerRootView>
  );
}
