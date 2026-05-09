// Tab bar shape per MOBILE-PLAN §7. Order matches the user's
// expected day-to-day flow: start a meeting, browse past ones,
// manage the artifact library, tweak settings.

import { Tabs } from "expo-router";

export default function TabLayout() {
  return (
    <Tabs screenOptions={{ headerShown: true }}>
      <Tabs.Screen name="index" options={{ title: "Compose" }} />
      <Tabs.Screen name="history" options={{ title: "History" }} />
      <Tabs.Screen name="artifacts" options={{ title: "Artifacts" }} />
      <Tabs.Screen name="settings" options={{ title: "Settings" }} />
    </Tabs>
  );
}
