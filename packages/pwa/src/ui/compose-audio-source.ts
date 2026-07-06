//! Audio-source picker for the idle compose flow.
//!
//! Lists registered devices that can capture audio (`audio_capture`
//! capability) and lets the user choose which one will feed the meeting
//! they're about to start. The choice is held in
//! `composeAudioSourceDeviceId` and consumed by the Start button.
//!
//! Auto-seeding: when devices arrive (or change) and no device is
//! currently picked, we pre-select the first online audio-capable
//! device so single-source setups don't require any clicks.

import type { Store } from "../store";

export function mountComposeAudioSource(parent: HTMLElement, store: Store): void {
  // No outer wrapper — the calling compose-card owns the chrome
  // (title + underline + padding). We just emit the select +
  // empty-state hint into the slot we were given.
  const wrap = parent;

  // The previous version emitted an in-content `<label>` reading
  // "Audio source". That's now redundant with the compose-card
  // title above us, so the inline label is dropped.

  const select = document.createElement("select");
  select.className = "compose-audio-source-select";
  select.id = "compose-audio-source-select";
  wrap.appendChild(select);

  // Sibling empty-state node used in place of the select when no
  // online audio-capable device is registered. Plain language tells
  // the user what to do; no orange focus ring or interactive shape.
  const emptyHint = document.createElement("div");
  emptyHint.className = "compose-audio-source-empty";
  wrap.appendChild(emptyHint);

  select.addEventListener("change", () => {
    const v = select.value;
    store.update({ composeAudioSourceDeviceId: v === "" ? null : v });
  });

  function audioCapableDevices() {
    return store.get().availableDevices.filter((d) => d.capabilities.includes("audio_capture"));
  }

  function render() {
    const s = store.get();
    // The parent card hides itself outside idle; we don't need to
    // touch our own visibility. Bail early so we don't spend work
    // populating a select nobody can see.
    if (s.meetingState !== "idle") {
      return;
    }
    const devices = audioCapableDevices();
    const hasOnline = devices.some((d) => d.online);

    // No online audio-capable device: show the empty-state hint and
    // hide the dropdown. Saves the user from staring at a select
    // whose only option is "(no devices online)".
    if (!hasOnline) {
      select.style.display = "none";
      emptyHint.style.display = "flex";
      emptyHint.innerHTML = "";
      const icon = document.createElement("span");
      icon.className = "compose-audio-source-empty-icon";
      icon.textContent = "MIC";
      const text = document.createElement("span");
      // Per cross-surface spec Rule 4: browser-mic-as-source isn't
      // implemented yet, so be explicit that a Mac is the only path.
      text.textContent =
        devices.length === 0
          ? "── Mac required to capture audio; the web app doesn't ship a microphone source yet."
          : "── all audio-capable devices are offline; reopen the Mac app to reconnect.";
      emptyHint.append(icon, text);
      // Clear any stale pick — the device went away.
      if (s.composeAudioSourceDeviceId !== null) {
        queueMicrotask(() => store.update({ composeAudioSourceDeviceId: null }));
      }
      return;
    }

    select.style.display = "";
    emptyHint.style.display = "none";
    select.innerHTML = "";

    // Auto-seed: if nothing is picked and there's at least one online
    // audio-capable device, default to the first.
    let pick = s.composeAudioSourceDeviceId;
    if (pick === null) {
      const firstOnline = devices.find((d) => d.online)!;
      pick = firstOnline.id;
      // Defer the store mutation to next microtask so we don't
      // re-enter the subscriber that drove this render.
      queueMicrotask(() => store.update({ composeAudioSourceDeviceId: firstOnline.id }));
    }
    // If the previously-picked device disappeared, clear the selection.
    if (pick !== null && !devices.some((d) => d.id === pick)) {
      pick = null;
      queueMicrotask(() => store.update({ composeAudioSourceDeviceId: null }));
    }

    const noneOpt = document.createElement("option");
    noneOpt.value = "";
    noneOpt.textContent = "(silent — no source)";
    select.appendChild(noneOpt);

    for (const d of devices) {
      const opt = document.createElement("option");
      opt.value = d.id;
      const offlineSuffix = d.online ? "" : " (offline)";
      opt.textContent = `${d.hostname}${offlineSuffix}`;
      opt.disabled = !d.online;
      select.appendChild(opt);
    }

    select.value = pick ?? "";
  }

  render();
  store.subscribe(
    (s) =>
      `${s.meetingState}|${s.composeAudioSourceDeviceId ?? ""}|` +
      s.availableDevices
        .map((d) => `${d.id}:${d.online ? "1" : "0"}:${d.capabilities.join(",")}`)
        .join("/"),
    render,
  );
}
