//! Idle-state Start Meeting button. Lives in its own component so the
//! mount order can place it below the metadata (kv-editor) strip while
//! keeping kv-editor visible in both idle and active states.

import type { Store } from "../store";
import type { CtaActions } from "./cta-region";
import type { AppState } from "../types";

export function mountComposeStart(parent: HTMLElement, store: Store, actions: CtaActions): void {
  const wrap = document.createElement("section");
  wrap.className = "compose-start-wrap";
  parent.appendChild(wrap);

  const btn = document.createElement("button");
  btn.className = "btn-primary compose-start";
  btn.textContent = "Start Meeting";
  btn.addEventListener("click", () => {
    const s = store.get();
    if (startBlockedReason(s) !== null) return; // belt-and-suspenders
    const desc = s.composeDescription.trim();
    actions.startMeeting(desc, s.composeAudioSourceDeviceId);
  });
  wrap.appendChild(btn);

  const reason = document.createElement("div");
  reason.className = "compose-start-reason";
  wrap.appendChild(reason);

  function syncDisabledState() {
    const s = store.get();
    const blocked = startBlockedReason(s);
    btn.disabled = blocked !== null;
    if (blocked) {
      reason.textContent = blocked;
      reason.style.display = "block";
    } else {
      reason.textContent = "";
      reason.style.display = "none";
    }
  }

  function syncVisibility() {
    wrap.style.display = store.get().meetingState === "idle" ? "flex" : "none";
  }
  syncVisibility();
  syncDisabledState();
  store.subscribe((s) => s.meetingState, syncVisibility);
  // Re-evaluate disabled state on any input that affects it: WS status,
  // online audio-capable devices, picked source, and the extracting flag.
  store.subscribe(
    (s) =>
      `${s.wsStatus}|${s.extractingMetadata}|${s.composeAudioSourceDeviceId ?? ""}|` +
      s.availableDevices
        .filter((d) => d.capabilities.includes("audio_capture"))
        .map((d) => `${d.id}:${d.online ? "1" : "0"}`)
        .join(","),
    syncDisabledState,
  );
}

/// Returns a short user-facing reason the Start button should be
/// disabled, or `null` when start is allowed. The order matters —
/// the message reflects the most upstream blocker first (no
/// connection > no source > extracting in flight).
function startBlockedReason(s: AppState): string | null {
  if (s.wsStatus !== "open") return "Not connected to the server";
  const audioDevices = s.availableDevices.filter((d) => d.capabilities.includes("audio_capture"));
  const hasOnline = audioDevices.some((d) => d.online);
  if (!hasOnline) return "Open the Mac app to provide audio";
  if (s.composeAudioSourceDeviceId === null) return "Pick an audio source above";
  if (s.extractingMetadata) return "Finishing tag extraction…";
  return null;
}
