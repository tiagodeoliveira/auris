// AurisMark — the brand mark, rendered live as SVG so it can scale
// without raster blur and animate without sprite sheets.
//
// Geometry is lifted verbatim from `assets/branding/icon-{coral,
// light,dark}.svg`:
//
//   Viewbox       96 × 96
//   Mark group    translated by (34, 28)
//   Outer arc     M 22  4 A 18 18 0 0 0 22 40   stroke 4.5  round caps
//   Inner arc     M 22 12 A 10 10 0 0 0 22 32   stroke 4.5  round caps
//   Focal dot     cx 16  cy 22  r 3
//
// The mark is a left-facing ear silhouette: two concentric arcs
// (the ear's helix) with a focal dot at the tragus position. Every
// downstream use — splash hero, tab icon, loading state — composes
// from this single source.
//
// Variants mirror the SVG files in branding/:
//   coral  -> coral background, slate arcs, cream dot   (primary)
//   slate  -> slate background, cream arcs, coral dot
//   cream  -> cream background, slate arcs, coral dot
//   mono   -> no background, arcs use currentColor, dot is coral
//
// Animations (mutually exclusive):
//   breathe -> dot opacity pulses 0.5↔1.0 over `duration.breathe`
//   ripple  -> arcs scale + fade outward in a continuous loop
//   spin    -> rotates 360° / 1.5s — pull-to-refresh

import { useEffect } from "react";
import { View, type ViewProps } from "react-native";
import Animated, {
  cancelAnimation,
  Easing,
  useAnimatedProps,
  useAnimatedStyle,
  useSharedValue,
  withRepeat,
  withSequence,
  withTiming,
} from "react-native-reanimated";
import Svg, { Circle, G, Path, Rect } from "react-native-svg";

import { duration, useBreathe } from "@/src/theme/motion";
import { useTheme } from "@/src/theme/useTheme";

const AnimatedG = Animated.createAnimatedComponent(G);
const AnimatedCircle = Animated.createAnimatedComponent(Circle);

export interface AurisMarkProps extends Pick<ViewProps, "style" | "accessibilityLabel"> {
  /** Pixel size — the mark is always square. */
  size: number;
  /**
   * Color scheme. `coral` is the canonical app icon; `mono` is for
   * inline use in text (arcs inherit `color` from the surrounding
   * context, dot stays coral).
   */
  variant?: "coral" | "slate" | "cream" | "mono";
  /**
   * Render the rounded-square background plate. Default true.
   * Set false for inline glyph-style use (e.g. tab labels).
   */
  background?: boolean;
  /**
   * Animation mode. Only one can run at a time:
   *   - `none`    : static
   *   - `breathe` : dot opacity pulses — loading / listening
   *   - `ripple`  : arcs scale + fade outward — active recording
   *   - `spin`    : 360° rotation — pull-to-refresh
   */
  animate?: "none" | "breathe" | "ripple" | "spin";
  /**
   * For `mono` variant only: overrides currentColor for the arcs.
   * Ignored for the other variants (their arc color is fixed).
   */
  color?: string;
}

interface Palette {
  bg: string | null;
  arc: string;
  dot: string;
}

function resolvePalette(
  variant: NonNullable<AurisMarkProps["variant"]>,
  brand: { coral: string; slate: string; cream: string },
  monoColor: string,
): Palette {
  switch (variant) {
    case "coral":
      return { bg: brand.coral, arc: brand.slate, dot: brand.cream };
    case "slate":
      return { bg: brand.slate, arc: brand.cream, dot: brand.coral };
    case "cream":
      return { bg: brand.cream, arc: brand.slate, dot: brand.coral };
    case "mono":
      return { bg: null, arc: monoColor, dot: brand.coral };
  }
}

