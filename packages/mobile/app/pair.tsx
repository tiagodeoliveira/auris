// Pair-a-new-device sheet. Opened from Settings → Paired Devices →
// "+ Pair new device".
//
// WS-driven (post 0.6.x):
//   1. On mount: clear any stale pairCode in the store + send the
//      `mint_pair_code` intent over the existing WS.
//   2. Server replies with `pair_code_minted` on the same connection;
//      the store's case branch sets `pairCode`. Effect subscribes and
//      auto-copies the unhyphenated form to the clipboard.
//   3. Tick a 1-second countdown driven by `expires_at`.
//   4. Watch `pairedDevicesSeq` — when it increments AND we have a
//      code, the server just told us a device finished redeeming. Fetch
//      the device list, find the entry that wasn't in our baseline, and
//      flip to the success state. Auto-dismiss after a moment.
//   5. Tapping the code copies it again (manual recovery for users
//      who pasted something else in the meantime).

import * as Clipboard from "expo-clipboard";
import Constants from "expo-constants";
import { router } from "expo-router";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Animated, Pressable, ScrollView, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { haptics } from "@/src/lib/haptics";
import { useAppStore } from "@/src/store";
import { Card, IconButton, MonoLabel, Section } from "@/src/ui/components";
import { useTheme } from "@/src/theme/useTheme";
import { PairingApi } from "@/src/wire/pairing-api";

/// Strip the display hyphen so the PWA's clipboard-paste path sees
/// the canonical 8-char form.
function bareCode(code: string): string {
  return code.replace(/-/g, "");
}

function secondsUntil(iso: string, now: number): number {
  const ts = Date.parse(iso);
  if (!Number.isFinite(ts)) return 0;
  return Math.max(0, Math.floor((ts - now) / 1000));
}

