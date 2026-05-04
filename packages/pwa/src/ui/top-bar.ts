//! Always-visible top status row + settings gear.
//! See `docs/specs/pwa-ux-redesign.md` §3.1, §4.3.

import type { Store } from "../store";

export function mountTopBar(parent: HTMLElement, store: Store, onSettings: () => void): void {
  const bar = document.createElement("div");
  bar.className = "top-bar";
  parent.appendChild(bar);

  const status = document.createElement("div");
  status.className = "top-bar-status";
  bar.appendChild(status);

  const wsDot = document.createElement("span");
  wsDot.className = "top-bar-dot";
  const wsLabel = document.createElement("span");
  wsLabel.className = "label-mono";
  wsLabel.textContent = "WS";

  const bleDot = document.createElement("span");
  bleDot.className = "top-bar-dot";
  const bleLabel = document.createElement("span");
  bleLabel.className = "label-mono";
  bleLabel.textContent = "BLE";

  status.append(wsDot, wsLabel, bleDot, bleLabel);

  const gear = document.createElement("button");
  gear.className = "top-bar-gear";
  gear.setAttribute("aria-label", "Open settings");
  gear.innerHTML = "⚙";
  gear.addEventListener("click", onSettings);
  bar.appendChild(gear);

  function render() {
    const s = store.get();
    wsDot.dataset.state =
      s.wsStatus === "open"
        ? "ok"
        : s.wsStatus === "connecting" || s.wsStatus === "reconnecting"
          ? "pending"
          : "off";
    bleDot.dataset.state = s.bleConnected ? "ok" : "off";
  }

  render();
  store.subscribe((s) => s.wsStatus, render);
  store.subscribe((s) => s.bleConnected, render);
}
