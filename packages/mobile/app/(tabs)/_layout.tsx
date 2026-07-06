// Tab bar shape per MOBILE-PLAN §7. Order matches the user's
// expected day-to-day flow: start a meeting, browse past ones,
// manage the artifact library, tweak settings.
//
// Phase G — "Listening Room" tab bar: brand mark on the START tab
// (live SVG, coral when active), Ionicons elsewhere, coral active
// tint, mono-uppercase labels, theme-aware surfaces.

import { Ionicons } from "@expo/vector-icons";
import { Tabs } from "expo-router";

import { useTheme } from "@/src/theme/useTheme";
import { AurisMark } from "@/src/ui/AurisMark";

export default function TabLayout() {
  const t = useTheme();

  // The brand mark replaces the generic "create" icon on the
  // compose tab. Always mono geometry so it matches the outline
  // rhythm of the other tab icons; arcs take the active/inactive
  // tint via `color`, so focused = coral arcs (visible on dark
  // canvas, no plate needed), unfocused = secondary text color.
  const composeIcon = ({ focused, size }: { focused: boolean; size: number }) => (
    <AurisMark
      size={size ?? 24}
      variant="mono"
      background={false}
      color={focused ? t.color.brand.coral : t.color.text.secondary}
      animate="none"
    />
  );

  return (
    <Tabs
      screenOptions={{
        headerShown: false,
        tabBarActiveTintColor: t.color.brand.coral,
        tabBarInactiveTintColor: t.color.text.secondary,
        tabBarStyle: {
          backgroundColor: t.color.bg.elevated,
          borderTopColor: t.color.border.hairline,
          borderTopWidth: 1,
        },
        // Mono-uppercase treatment matches the MonoLabel recipe:
        // JetBrainsMono medium, tight letterSpacing, 10pt. Color
        // is driven by tabBarActive/InactiveTintColor above.
        tabBarLabelStyle: {
          fontFamily: t.font.monoMedium,
          fontSize: 10,
          letterSpacing: 0.6,
        },
      }}
    >
      <Tabs.Screen
        name="index"
        options={{
          title: "START",
          tabBarIcon: composeIcon,
        }}
      />
      <Tabs.Screen
        name="history"
        options={{
          title: "HISTORY",
          tabBarIcon: ({ focused, color, size }) => (
            <Ionicons name={focused ? "time" : "time-outline"} size={size} color={color} />
          ),
        }}
      />
      <Tabs.Screen
        name="artifacts"
        options={{
          title: "ARTIFACTS",
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
          title: "SETTINGS",
          tabBarIcon: ({ focused, color, size }) => (
            <Ionicons name={focused ? "settings" : "settings-outline"} size={size} color={color} />
          ),
        }}
      />
    </Tabs>
  );
}