export function AurisMark({
  size,
  variant = "coral",
  background = true,
  animate = "none",
  color,
  style,
  accessibilityLabel = "Auris",
}: AurisMarkProps) {
  const t = useTheme();
  const palette = resolvePalette(variant, t.color.brand, color ?? t.color.text.primary);

  // Hooks must run unconditionally; gate effects by `animate` inside.
  const { opacity: breatheOpacity } = useBreathe(animate === "breathe");

  // Ripple: outer arc scales 1.0 → 1.2 while fading 1 → 0, looping.
  // Implemented as a transform on the inner <G> wrapping the arcs.
  const rippleScale = useSharedValue(1);
  const rippleOpacity = useSharedValue(1);
  useEffect(() => {
    if (animate !== "ripple") {
      cancelAnimation(rippleScale);
      cancelAnimation(rippleOpacity);
      rippleScale.value = 1;
      rippleOpacity.value = 1;
      return;
    }
    rippleScale.value = withRepeat(
      withSequence(
        withTiming(1.18, { duration: duration.ripple, easing: Easing.out(Easing.cubic) }),
        withTiming(1.0, { duration: 0 }),
      ),
      -1,
      false,
    );
    rippleOpacity.value = withRepeat(
      withSequence(
        withTiming(0.0, { duration: duration.ripple, easing: Easing.out(Easing.cubic) }),
        withTiming(1.0, { duration: 0 }),
      ),
      -1,
      false,
    );
    return () => {
      cancelAnimation(rippleScale);
      cancelAnimation(rippleOpacity);
    };
  }, [animate, rippleScale, rippleOpacity]);

  // Spin: 360° / 1500ms, ease-linear, looping.
  const spinDeg = useSharedValue(0);
  useEffect(() => {
    if (animate !== "spin") {
      cancelAnimation(spinDeg);
      spinDeg.value = 0;
      return;
    }
    spinDeg.value = withRepeat(
      withTiming(360, { duration: 1500, easing: Easing.linear }),
      -1,
      false,
    );
    return () => cancelAnimation(spinDeg);
  }, [animate, spinDeg]);

  // Apply spin to the outer <View>; ripple/breathe stay inside the
  // SVG so they only affect the mark, not the background plate.
  const spinStyle = useAnimatedStyle(() => ({
    transform: [{ rotate: `${spinDeg.value}deg` }],
  }));

  // Ripple animates the arcs group as a whole.
  const rippleArcProps = useAnimatedProps(() => ({
    opacity: rippleOpacity.value,
    // SVG group transform: translate to mark origin, scale, translate
    // back. Mark center in mark-local coords is roughly (22, 22).
    transform: `translate(22 22) scale(${rippleScale.value}) translate(-22 -22)`,
  }));

  // Breathe animates the dot's opacity only.
  const dotProps = useAnimatedProps(() => ({
    opacity: animate === "breathe" ? breatheOpacity.value : 1,
  }));

  const bgRadius = background ? size * 0.16 : 0; // matches Mac rounded-square
  // SVG content scales itself to the requested pixel size via the
  // viewBox; we keep the geometry in the original 96-unit space.
  const Content = (
    <Svg width={size} height={size} viewBox="0 0 96 96" accessibilityLabel={accessibilityLabel}>
      {background && palette.bg && (
        <Rect x={0} y={0} width={96} height={96} rx={bgRadius * (96 / size)} fill={palette.bg} />
      )}
      <G transform="translate(34 28)">
        <AnimatedG animatedProps={rippleArcProps}>
          <Path
            d="M 22 4 A 18 18 0 0 0 22 40"
            fill="none"
            stroke={palette.arc}
            strokeWidth={4.5}
            strokeLinecap="round"
          />
          <Path
            d="M 22 12 A 10 10 0 0 0 22 32"
            fill="none"
            stroke={palette.arc}
            strokeWidth={4.5}
            strokeLinecap="round"
          />
        </AnimatedG>
        <AnimatedCircle cx={16} cy={22} r={3} fill={palette.dot} animatedProps={dotProps} />
      </G>
    </Svg>
  );

  // Spin needs the View wrapper to rotate the whole composition;
  // for non-spin modes we skip the Animated.View to keep the tree
  // a touch lighter.
  if (animate === "spin") {
    return (
      <Animated.View style={[{ width: size, height: size }, spinStyle, style]}>
        {Content}
      </Animated.View>
    );
  }
  return <View style={[{ width: size, height: size }, style]}>{Content}</View>;
}
