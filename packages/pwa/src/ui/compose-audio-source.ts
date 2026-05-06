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
  const wrap = document.createElement("section");
  wrap.className = "compose-audio-source";
  parent.appendChild(wrap);

  const label = document.createElement("label");
  label.className = "compose-audio-source-label";
  label.textContent = "Audio source";
  wrap.appendChild(label);

  const select = document.createElement("select");
  select.className = "compose-audio-source-select";
  label.htmlFor = "compose-audio-source-select";
  select.id = "compose-audio-source-select";
  wrap.appendChild(select);

  select.addEventListener("change", () => {
    const v = select.value;
    store.update({ composeAudioSourceDeviceId: v === "" ? null : v });
  });

  function audioCapableDevices() {
    return store.get().availableDevices.filter((d) => d.capabilities.includes("audio_capture"));
  }

  function render() {
    const s = store.get();
    if (s.meetingState !== "idle") {
      wrap.style.display = "none";
      return;
    }
    const devices = audioCapableDevices();
    wrap.style.display = "flex";
    select.innerHTML = "";

    // Auto-seed: if nothing is picked and there's at least one online
    // audio-capable device, default to the first.
    let pick = s.composeAudioSourceDeviceId;
    if (pick === null) {
      const firstOnline = devices.find((d) => d.online);
      if (firstOnline) {
        pick = firstOnline.id;
        // Defer the store mutation to next microtask so we don't
        // re-enter the subscriber that drove this render.
        queueMicrotask(() => store.update({ composeAudioSourceDeviceId: firstOnline.id }));
      }
    }
    // If the previously-picked device disappeared, clear the selection.
    if (pick !== null && !devices.some((d) => d.id === pick)) {
      pick = null;
      queueMicrotask(() => store.update({ composeAudioSourceDeviceId: null }));
    }

    const noneOpt = document.createElement("option");
    noneOpt.value = "";
    noneOpt.textContent = devices.length === 0 ? "(no devices online)" : "(silent — no source)";
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
