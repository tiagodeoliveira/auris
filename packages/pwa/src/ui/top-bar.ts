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

  // Inline SVGs (rather than unicode glyphs / emoji) so all three
  // icons render at matched stroke weight + size on every browser.
  // Emoji like 📄 / ⚙ render as colored bitmaps on some platforms
  // and as outlined glyphs on others — inconsistent at best, faint
  // at worst against the light pill background.
  const meetings = makeIconBtn(
    "Browse meetings",
    "Meetings",
    `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
       <line x1="5" y1="6" x2="15" y2="6"/>
       <line x1="5" y1="10" x2="15" y2="10"/>
       <line x1="5" y1="14" x2="15" y2="14"/>
     </svg>`,
    () => store.update({ meetingsModalOpen: true }),
  );
  bar.appendChild(meetings);

  const artifacts = makeIconBtn(
    "Browse artifacts",
    "Artifacts",
    `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round">
       <path d="M6 4 H11 L14 7 V16 H6 Z"/>
       <path d="M11 4 V7 H14"/>
       <line x1="8" y1="11" x2="12" y2="11" stroke-width="1.4"/>
       <line x1="8" y1="13.5" x2="12" y2="13.5" stroke-width="1.4"/>
     </svg>`,
    () => store.update({ artifactsModalOpen: true }),
  );
  bar.appendChild(artifacts);

  const gear = makeIconBtn(
    "Open settings",
    "Settings",
    `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round">
       <circle cx="10" cy="10" r="2.5"/>
       <path d="M10 3 L10 5 M10 15 L10 17 M3 10 L5 10 M15 10 L17 10
                M5.05 5.05 L6.46 6.46 M13.54 13.54 L14.95 14.95
                M5.05 14.95 L6.46 13.54 M13.54 6.46 L14.95 5.05" stroke-linecap="round"/>
     </svg>`,
    onSettings,
  );
  bar.appendChild(gear);

  function makeIconBtn(
    ariaLabel: string,
    tooltip: string,
    svgHtml: string,
    onClick: () => void,
  ): HTMLButtonElement {
    const b = document.createElement("button");
    b.className = "top-bar-icon-btn";
    b.setAttribute("aria-label", ariaLabel);
    b.title = tooltip;
    b.innerHTML = svgHtml;
    b.addEventListener("click", onClick);
    return b;
  }

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
