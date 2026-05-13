// Tab bar shape per MOBILE-PLAN §7. Order matches the user's
// expected day-to-day flow: start a meeting, browse past ones,
// manage the artifact library, tweak settings.

import { Ionicons } from "@expo/vector-icons";
import { Tabs } from "expo-router";

// Auris coral, used as the active-tab tint.
const ACTIVE_TINT = "#d97757";

export default function TabLayout() {
  return (
    <Tabs
      screenOptions={{
        headerShown: true,
        tabBarActiveTintColor: ACTIVE_TINT,
      }}
    >
      <Tabs.Screen
        name="index"
        options={{
          title: "Compose",
          tabBarIcon: ({ focused, color, size }) => (
            <Ionicons name={focused ? "create" : "create-outline"} size={size} color={color} />
          ),
        }}
      />
      <Tabs.Screen
        name="history"
        options={{
          title: "History",
          tabBarIcon: ({ focused, color, size }) => (
            <Ionicons name={focused ? "time" : "time-outline"} size={size} color={color} />
          ),
        }}
      />
      <Tabs.Screen
        name="artifacts"
        options={{
          title: "Artifacts",
          tabBarIcon: ({ focused, color, size }) => (
            <Ionicons
              name={focused ? "document-text" : "document-text-outline"}
              size={size}
              color={color}
            />
          ),
        }}
      />
      <Tabs.Screen
        name="settings"
        options={{
          title: "Settings",
          tabBarIcon: ({ focused, color, size }) => (
            <Ionicons name={focused ? "settings" : "settings-outline"} size={size} color={color} />
          ),
        }}
      />
    </Tabs>
  );
}
