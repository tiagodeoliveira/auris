// MicActivityIcon — mobile analog of the Mac overlay's MicActivityIcon.
// A stylized microphone shape whose interior fills proportional to the
// instantaneous audio peak. Recording state colors the outline coral;
// idle stays muted so the icon doubles as a mute/active indicator.
//
// Geometry (referenced to a 48-unit canvas; scaled to `size`):
//   Capsule body     width 48%, height 58%, top at 12% from top
//   Inner fill       rises from capsule bottom, height = max(16%, peak)
//   Yoke (cup arc)   cubic bezier beneath capsule, stroke 2.4pt
//   Stand            small vertical capsule, width 6%, height 12%
//
// Fill height transitions over 80ms (linear) when `peak` changes —
// mirrors the Mac VU's snappy decay without flicker.

import { useEffect } from "react";
import { View, type ViewProps } from "react-native";
import Animated, {
  cancelAnimation,
  Easing,
  useAnimatedProps,
  useSharedValue,
  withTiming,
} from "react-native-reanimated";
import Svg, { ClipPath, Defs, G, Path, Rect } from "react-native-svg";

import { useTheme } from "@/src/theme/useTheme";

const AnimatedRect = Animated.createAnimatedComponent(Rect);

export interface MicActivityIconProps extends Pick<ViewProps, "style" | "accessibilityLabel"> {
  size: number;
  /** Audio level, clamped to 0..1. */
  peak: number;
  /** When true, outline + fill are coral; otherwise muted. */
  isRecording: boolean;
  /** Overlays a pause glyph if true (recording can be paused). */
  isPaused?: boolean;
}

export function MicActivityIcon({
  size,
  peak,
  isRecording,
  isPaused = false,
  style,
  accessibilityLabel,
}: MicActivityIconProps) {
  const t = useTheme();

  // Layout in 48-unit space. Capsule centered horizontally.
  const W = 48;
  const capW = W * 0.48; // 23.04
  const capH = W * 0.58; // 27.84
  const capX = (W - capW) / 2; // 12.48
  const capY = W * 0.12; // 5.76
  const capR = capW / 2;

  const yokeStrokeW = 2.4;
  const yokeY = capY + capH; // top of yoke arc start
  // Yoke arc — a downward "U" cradling the capsule bottom.
  const yokeLeftX = capX - W * 0.05;
  const yokeRightX = capX + capW + W * 0.05;
  const yokeBottomY = yokeY + W * 0.12;
  const yokeMidY = yokeBottomY;
  const yokePath = `M ${yokeLeftX} ${yokeY} C ${yokeLeftX} ${yokeBottomY}, ${yokeRightX} ${yokeBottomY}, ${yokeRightX} ${yokeY}`;

  // Stand — short vertical capsule below the yoke.
  const standW = W * 0.06;
  const standH = W * 0.12;
  const standX = (W - standW) / 2;
  const standY = yokeMidY;

  // Outline color: coral when recording (any peak), muted otherwise.
  const outline = isRecording ? t.color.brand.coral : t.color.text.muted;
  // Fill color: same logic, but a slightly more saturated coral so
  // it pops inside the capsule.
  const fill = isRecording ? t.color.brand.coral : t.color.text.muted;

  // Clamp peak and convert into an SVG-space fill height. Floor at
  // 16% so the capsule never looks empty mid-meeting.
  const clamped = Math.max(0, Math.min(1, peak));
  const minFill = capH * 0.16;
  const targetFillH = Math.max(minFill, capH * clamped);

  // Animate fill height with linear 80ms (mirrors Mac).
  const fillH = useSharedValue(targetFillH);
  useEffect(() => {
    fillH.value = withTiming(targetFillH, { duration: 80, easing: Easing.linear });
    return () => cancelAnimation(fillH);
  }, [targetFillH, fillH]);

  // SVG <Rect> animated props: `y` and `height` shift in lockstep so
  // the fill rises from the capsule bottom (y = capY + capH - h).
  const fillProps = useAnimatedProps(() => ({
    y: capY + capH - fillH.value,
    height: fillH.value,
  }));

  // The capsule fill is clipped to the rounded-rect interior so the
  // rising rectangle never extends past the capsule silhouette.
  const clipId = "mic-cap-clip";

  // Scale all geometry by (size / 48). Easiest path: use the SVG's
  // viewBox so the unit math above stays readable.
  return (
    <View style={[{ width: size, height: size }, style]}>
      <Svg
        width={size}
        height={size}
        viewBox={`0 0 ${W} ${W}`}
        accessibilityLabel={
          accessibilityLabel ?? (isRecording ? "Microphone active" : "Microphone idle")
        }
      >
        <Defs>
          <ClipPath id={clipId}>
            <Rect x={capX} y={capY} width={capW} height={capH} rx={capR} ry={capR} />
          </ClipPath>
        </Defs>

        {/* Capsule body — outline only, fill provided via clipped rect below. */}
        <Rect
          x={capX}
          y={capY}
          width={capW}
          height={capH}
          rx={capR}
          ry={capR}
          fill="none"
          stroke={outline}
          strokeWidth={2.4}
        />

        {/* Animated fill — clipped to the capsule silhouette. */}
        <G clipPath={`url(#${clipId})`}>
          <AnimatedRect
            x={capX}
            width={capW}
            fill={fill}
            opacity={0.85}
            animatedProps={fillProps}
          />
        </G>

        {/* Yoke (cup beneath the capsule). */}
        <Path
          d={yokePath}
          fill="none"
          stroke={outline}
          strokeWidth={yokeStrokeW}
          strokeLinecap="round"
        />

        {/* Stand */}
        <Rect
          x={standX}
          y={standY}
          width={standW}
          height={standH}
          rx={standW / 2}
          ry={standW / 2}
          fill={outline}
        />

        {/* Pause glyph — two short bars over the capsule, drawn only
            when paused. Uses the canvas background as the bar fill so
            it reads as a cut-out, not an overlay color. */}
        {isPaused && (
          <G>
            <Rect
              x={capX + capW * 0.28}
              y={capY + capH * 0.3}
              width={capW * 0.12}
              height={capH * 0.4}
              rx={1}
              fill={t.color.bg.canvas}
            />
            <Rect
              x={capX + capW * 0.6}
              y={capY + capH * 0.3}
              width={capW * 0.12}
              height={capH * 0.4}
              rx={1}
              fill={t.color.bg.canvas}
            />
          </G>
        )}
      </Svg>
    </View>
  );
}
