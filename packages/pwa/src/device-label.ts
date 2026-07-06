//! Human-readable device label derived from the glasses' own serial.
//!
//! The EvenHub SDK exposes the connected device's serial + model via
//! `bridge.getDeviceInfo()`. We use that to label this client as
//! `"<serial> (G2)"` everywhere it shows up — the audio-source picker
//! (via `register_device.hostname`) and the paired-devices list (via
//! the `/pair/redeem` `device_label`). Replaces the static
//! "Browser (Glasses)" / "G2 glasses" strings so a user with multiple
//! pairs can tell them apart.
//!
//! Returns `null` when no serial is available (prototype mode in a
//! plain browser tab, or glasses not yet connected). Callers choose
//! their own fallback: the audio source falls back to
//! "Browser (Glasses)"; the pair flow omits the field so the server
//! keeps its "G2 glasses" default.

/// Minimal structural view of the EvenHub bridge's device-info call.
/// `getDeviceInfo` is optional so a bare KV bridge (auth.ts) is
/// assignable without widening its type; at runtime the real bridge
/// provides it.
export interface DeviceInfoBridge {
  getDeviceInfo?: () => Promise<{ sn?: string | null; model?: string | null } | null>;
}

/// Resolve `"<serial> (MODEL)"` from the connected glasses, or `null`
/// when there's no serial to use. Never throws — `getDeviceInfo` can
/// reject in prototype mode, which we treat as "no device."
export async function resolveDeviceLabel(bridge: DeviceInfoBridge): Promise<string | null> {
  try {
    const info = await bridge.getDeviceInfo?.();
    const sn = info?.sn?.trim();
    if (sn) {
      // Model defaults to g2 — the only audio-capable glasses today —
      // when the SDK omits it. Uppercased for display: "(G2)".
      const model = (info?.model ?? "g2").toString().trim().toUpperCase();
      return model ? `${sn} (${model})` : sn;
    }
  } catch {
    // No bridge device-info (prototype/dev) — fall through to null.
  }
  return null;
}
