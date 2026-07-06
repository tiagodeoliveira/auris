// Editorial tab bar for the past-meeting detail screen. Mono uppercase
// labels with no pill background; the active tab is marked by a coral
// underline that slides between tabs via Reanimated.
//
// The bar is horizontally scrollable so a long set of modes never
// clips; in practice the six current tabs fit on a phone width, but
// the scroll buffer keeps the layout safe on small / tagalog locales.

import { useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, View, type LayoutChangeEvent } from "react-native";
import Animated, {
  Easing,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
} from "react-native-reanimated";

import { useTheme } from "@/src/theme/useTheme";
import { MonoLabel } from "@/src/ui/components";

export interface TabDescriptor {
  id: string;
  label: string;
}

interface TabBarProps {
  tabs: TabDescriptor[];
  activeId: string;
  onSelect: (id: string) => void;
}

interface TabLayout {
  x: number;
  width: number;
}

const UNDERLINE_HEIGHT = 2;

export function TabBar({ tabs, activeId, onSelect }: TabBarProps) {
  const t = useTheme();

  // Track each tab's x/width so the coral underline can slide to the
  // active one. Layouts come in async (each Pressable reports its own
  // onLayout), so the underline animates after the first frame — fine
  // because the user reads the label first anyway.
  const [layouts, setLayouts] = useState<Record<string, TabLayout>>({});

  const underlineX = useSharedValue(0);
  const underlineW = useSharedValue(0);

  // Whenever activeId or any layout changes, retarget the underline.
  // 150ms ease-out matches the rest of the system's interactive moves
  // (Card press, tab body fade-in).
  useEffect(() => {
    const target = layouts[activeId];
    if (!target) return;
    underlineX.value = withTiming(target.x, {
      duration: 150,
      easing: Easing.out(Easing.cubic),
    });
    underlineW.value = withTiming(target.width, {
      duration: 150,
      easing: Easing.out(Easing.cubic),
    });
  }, [activeId, layouts, underlineX, underlineW]);

  const underlineStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: underlineX.value }],
    width: underlineW.value,
  }));

  const onTabLayout = (id: string, e: LayoutChangeEvent) => {
    const { x, width } = e.nativeEvent.layout;
    setLayouts((prev) => {
      const existing = prev[id];
      if (existing && existing.x === x && existing.width === width) return prev;
      return { ...prev, [id]: { x, width } };
    });
  };

  return (
    <View>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={[
          styles.row,
          {
            paddingHorizontal: t.spacing.lg,
            gap: t.spacing.lg,
            // Leave room below for the underline rail; the rail itself
            // is positioned absolute so it doesn't push the labels
            // around as it animates.
            paddingBottom: t.spacing.sm,
          },
        ]}
      >
        {tabs.map((tab) => {
          const active = tab.id === activeId;
          return (
            <Pressable
              key={tab.id}
              onPress={() => onSelect(tab.id)}
              onLayout={(e) => onTabLayout(tab.id, e)}
              style={({ pressed }) => [
                styles.tab,
                { paddingVertical: t.spacing.sm },
                pressed && { opacity: 0.6 },
              ]}
              hitSlop={6}
            >
              <MonoLabel tone={active ? "brand" : "secondary"}>{tab.label}</MonoLabel>
            </Pressable>
          );
        })}

        {/*
          Underline rail — pinned to the bottom of the scroll content
          so it travels with horizontal scroll if tabs ever overflow.
          Coral, 2pt tall.
        */}
        <Animated.View
          pointerEvents="none"
          style={[
            styles.underline,
            { backgroundColor: t.color.brand.coral, height: UNDERLINE_HEIGHT },
            underlineStyle,
          ]}
        />
      </ScrollView>

      {/* Hairline under the bar — gives the editorial feel of a column
          rule below a section masthead. */}
      <View style={[styles.hairline, { backgroundColor: t.color.border.soft }]} />
    </View>
  );
}

const styles = StyleSheet.create({
  row: {
    flexDirection: "row",
    alignItems: "center",
  },
  tab: {
    // No pill background — just text.
    flexDirection: "row",
    alignItems: "center",
  },
  underline: {
    position: "absolute",
    bottom: 0,
    // `left: 0` is correct here. The underline sits inside the
    // ScrollView's contentContainer, which carries the horizontal
    // padding. Each Pressable's `onLayout` reports `x` relative to
    // that same content container — so `x` is already past the
    // padding, and an additional `left` offset would double-count.
    // This matches the live meeting tab bar's geometry exactly (see
    // app/meeting.tsx :: ModeTabsBar).
    left: 0,
    borderRadius: 1,
  },
  hairline: {
    height: StyleSheet.hairlineWidth,
  },
});
