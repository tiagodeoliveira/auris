//! Slim top bar: WS status (dot + label) on the left, single
//! overflow button on the right that opens a dropdown menu listing
//! the four destinations (Meetings, Artifacts, Quick Asks, Settings).
//!
//! The PWA is always hosted inside the Even Hub native app on
//! mobile. The host chrome already renders the app name + back
//! arrow above this view, so the previous brand mark + four inline
//! icon buttons were duplicating navigation surface and felt
//! desktop-toolbar-y on a phone. This shape mirrors Even Hub's
//! own QuickList chrome (slim title row with a single `…` menu
//! affordance).

import type { Store } from "../store";

const SVG_NS = "http://www.w3.org/2000/svg";

export function mountTopBar(parent: HTMLElement, store: Store, onSettings: () => void): void {
  const bar = document.createElement("div");
  bar.className = "top-bar";
  parent.appendChild(bar);

  // === STATUS (left) ===
  const status = document.createElement("div");
  status.className = "top-bar-status";
  bar.appendChild(status);

  const dot = document.createElement("span");
  dot.className = "top-bar-dot";
  const label = document.createElement("span");
  label.className = "top-bar-label";

  // BLE badge sits inline next to the WS label, only visible when
  // the glasses are paired. Most users never pair glasses, so this
  // stays hidden by default — keeping the bar uncluttered.
  const bleBadge = document.createElement("span");
  bleBadge.className = "top-bar-ble";
  bleBadge.title = "Glasses connected";
  bleBadge.textContent = "GLASSES";

  status.append(dot, label, bleBadge);

  // === OVERFLOW BUTTON (right) ===
  const overflow = document.createElement("button");
  overflow.type = "button";
  overflow.className = "top-bar-overflow";
  overflow.setAttribute("aria-label", "More");
  overflow.setAttribute("aria-haspopup", "menu");
  overflow.setAttribute("aria-expanded", "false");
  overflow.title = "More";
  overflow.appendChild(makeOverflowIcon());
  bar.appendChild(overflow);

  // === OVERFLOW MENU (anchored, hidden by default) ===
  // Mounted as a child of the bar — `position: sticky` establishes
  // a containing block for absolute descendants, so the menu's
  // `top: calc(100% + 4px); right: 8px` anchors to the bar's
  // bottom-right corner. The bar's parent (`#app`) has
  // overflow:hidden which would clip a sibling-positioned menu,
  // but the bar itself doesn't clip its own absolutely-positioned
  // children — they paint above the bar's box.
  const menu = document.createElement("div");
  menu.className = "top-bar-menu";
  menu.setAttribute("role", "menu");
  menu.hidden = true;
  bar.appendChild(menu);

  let menuOpen = false;

  function openMenu(): void {
    if (menuOpen) return;
    menuOpen = true;
    menu.hidden = false;
    overflow.setAttribute("aria-expanded", "true");
    // Defer attaching the outside-click handler one tick — the
    // current click is the one that just opened the menu; we
    // don't want it to immediately close again.
    setTimeout(() => document.addEventListener("click", onOutsideClick), 0);
  }
  function closeMenu(): void {
    if (!menuOpen) return;
    menuOpen = false;
    menu.hidden = true;
    overflow.setAttribute("aria-expanded", "false");
    document.removeEventListener("click", onOutsideClick);
  }
  function onOutsideClick(ev: MouseEvent): void {
    if (!(ev.target instanceof Node)) return;
    if (menu.contains(ev.target) || overflow.contains(ev.target)) return;
    closeMenu();
  }

  overflow.addEventListener("click", (ev) => {
    ev.stopPropagation();
    if (menuOpen) closeMenu();
    else openMenu();
  });

  /// Build one menu row. Selecting a row closes the menu first
  /// (so the destination's open transition isn't competing with
  /// our dismiss animation) then fires the row's action.
  function makeRow(text: string, onClick: () => void): HTMLButtonElement {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "top-bar-menu-item";
    row.setAttribute("role", "menuitem");
    row.textContent = text;
    row.addEventListener("click", () => {
      closeMenu();
      onClick();
    });
    return row;
  }

  menu.append(
    makeRow("Meetings", () => store.update({ meetingsModalOpen: true })),
    makeRow("Artifacts", () => store.update({ artifactsModalOpen: true })),
    makeRow("Quick Asks", () => store.update({ quickAsksModalOpen: true })),
    makeRow("Settings", onSettings),
  );

  function render() {
    const s = store.get();
    const ws = s.wsStatus;
    let state: "ok" | "pending" | "off" | "error";
    let text: string;

    // Control WS down dominates everything — without it chat,
    // events, and re-binding all break, so report that first.
    if (ws === "connecting") {
      state = "pending";
      text = "Connecting…";
    } else if (ws === "reconnecting") {
      state = "pending";
      text = "Reconnecting…";
    } else if (ws === "error") {
      state = "error";
      text = "Connection error";
    } else if (ws !== "open") {
      state = "off";
      text = "Offline";
    } else if (s.meetingState === "active") {
      // Control WS up + active meeting → the user cares about
      // recording status, not the WS. The original bug was here:
      // the bar showed "Connected" because the *control* WS was
      // up, while the /audio WS had been silently dead for ~56
      // minutes. The pill must reflect actual frame flow.
      const ac = s.audioCaptureState;
      if (ac.kind === "streaming") {
        state = "ok";
        text = "Recording";
      } else if (ac.kind === "connecting") {
        state = "pending";
        text = "Connecting audio…";
      } else if (ac.kind === "reconnecting") {
        state = "error";
        text = `Audio reconnecting (${ac.attempt})…`;
      } else if (ac.kind === "failed") {
        state = "error";
        text = "Audio disconnected";
      } else {
        // Silent meeting (no source bound) — connection is healthy,
        // just nothing to record.
        state = "ok";
        text = "Connected";
      }
    } else {
      state = "ok";
      text = "Connected";
    }

    dot.dataset.state = state;
    label.textContent = text;
    bleBadge.style.display = s.bleConnected ? "inline" : "none";
  }

  render();
  store.subscribe(
    (s) =>
      `${s.wsStatus}|${s.bleConnected ? 1 : 0}|${s.meetingState}|${s.audioCaptureState.kind}|${
        s.audioCaptureState.kind === "reconnecting" ? s.audioCaptureState.attempt : 0
      }`,
    render,
  );
}

/// Three filled circles — the standard horizontal-ellipsis affordance
/// for an overflow menu. Built via createElementNS so the SVG tree
/// is constructed without an innerHTML write.
function makeOverflowIcon(): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 20 20");
  svg.setAttribute("fill", "currentColor");
  svg.setAttribute("aria-hidden", "true");
  for (const cx of [4.5, 10, 15.5]) {
    const dot = document.createElementNS(SVG_NS, "circle");
    dot.setAttribute("cx", String(cx));
    dot.setAttribute("cy", "10");
    dot.setAttribute("r", "1.7");
    svg.appendChild(dot);
  }
  return svg;
}
