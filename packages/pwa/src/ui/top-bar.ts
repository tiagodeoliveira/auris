//! Always-visible top status row + meetings browser + settings gear.
//!
//! Renders a single status pill (dot + label, color follows state)
//! instead of two side-by-side WS/BLE labels. The glasses (BLE) state
//! piggybacks on the same pill via a small secondary indicator only
//! when relevant — most users never pair glasses, so the chrome stays
//! out of their way.

import type { Store } from "../store";

export function mountTopBar(parent: HTMLElement, store: Store, onSettings: () => void): void {
  const bar = document.createElement("div");
  bar.className = "top-bar";
  parent.appendChild(bar);

  const status = document.createElement("div");
  status.className = "top-bar-status";
  bar.appendChild(status);

  const dot = document.createElement("span");
  dot.className = "top-bar-dot";
  const label = document.createElement("span");
  label.className = "top-bar-label";

  const bleBadge = document.createElement("span");
  bleBadge.className = "top-bar-ble";
  bleBadge.title = "Glasses connected";
  bleBadge.textContent = "GLASSES";

  status.append(dot, label, bleBadge);

  const meetings = document.createElement("button");
  meetings.className = "top-bar-icon-btn";
  meetings.setAttribute("aria-label", "Browse meetings");
  meetings.title = "Meetings";
  meetings.textContent = "☰";
  meetings.addEventListener("click", () => store.update({ meetingsModalOpen: true }));
  bar.appendChild(meetings);

  const artifacts = document.createElement("button");
  artifacts.className = "top-bar-icon-btn";
  artifacts.setAttribute("aria-label", "Browse artifacts");
  artifacts.title = "Artifacts";
  // 📄 (page-with-curl) reads as "documents" without competing with
  // the meetings hamburger or the gear visually.
  artifacts.textContent = "📄";
  artifacts.addEventListener("click", () => store.update({ artifactsModalOpen: true }));
  bar.appendChild(artifacts);

  const gear = document.createElement("button");
  gear.className = "top-bar-icon-btn";
  gear.setAttribute("aria-label", "Open settings");
  gear.textContent = "⚙";
  gear.addEventListener("click", onSettings);
  bar.appendChild(gear);

  function render() {
    const s = store.get();
    const ws = s.wsStatus;
    let state: "ok" | "pending" | "off" | "error";
    let text: string;
    if (ws === "open") {
      state = "ok";
      text = "Connected";
    } else if (ws === "connecting") {
      state = "pending";
      text = "Connecting…";
    } else if (ws === "reconnecting") {
      state = "pending";
      text = "Reconnecting…";
    } else if (ws === "error") {
      state = "error";
      text = "Connection error";
    } else {
      state = "off";
      text = "Offline";
    }
    dot.dataset.state = state;
    label.textContent = text;
    bleBadge.style.display = s.bleConnected ? "inline" : "none";
  }

  render();
  store.subscribe((s) => `${s.wsStatus}|${s.bleConnected ? 1 : 0}`, render);
}
