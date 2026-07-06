// Motion utilities — durations, easings, and the handful of
// animation patterns the design system uses repeatedly. Subsequent
// agents consume these directly so every fade/press/breathe across
// the app moves in lockstep.
//
// Built on react-native-reanimated v3. The shared-value pattern (vs.
// the legacy Animated API) keeps animations on the UI thread, which
// is what gives the "breathe" and "ripple" patterns their unbroken
// 60fps feel on low-end Androids.

import { useEffect } from "react";
import {
  cancelAnimation,
  Easing,
  useSharedValue,
  withDelay,
  withRepeat,
  withSequence,
  withSpring,
  withTiming,
  type SharedValue,
  type WithSpringConfig,
} from "react-native-reanimated";

// Canonical durations. `breathe` and `ripple` are deliberately long
// — they're meditative loops, not feedback flickers. `fast`/`medium`
// are the standard interaction durations; pick `medium` unless the
// motion feels sluggish at that speed.
export const duration = {
  instant: 0,
  fast: 120,
  medium: 200,
  slow: 320,
  breathe: 1400,
  ripple: 1800,
} as const;

// Easing curves. `standard` is the everyday curve (ease-out cubic);
// `enter` / `exit` are the asymmetric pair for content appearing or
// leaving (material-style). `spring` is the gentle spring config
// — pass it as a second arg to `withSpring(value, easing.spring)`.
export const easing = {
  standard: Easing.out(Easing.cubic),
  enter: Easing.out(Easing.cubic),
  exit: Easing.in(Easing.cubic),
  // Spring config — damping/stiffness tuned to settle in ~250ms
  // with a hair of overshoot. Used for press feedback and modal
  // entrances.
  spring: {
    damping: 18,
    stiffness: 220,
    mass: 1,
  } satisfies WithSpringConfig,
} as const;

// ----------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------

/**
 * Breathing opacity loop — pulses 0.5 → 1.0 → 0.5 over `duration.breathe`,
 * repeating forever while `active` is true. When `active` flips to false,
 * the animation cancels and the opacity is held at 1.
 *
 * Used by AurisMark's `breathe` variant and anywhere a loading state
 * wants a calmer alternative to a spinner.
 */
export function useBreathe(active: boolean): { opacity: SharedValue<number> } {
  const opacity = useSharedValue(1);

  useEffect(() => {
    if (active) {
      // Half-cycle is `breathe / 2` so a full down-up-down trip
      // totals `breathe` ms.
      const half = duration.breathe / 2;
      opacity.value = withRepeat(
        withSequence(
          withTiming(0.5, { duration: half, easing: easing.standard }),
          withTiming(1.0, { duration: half, easing: easing.standard }),
        ),
        -1,
        false,
      );
    } else {
      cancelAnimation(opacity);
      opacity.value = withTiming(1, { duration: duration.fast });
    }
    return () => cancelAnimation(opacity);
  }, [active, opacity]);

  return { opacity };
}

/**
 * Press-feedback scale. Returns a shared scale + onPressIn/onPressOut
 * handlers ready to drop on a Pressable. The scale dips to 0.96 on
 * press-in (spring) and returns to 1 on press-out.
 *
 * Use this in place of the default Pressable opacity feedback when
 * you want the press to feel physical — Cards, primary CTAs, the
 * AurisMark, etc.
 */
export function usePressFeedback(): {
  scale: SharedValue<number>;
  onPressIn: () => void;
  onPressOut: () => void;
} {
  const scale = useSharedValue(1);
  const onPressIn = () => {
    scale.value = withSpring(0.96, easing.spring);
  };
  const onPressOut = () => {
    scale.value = withSpring(1, easing.spring);
  };
  return { scale, onPressIn, onPressOut };
}

/**
 * Fade-in on mount. Returns a shared opacity that animates from 0 to
 * 1 once the component mounts, with an optional delay. Useful for
 * staggered list reveals and hero entrances.
 */
export function useFadeInOnMount(delayMs: number = 0): { opacity: SharedValue<number> } {
  const opacity = useSharedValue(0);
  useEffect(() => {
    opacity.value = withDelay(
      delayMs,
      withTiming(1, { duration: duration.medium, easing: easing.enter }),
    );
    return () => cancelAnimation(opacity);
    // Run once on mount — delay change shouldn't retrigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return { opacity };
}
