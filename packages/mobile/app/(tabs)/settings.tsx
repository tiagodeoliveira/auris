// Settings — the "listening room" surface for account, appearance,
// audio defaults, and the about line. The header is a brand-forward
// lockup (live AurisMark + Bebas Neue wordmark) so opening the tab
// reads as "you are inside auris" before the rows do any work.
//
// The Appearance picker is fully wired: the segmented control reads
// from `themeOverride` on the store and writes through the
// `setThemeOverride` setter (which persists to AsyncStorage and
// triggers a tree-wide re-render via `useTheme()`).

import Constants from "expo-constants";
import * as Updates from "expo-updates";
import { router, type Href } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { Alert, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import * as auth0 from "@/src/auth/auth0";
import { serverUrl } from "@/src/config";
import { haptics } from "@/src/lib/haptics";
import { AurisMark } from "@/src/ui/AurisMark";
import { Card, IconButton, MonoLabel, Section } from "@/src/ui/components";
import { useAppStore, type ThemeOverride } from "@/src/store";
import { useTheme } from "@/src/theme/useTheme";
import { PairingApi, type PairedDevice } from "@/src/wire/pairing-api";

/// Map an Auth0 `sub` to a human-readable identity provider label.
/// `sub` is documented as `<connection>|<user-id>`; the connection
/// prefix names the social/database provider. Unknown providers
/// fall back to a capitalized version of the prefix so we don't
/// claim more knowledge than we have.
function providerLabel(sub: string): string {
  const prefix = sub.split("|", 1)[0] ?? sub;
  const map: Record<string, string> = {
    auth0: "Username/password",
    "google-oauth2": "Google",
    apple: "Apple",
    github: "GitHub",
    facebook: "Facebook",
    linkedin: "LinkedIn",
    windowslive: "Microsoft",
    twitter: "Twitter/X",
    email: "Email link",
    sms: "SMS",
  };
  return map[prefix] ?? prefix.charAt(0).toUpperCase() + prefix.slice(1);
}

/// Return the user-id portion of an Auth0 `sub` (the part after `|`).
function userIdTail(sub: string): string {
  const idx = sub.indexOf("|");
  return idx === -1 ? sub : sub.slice(idx + 1);
}

const APP_VERSION = `v${Constants.expoConfig?.version ?? "?"}`;

/// Short identifier for the currently-loaded JS bundle:
///   - `embedded`    — running the bundle that shipped inside the .ipa
///                     (the EAS Update CDN hasn't been hit yet, OR the
///                     CDN had no newer compatible bundle to deliver)
///   - first 8 chars of `Updates.updateId` once an OTA has been applied,
///     matching the prefix the EAS dashboard uses in update group IDs
///     (e.g., 9fb86dad)
///
/// Useful for confirming "did my OTA actually land?" without having to
/// crack open the IPA. Falls back to "embedded" in dev / Expo Go where
/// `Updates.updateId` is null.
function bundleLabel(): string {
  const id = Updates.updateId;
  if (!id) return "embedded";
  return id.replace(/-/g, "").slice(0, 8);
}

/// Relative-time label for "last seen N ago". Cheap heuristic — we
/// re-render on focus, not on a clock tick, so granularity beyond
/// minutes isn't useful here.
function relativeAgo(iso: string): string {
  const ms = Date.now() - Date.parse(iso);
  if (!Number.isFinite(ms) || ms < 0) return "just now";
  const sec = Math.floor(ms / 1000);
  if (sec < 60) return "just now";
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  return `${day}d ago`;
}

export default function SettingsScreen() {
  const t = useTheme();
  const identity = useAppStore((s) => s.identity);
  const signOut = useAppStore((s) => s.signOut);
  const themeMode = useAppStore((s) => s.themeOverride);
  const setThemeOverride = useAppStore((s) => s.setThemeOverride);

  // Paired devices. Loaded on mount and on every `paired_devices_changed`
  // event the server fans out (redeem or revoke from any surface).
  // Errors are swallowed — the Account row is the canonical "is the
  // server reachable" signal, and a transient blip shouldn't visually
  // scream.
  const [devices, setDevices] = useState<PairedDevice[] | null>(null);
  const pairedDevicesSeq = useAppStore((s) => s.pairedDevicesSeq);
  const reloadDevices = useCallback(async () => {
    if (!identity) {
      setDevices(null);
      return;
    }
    const api = PairingApi.from(serverUrl, () => auth0.getAccessToken());
    if (!api) return;
    try {
      setDevices(await api.listDevices());
    } catch {
      // Keep stale list; user-visible errors land via the pair sheet.
    }
  }, [identity]);
  useEffect(() => {
    void reloadDevices();
  }, [reloadDevices, pairedDevicesSeq]);

  const handleUnpair = useCallback(
    (device: PairedDevice) => {
      Alert.alert(
        "Unpair this device?",
        `${device.device_label} will need to pair again before it can sync with Auris.`,
        [
          { text: "Cancel", style: "cancel" },
          {
            text: "Unpair",
            style: "destructive",
            onPress: async () => {
              const api = PairingApi.from(serverUrl, () => auth0.getAccessToken());
              if (!api) return;
              try {
                await api.revoke(device.device_id);
                haptics.warning();
                await reloadDevices();
              } catch (e) {
                Alert.alert("Couldn't unpair", e instanceof Error ? e.message : "Network error.");
              }
            },
          },
        ],
      );
    },
    [reloadDevices],
  );

  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: t.color.bg.canvas }}>
      <ScrollView
        contentContainerStyle={{
          paddingHorizontal: t.spacing.lg,
          paddingBottom: t.spacing.xxxl,
        }}
        showsVerticalScrollIndicator={false}
      >
        {/* ─── Brand lockup ─────────────────────────────────────── */}
        <View
          style={{
            alignItems: "center",
            paddingTop: t.spacing.xxl,
            paddingBottom: t.spacing.xl,
          }}
        >
          <AurisMark size={48} variant="coral" />
          <Text
            style={{
              fontFamily: t.font.display,
              fontSize: 40,
              letterSpacing: 3,
              color: t.color.text.primary,
              marginTop: t.spacing.md,
            }}
          >
            auris
          </Text>
          <MonoLabel tone="secondary" style={{ marginTop: t.spacing.xs }}>
            {`meeting companion · ${APP_VERSION}`}
          </MonoLabel>
          <MonoLabel tone="muted" style={{ marginTop: 2 }}>
            {`bundle · ${bundleLabel()}`}
          </MonoLabel>
        </View>

        {/* ─── Account ──────────────────────────────────────────── */}
        <Card style={{ marginBottom: t.spacing.lg }}>
          <Section title="Account">
            {identity ? (
              <View style={{ gap: t.spacing.xs }}>
                <Text
                  style={{
                    ...t.type.subtitle,
                    color: t.color.text.primary,
                  }}
                >
                  {identity.name ?? identity.email ?? "Signed in"}
                </Text>
                {identity.email && identity.email !== identity.name && (
                  <Text
                    style={{
                      ...t.type.mono,
                      color: t.color.text.secondary,
                    }}
                  >
                    {identity.email}
                  </Text>
                )}
                {/* Sign-in method + user ID. Auth0's `sub` claim is
                    shaped `<provider>|<user-id>` (e.g.,
                    "google-oauth2|123…"); split to surface each piece
                    so users can recognize which identity they're
                    signed in with and copy the user ID for support. */}
                <MonoLabel tone="muted" style={{ marginTop: t.spacing.xs }}>
                  {`SIGN-IN · ${providerLabel(identity.sub)}`}
                </MonoLabel>
                <Text
                  selectable
                  style={{
                    ...t.type.mono,
                    color: t.color.text.secondary,
                  }}
                  numberOfLines={1}
                  ellipsizeMode="middle"
                >
                  {userIdTail(identity.sub)}
                </Text>
                <View style={{ marginTop: t.spacing.md, alignSelf: "flex-start" }}>
                  <IconButton
                    glyph="⤴"
                    label="SIGN OUT"
                    tone="danger"
                    onPress={() => void signOut()}
                    style={{
                      borderWidth: 1,
                      borderColor: t.color.danger.base,
                      paddingHorizontal: t.spacing.md,
                      paddingVertical: t.spacing.sm,
                    }}
                  />
                </View>
              </View>
            ) : (
              <Text
                style={{
                  ...t.type.bodySmall,
                  color: t.color.text.secondary,
                }}
              >
                — not signed in
              </Text>
            )}
          </Section>
        </Card>

        {/* ─── Paired devices ───────────────────────────────────── */}
        {identity && (
          <Card style={{ marginBottom: t.spacing.lg }}>
            <Section
              title="Paired devices"
              subtitle="glasses + future hardware that talks to auris"
            >
              {devices && devices.length > 0 ? (
                <View style={{ gap: t.spacing.sm }}>
                  {devices.map((d) => (
                    <View
                      key={d.device_id}
                      style={{
                        flexDirection: "row",
                        alignItems: "center",
                        justifyContent: "space-between",
                        paddingVertical: t.spacing.xs,
                      }}
                    >
                      <View style={{ flex: 1, paddingRight: t.spacing.sm }}>
                        <Text
                          style={{
                            ...t.type.body,
                            color: t.color.text.primary,
                          }}
                        >
                          {d.device_label}
                        </Text>
                        <MonoLabel tone="muted" style={{ marginTop: 2 }}>
                          {`ACTIVE · ${relativeAgo(d.last_seen_at)}`}
                        </MonoLabel>
                      </View>
                      <IconButton
                        glyph="⊘"
                        label="UNPAIR"
                        tone="danger"
                        onPress={() => handleUnpair(d)}
                      />
                    </View>
                  ))}
                </View>
              ) : (
                <Text
                  style={{
                    ...t.type.bodySmall,
                    color: t.color.text.secondary,
                  }}
                >
                  — no glasses paired yet
                </Text>
              )}
              <View style={{ marginTop: t.spacing.md, alignSelf: "flex-start" }}>
                <IconButton
                  glyph="＋"
                  label="PAIR NEW DEVICE"
                  tone="action"
                  filled
                  onPress={() => {
                    haptics.select();
                    router.push("/pair" as Href);
                  }}
                />
              </View>
            </Section>
          </Card>
        )}

        {/* ─── Quick asks ───────────────────────────────────────── */}
        {identity && (
          <Card style={{ marginBottom: t.spacing.lg }}>
            <Section
              title="Quick asks"
              subtitle="saved prompts you can fire into chat or pick from the glasses"
            >
              <View style={{ alignSelf: "flex-start" }}>
                <IconButton
                  glyph="✎"
                  label="MANAGE QUICK ASKS"
                  tone="action"
                  filled
                  onPress={() => {
                    haptics.select();
                    router.push("/quick-asks" as Href);
                  }}
                />
              </View>
            </Section>
          </Card>
        )}

        {/* ─── Appearance ───────────────────────────────────────── */}
        <Card style={{ marginBottom: t.spacing.lg }}>
          <Section title="Appearance" subtitle="dark mode follows your device by default">
            <View
              style={[
                styles.segment,
                {
                  backgroundColor: t.color.bg.tint,
                  borderRadius: t.radius.pill,
                  padding: 4,
                },
              ]}
            >
              {(["system", "light", "dark"] as const satisfies readonly ThemeOverride[]).map(
                (mode) => {
                  const active = mode === themeMode;
                  return (
                    <Pressable
                      key={mode}
                      onPress={() => {
                        if (mode === themeMode) return;
                        haptics.select();
                        setThemeOverride(mode);
                      }}
                      style={({ pressed }) => [
                        styles.segmentItem,
                        {
                          borderRadius: t.radius.pill,
                          paddingVertical: t.spacing.sm,
                        },
                        active && { backgroundColor: t.color.brand.coral },
                        pressed && !active && { opacity: 0.6 },
                      ]}
                    >
                      <Text
                        style={{
                          ...t.type.labelMono,
                          textTransform: "uppercase",
                          letterSpacing: 2,
                          color: active ? t.color.text.onCoral : t.color.text.secondary,
                        }}
                      >
                        {mode}
                      </Text>
                    </Pressable>
                  );
                },
              )}
            </View>
          </Section>
        </Card>

        {/* ─── About ────────────────────────────────────────────── */}
        <Card style={{ marginBottom: t.spacing.lg }}>
          <Section title="About">
            <Text
              style={{
                ...t.type.body,
                color: t.color.text.primary,
                lineHeight: 22,
              }}
            >
              auris listens to your meetings — on mac, on the web, and now on your phone.
            </Text>
            <Text
              style={{
                ...t.type.bodySmall,
                color: t.color.text.secondary,
                marginTop: t.spacing.sm,
              }}
            >
              crafted by tiago oliveira.
            </Text>
            <Text
              style={{
                ...t.type.mono,
                color: t.color.text.muted,
                marginTop: t.spacing.md,
              }}
            >
              github.com/tiagodeoliveira/auris
            </Text>
          </Section>
        </Card>
      </ScrollView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  segment: {
    flexDirection: "row",
    alignItems: "center",
  },
  segmentItem: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
  },
});