function formatCountdown(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export default function PairScreen() {
  const t = useTheme();
  const pairCode = useAppStore((s) => s.pairCode);
  const setPairCode = useAppStore((s) => s.setPairCode);
  const sendIntent = useAppStore((s) => s.send);
  const pairedDevicesSeq = useAppStore((s) => s.pairedDevicesSeq);
  const wsStatus = useAppStore((s) => s.wsStatus);

  const [now, setNow] = useState(() => Date.now());
  const [paired, setPaired] = useState<{ device_label: string } | null>(null);
  const [copyConfirm, setCopyConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const baselineDeviceIds = useRef<Set<string> | null>(null);
  const copyConfirmTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const handledSeqRef = useRef(pairedDevicesSeq);

  const api = useMemo(() => PairingApi.from(serverUrl, () => auth0.getAccessToken()), []);

  // 1. Boot: clear the previous code (so we don't flash a stale one),
  //    capture the current device list as the baseline, and ask the
  //    server for a fresh code. The WS `send()` buffers if the socket
  //    isn't connected yet, so we don't need to await the handshake.
  useEffect(() => {
    if (!api) {
      setError("Server URL not configured. Set EXPO_PUBLIC_SERVER_URL.");
      return;
    }
    let cancelled = false;
    setPairCode(null);
    (async () => {
      try {
        const baseline = await api.listDevices();
        if (cancelled) return;
        baselineDeviceIds.current = new Set(baseline.map((d) => d.device_id));
        // Snapshot the current seq so the success-detection effect
        // ignores any tick that happened *before* we sent the mint.
        handledSeqRef.current = useAppStore.getState().pairedDevicesSeq;
        sendIntent({ type: "mint_pair_code" });
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "Failed to fetch baseline devices.");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // Intentionally one-shot: this is the screen's bootstrap, not a
    // reactive effect. Resending mint on every render would burn codes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 2. Auto-copy whenever a fresh code arrives. The pill confirms
  //    the clipboard write so the user knows the code is already
  //    on the clipboard before reaching for the PWA.
  useEffect(() => {
    if (!pairCode) return;
    void (async () => {
      await Clipboard.setStringAsync(bareCode(pairCode.code));
      showCopyConfirm();
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pairCode]);

  // 3. 1-second clock tick — drives the countdown label.
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  // 4. Success detection via pairedDevicesSeq. Each tick = the server
  //    fired `paired_devices_changed`. Diff the device list to find
  //    the entry that just appeared.
  useEffect(() => {
    if (paired) return;
    if (!api || !pairCode) return;
    if (pairedDevicesSeq === handledSeqRef.current) return;
    handledSeqRef.current = pairedDevicesSeq;
    void (async () => {
      const devices = await api.listDevices().catch(() => null);
      if (!devices) return;
      const baseline = baselineDeviceIds.current;
      if (!baseline) return;
      const fresh = devices.find((d) => !baseline.has(d.device_id));
      if (fresh) {
        haptics.success();
        setPaired({ device_label: fresh.device_label });
        setTimeout(() => router.back(), 1500);
      }
    })();
  }, [api, pairCode, paired, pairedDevicesSeq]);

  const showCopyConfirm = useCallback(() => {
    setCopyConfirm(true);
    if (copyConfirmTimer.current) clearTimeout(copyConfirmTimer.current);
    copyConfirmTimer.current = setTimeout(() => setCopyConfirm(false), 2000);
  }, []);

  const handleCopy = useCallback(async () => {
    if (!pairCode) return;
    await Clipboard.setStringAsync(bareCode(pairCode.code));
    haptics.select();
    showCopyConfirm();
  }, [pairCode, showCopyConfirm]);

  const secsLeft = pairCode ? secondsUntil(pairCode.expires_at, now) : 0;
  const expired = pairCode !== null && secsLeft === 0 && !paired;

  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: t.color.bg.canvas }}>
      <ScrollView
        contentContainerStyle={{
          padding: t.spacing.lg,
          paddingTop: t.spacing.xxl,
        }}
        showsVerticalScrollIndicator={false}
      >
        <View style={{ alignItems: "center", marginBottom: t.spacing.xl }}>
          <Text
            style={{
              fontFamily: t.font.display,
              fontSize: 28,
              letterSpacing: 2,
              color: t.color.text.primary,
            }}
          >
            pair glasses
          </Text>
          <MonoLabel tone="secondary" style={{ marginTop: t.spacing.xs }}>
            {Constants.expoConfig?.name ?? "Auris"} · {Constants.expoConfig?.version ?? ""}
          </MonoLabel>
        </View>

        <Card style={{ marginBottom: t.spacing.lg, alignItems: "center" }}>
          <Section title="Your pair code">
            {error ? (
              <Text style={{ ...t.type.body, color: t.color.danger.base }}>{error}</Text>
            ) : paired ? (
              <View style={{ alignItems: "center", gap: t.spacing.sm }}>
                <Text
                  style={{
                    fontFamily: t.font.display,
                    fontSize: 32,
                    color: t.color.brand.coral,
                  }}
                >
                  ✓ paired
                </Text>
                <Text style={{ ...t.type.body, color: t.color.text.primary }}>
                  {paired.device_label}
                </Text>
              </View>
            ) : pairCode ? (
              <View style={{ alignItems: "center", gap: t.spacing.md }}>
                <Pressable onPress={handleCopy} hitSlop={8}>
                  <Text
                    selectable
                    style={{
                      fontFamily: t.font.mono,
                      fontSize: 40,
                      letterSpacing: 4,
                      color: expired ? t.color.text.muted : t.color.text.primary,
                      textDecorationLine: expired ? "line-through" : "none",
                    }}
                  >
                    {pairCode.code}
                  </Text>
                </Pressable>
                <MonoLabel tone={expired ? "muted" : "secondary"}>
                  {expired ? "expired" : `expires in ${formatCountdown(secsLeft)}`}
                </MonoLabel>
                <View
                  style={{
                    flexDirection: "row",
                    gap: t.spacing.md,
                    marginTop: t.spacing.sm,
                  }}
                >
                  <IconButton
                    glyph="⎘"
                    label="COPY"
                    tone="action"
                    filled
                    onPress={handleCopy}
                    disabled={expired}
                  />
                </View>
                {copyConfirm && <CopyPill key={now}>copied to clipboard</CopyPill>}
              </View>
            ) : (
              <View style={{ alignItems: "center" }}>
                <Text style={{ ...t.type.body, color: t.color.text.secondary }}>
                  {wsStatus === "open" ? "generating…" : "waiting for connection…"}
                </Text>
              </View>
            )}
          </Section>
        </Card>

        <Card style={{ marginBottom: t.spacing.lg }}>
          <Section title="How to pair">
            <Text
              style={{
                ...t.type.body,
                color: t.color.text.primary,
                lineHeight: 22,
              }}
            >
              1. Open Auris on your glasses.{"\n"}
              2. Tap "Paste from phone" — the code is already copied.{"\n"}
              3. Or type it manually if you prefer.
            </Text>
            <Text
              style={{
                ...t.type.bodySmall,
                color: t.color.text.secondary,
                marginTop: t.spacing.sm,
              }}
            >
              Codes are single-use and expire after 5 minutes.
            </Text>
          </Section>
        </Card>

        <View style={{ alignItems: "center", marginTop: t.spacing.md }}>
          <IconButton glyph="✕" label="CANCEL" tone="neutral" onPress={() => router.back()} />
        </View>
      </ScrollView>
    </SafeAreaView>
  );
}

/// Tiny fade-in pill confirming the clipboard write.
function CopyPill({ children }: { children: React.ReactNode }) {
  const t = useTheme();
  const opacity = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    Animated.timing(opacity, {
      toValue: 1,
      duration: 150,
      useNativeDriver: true,
    }).start();
  }, [opacity]);
  return (
    <Animated.View
      style={{
        opacity,
        backgroundColor: t.color.brand.coralDim,
        paddingHorizontal: t.spacing.md,
        paddingVertical: t.spacing.xs,
        borderRadius: t.radius.pill,
      }}
    >
      <Text
        style={{
          ...t.type.labelMono,
          color: t.color.brand.coral,
          textTransform: "uppercase",
          letterSpacing: 2,
        }}
      >
        {children}
      </Text>
    </Animated.View>
  );
}
