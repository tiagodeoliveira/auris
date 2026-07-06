// Haptics helper — thin defensive wrapper around expo-haptics.
//
// Why this layer:
//   - The Taptic Engine is unreliable on the iOS Simulator (calls
//     succeed but produce no feedback) and entirely absent on web /
//     older Android devices. expo-haptics throws on unsupported
//     surfaces in some SDK versions; in others it silently no-ops.
//     We wrap every call in a try/catch so a missing engine can't
//     bubble up and break the action it was meant to garnish.
//   - Centralizing the imports also lets the call sites stay terse:
//     `haptics.success()` reads more like brand voice than the
//     verbose Haptics.NotificationFeedbackType enum.
//
// Mapping (matches the design-pass spec):
//   success  → notificationAsync(Success)  — meeting starts, etc.
//   warning  → notificationAsync(Warning)  — stop confirm, delete confirm
//   medium   → impactAsync(Medium)         — stop armed
//   light    → impactAsync(Light)          — moment, extract tags
//   select   → selectionAsync              — pause/resume, theme pick, chat send
//
// Each call is fire-and-forget; we deliberately don't await so the UI
// thread isn't blocked by the haptic. Errors are swallowed silently
// — haptics are decoration, not correctness.

import * as Haptics from "expo-haptics";

function safe(fn: () => Promise<unknown>): void {
  try {
    void fn().catch(() => {
      // ignore — simulator / unsupported devices throw or reject here
    });
  } catch {
    // ignore — synchronous throw on unsupported platforms
  }
}

export const haptics = {
  success: () => safe(() => Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success)),
  warning: () => safe(() => Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning)),
  medium: () => safe(() => Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium)),
  light: () => safe(() => Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light)),
  select: () => safe(() => Haptics.selectionAsync()),
};
