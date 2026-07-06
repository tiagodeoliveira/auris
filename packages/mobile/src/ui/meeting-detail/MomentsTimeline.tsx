// Vertical timeline of `Moment`s captured during a meeting.
//
// Visual treatment: a single coral rail runs the height of the
// section; each moment is anchored to the rail with a coral focal dot
// and the body sits to its right — timestamp + kind chip on the first
// line, optional note + summary below.
//
// Section heading is a MonoLabel above a coral hairline so the
// timeline reads as a chapter break rather than a generic list.

import { useEffect, useState } from "react";
import { Image, Modal, Pressable, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { Gesture, GestureDetector } from "react-native-gesture-handler";
import Animated, {
  FadeInDown,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
} from "react-native-reanimated";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { useTheme } from "@/src/theme/useTheme";
import { deriveApiBase } from "@/src/wire/meetings-api";
import type { Moment } from "@/src/wire/meetings-api";
import { AurisMark } from "@/src/ui/AurisMark";
import { Chip, MonoLabel } from "@/src/ui/components";

interface MomentsTimelineProps {
  moments: Moment[];
}

export function MomentsTimeline({ moments }: MomentsTimelineProps) {
  const t = useTheme();

  if (moments.length === 0) {
    return (
      <View
        style={[
          styles.empty,
          {
            paddingHorizontal: t.spacing.xxl,
            paddingVertical: t.spacing.xxxl,
            gap: t.spacing.sm,
          },
        ]}
      >
        <View style={{ marginBottom: t.spacing.sm }}>
          <AurisMark size={56} variant="mono" background={false} animate="breathe" />
        </View>
        <Text
          style={{
            ...t.type.subtitle,
            color: t.color.text.primary,
            textAlign: "center",
          }}
        >
          no moments captured
        </Text>
        <Text
          style={{
            ...t.type.body,
            color: t.color.text.secondary,
            textAlign: "center",
          }}
        >
          ── pin a moment during the meeting
        </Text>
      </View>
    );
  }

  return (
    <View
      style={{
        paddingHorizontal: t.spacing.lg,
        paddingTop: t.spacing.lg,
        paddingBottom: t.spacing.xl,
      }}
    >
      <View style={[styles.header, { marginBottom: t.spacing.md }]}>
        <MonoLabel>MOMENTS</MonoLabel>
        <View style={[styles.headerRule, { backgroundColor: t.color.brand.coral }]} />
      </View>

      <View>
        {moments.map((m, idx) => (
          <Animated.View
            key={m.id}
            entering={FadeInDown.delay(idx * 40).duration(220)}
            style={[styles.row, { gap: t.spacing.md }]}
          >
            <View style={[styles.gutter, { width: GUTTER }]}>
              {/* Rail — drawn full-height of the row; the last row's
                  rail is faded so it tapers off rather than running
                  into the next section flush. */}
              <View
                style={[
                  styles.rail,
                  {
                    backgroundColor: t.color.brand.coral,
                    opacity: idx === moments.length - 1 ? 0.25 : 1,
                  },
                ]}
              />
              <View style={[styles.dot, { backgroundColor: t.color.brand.coral }]} />
            </View>
            <View style={[styles.body, { paddingBottom: t.spacing.xl, gap: t.spacing.xs }]}>
              {m.screenshot_url ? (
                <MomentScreenshot
                  url={m.screenshot_url}
                  radius={t.radius.md}
                  border={t.color.border.soft}
                />
              ) : null}
              <View style={[styles.bodyHead, { gap: t.spacing.sm }]}>
                <Text
                  style={{
                    ...t.type.monoMedium,
                    color: t.color.text.secondary,
                  }}
                >
                  {formatT(m.t)}
                </Text>
                <Chip label={m.kind.toUpperCase()} tone="brand" size="sm" />
              </View>
              {m.note ? (
                <Text
                  style={{
                    ...t.type.body,
                    color: t.color.text.primary,
                  }}
                >
                  {m.note}
                </Text>
              ) : null}
              {m.summary ? (
                <Text
                  style={{
                    ...t.type.bodySmall,
                    color: t.color.text.secondary,
                  }}
                >
                  {m.summary}
                </Text>
              ) : null}
            </View>
          </Animated.View>
        ))}
      </View>
    </View>
  );
}

/// Authed thumbnail for a moment screenshot. The server requires a
/// bearer header on /meetings/.../screenshot/..., and RN's `Image`
/// honors `source.headers` directly on iOS + Android — no blob-URL
/// dance needed (unlike the PWA, where browsers can't pass auth
/// headers on `<img>` and we must fetch + objectURL).
function MomentScreenshot({
  url,
  radius,
  border,
}: {
  url: string;
  radius: number;
  border: string;
}) {
  const [token, setToken] = useState<string | null>(null);
  const [lightboxOpen, setLightboxOpen] = useState(false);
  const baseUrl = deriveApiBase(serverUrl);

  useEffect(() => {
    let cancelled = false;
    void auth0
      .getAccessToken()
      .then((tok) => {
        if (!cancelled) setToken(tok);
      })
      .catch(() => {
        // Token fetch failed — leave token null so we render nothing.
        // The moment body still shows timestamp + note + summary.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!baseUrl || !token) return null;

  const fullUri = baseUrl + url;
  const authHeaders = { Authorization: `Bearer ${token}` };

  return (
    <>
      <Pressable onPress={() => setLightboxOpen(true)}>
        <Image
          source={{ uri: fullUri, headers: authHeaders }}
          style={{
            width: "100%",
            aspectRatio: 16 / 10,
            borderRadius: radius,
            borderWidth: 1,
            borderColor: border,
            marginBottom: 6,
          }}
          resizeMode="cover"
        />
      </Pressable>
      {lightboxOpen ? (
        <MomentLightbox
          uri={fullUri}
          headers={authHeaders}
          onClose={() => setLightboxOpen(false)}
        />
      ) : null}
    </>
  );
}

/// Fullscreen lightbox with pinch + pan gestures. Reanimated worklets
/// run the transforms off the JS thread so zooming stays at 60fps
/// even when the main React tree is busy. Tap the backdrop or the ✕
/// to dismiss; tap the image itself does nothing (the GestureDetector
/// swallows it so a stray pinch end-tap won't dismiss).
function MomentLightbox({
  uri,
  headers,
  onClose,
}: {
  uri: string;
  headers: { Authorization: string };
  onClose: () => void;
}) {
  const { width, height } = useWindowDimensions();

  // Shared values drive an animated transform on the image. `scale`
  // is the running zoom level; `savedScale` snapshots it at gesture
  // start so the next pinch composes from the current zoom instead
  // of resetting to 1. Same pattern for translate.
  const scale = useSharedValue(1);
  const savedScale = useSharedValue(1);
  const translateX = useSharedValue(0);
  const translateY = useSharedValue(0);
  const savedTranslateX = useSharedValue(0);
  const savedTranslateY = useSharedValue(0);

  const pinch = Gesture.Pinch()
    .onUpdate((e) => {
      scale.value = Math.max(1, Math.min(savedScale.value * e.scale, 6));
    })
    .onEnd(() => {
      savedScale.value = scale.value;
      // Snap back to 1x if the user pinched out below the floor —
      // also re-center so a zoomed-out image doesn't sit off-screen.
      if (scale.value < 1.05) {
        scale.value = withTiming(1);
        translateX.value = withTiming(0);
        translateY.value = withTiming(0);
        savedScale.value = 1;
        savedTranslateX.value = 0;
        savedTranslateY.value = 0;
      }
    });

  const pan = Gesture.Pan()
    .minPointers(1)
    .onUpdate((e) => {
      translateX.value = savedTranslateX.value + e.translationX;
      translateY.value = savedTranslateY.value + e.translationY;
    })
    .onEnd(() => {
      savedTranslateX.value = translateX.value;
      savedTranslateY.value = translateY.value;
    });

  const doubleTap = Gesture.Tap()
    .numberOfTaps(2)
    .onEnd(() => {
      // Double-tap toggles between 1x and 2.5x. Common gesture on
      // mobile photo viewers; cheaper than a slider for quick zoom.
      const target = scale.value > 1.1 ? 1 : 2.5;
      scale.value = withTiming(target);
      savedScale.value = target;
      if (target === 1) {
        translateX.value = withTiming(0);
        translateY.value = withTiming(0);
        savedTranslateX.value = 0;
        savedTranslateY.value = 0;
      }
    });

  const composed = Gesture.Simultaneous(pinch, pan, doubleTap);

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [
      { translateX: translateX.value },
      { translateY: translateY.value },
      { scale: scale.value },
    ],
  }));

  return (
    <Modal visible transparent animationType="fade" onRequestClose={onClose} statusBarTranslucent>
      <Pressable
        style={lightboxStyles.backdrop}
        onPress={onClose}
        accessibilityRole="button"
        accessibilityLabel="Close screenshot"
      >
        <GestureDetector gesture={composed}>
          <Animated.View
            // Stop backdrop-press from firing when the user touches
            // the image — pinch/pan/double-tap should all stay inside
            // the GestureDetector; only the surrounding black space
            // dismisses.
            onStartShouldSetResponder={() => true}
            style={[
              { width, height, justifyContent: "center", alignItems: "center" },
              animatedStyle,
            ]}
          >
            <Image
              source={{ uri, headers }}
              style={{ width, height: height * 0.9 }}
              resizeMode="contain"
            />
          </Animated.View>
        </GestureDetector>
        <Pressable
          onPress={onClose}
          style={lightboxStyles.close}
          accessibilityRole="button"
          accessibilityLabel="Close"
          hitSlop={12}
        >
          <Text style={lightboxStyles.closeText}>✕</Text>
        </Pressable>
      </Pressable>
    </Modal>
  );
}

const lightboxStyles = StyleSheet.create({
  backdrop: {
    flex: 1,
    backgroundColor: "rgba(0, 0, 0, 0.95)",
    justifyContent: "center",
    alignItems: "center",
  },
  close: {
    position: "absolute",
    top: 56,
    right: 20,
    width: 36,
    height: 36,
    borderRadius: 18,
    backgroundColor: "rgba(255, 255, 255, 0.15)",
    justifyContent: "center",
    alignItems: "center",
  },
  closeText: {
    color: "white",
    fontSize: 18,
    lineHeight: 18,
  },
});

function formatT(ms: number): string {
  // `Moment.t` is documented as ms-since-meeting-start; mirror the
  // same `[mm:ss]` pill used by the item rows so users can cross-
  // reference timestamps between the transcript and the timeline.
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60)
    .toString()
    .padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `[${m}:${s}]`;
}

const DOT = 8;
const GUTTER = 18;
const RAIL_WIDTH = 1;
const DOT_TOP_OFFSET = 6;

const styles = StyleSheet.create({
  header: {
    flexDirection: "row",
    alignItems: "center",
    gap: 8,
  },
  headerRule: {
    flex: 1,
    height: 1,
  },
  row: {
    flexDirection: "row",
  },
  gutter: {
    alignItems: "center",
    position: "relative",
  },
  rail: {
    position: "absolute",
    top: 0,
    bottom: 0,
    left: (GUTTER - RAIL_WIDTH) / 2,
    width: RAIL_WIDTH,
  },
  dot: {
    width: DOT,
    height: DOT,
    borderRadius: DOT / 2,
    marginTop: DOT_TOP_OFFSET,
  },
  body: {
    flex: 1,
  },
  bodyHead: {
    flexDirection: "row",
    alignItems: "center",
  },
  empty: {
    alignItems: "center",
    justifyContent: "center",
  },
});
