//! Settings modal — brand lockup + Account / About cards.
//! Replaces the v1 single-pane modal (which also showed the WS URL
//! + connection state, both now living in the top-bar status pill).
//!
//! No theme picker: the PWA always renders light to match the
//! Even Hub host chrome. Tracking the OS dark-mode preference
//! created a jarring mismatch (light Even Hub chrome over a dark
//! PWA body) so the dark-mode CSS rules and the theme picker were
//! removed together.

import type { Store } from "../store";
import type { AuthBundle } from "../auth";
import { makeCloseButton } from "./modal-chrome";
import { saveSetting } from "../storage";
import { resolveDeviceLabel } from "../device-label";
import type { ModeOption } from "../contract";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
  /// Optional — the real EvenHub bridge exposes it; the narrow KV
  /// bridge used in tests doesn't. Drives the About card's device row.
  getDeviceInfo?: () => Promise<{ sn?: string | null; model?: string | null } | null>;
}

/// Modes hidden from the glasses cycle regardless of user setting —
/// must match the hardcoded exclusion in `glassesModeIds` in
/// `input/gesture-router.ts` so the "Glasses display" toggles only
/// expose what the cycle actually shows.
const GLASSES_CYCLE_HIDDEN: ReadonlySet<string> = new Set(["chat", "assist"]);

export function mountSettingsModal(
  parent: HTMLElement,
  store: Store,
  bridge: BridgeLike,
  _onSave: () => void,
  auth: AuthBundle,
): void {
  const overlay = document.createElement("div");
  overlay.className = "settings-overlay";
  parent.appendChild(overlay);

  const modal = document.createElement("div");
  modal.className = "settings-modal settings-modal-v2";
  overlay.appendChild(modal);

  // Click on the backdrop closes — matches the meetings/artifacts
  // modal pattern. Clicks inside the modal don't bubble past
  // .settings-modal because we stop here.
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) store.update({ settingsModalOpen: false });
  });

  // === CLOSE (top-right) ===
  // Floats over the brand lockup at the modal's top-right corner.
  // Same ✕ button the other three modals use; positioned absolutely
  // via the `.settings-modal-close` rule so it doesn't push the
  // brand lockup off-center.
  const closeBtn = makeCloseButton(() => store.update({ settingsModalOpen: false }));
  closeBtn.classList.add("settings-modal-close");
  modal.appendChild(closeBtn);

  // === BRAND LOCKUP ===
  // Centered mark + wordmark + tagline. The mark is the same ear-arc
  // SVG used in the top bar; here it's larger (52px) so the brand
  // reads as the focal element when the modal opens.
  const lockup = document.createElement("div");
  lockup.className = "settings-brand-lockup";
  const markWrap = document.createElement("div");
  markWrap.className = "settings-brand-lockup-mark";
  markWrap.appendChild(makeBrandMark());
  const word = document.createElement("div");
  word.className = "settings-brand-lockup-word";
  word.textContent = "auris";
  const tag = document.createElement("div");
  tag.className = "settings-brand-lockup-tag";
  // VITE_APP_VERSION is injected by vite.config.ts (defined via
  // `define`, read from package.json at build time). The cast +
  // fallback keeps tsc happy without a vite-env.d.ts and degrades
  // gracefully if the define ever drops out.
  const appVersion = (import.meta.env.VITE_APP_VERSION as string | undefined) ?? "?";
  tag.textContent = `MEETING COMPANION · V${appVersion.toUpperCase()}`;
  lockup.append(markWrap, word, tag);
  modal.appendChild(lockup);

  // === ACCOUNT CARD ===
  const accountCard = document.createElement("section");
  accountCard.className = "settings-card";
  const accountLabel = document.createElement("div");
  accountLabel.className = "settings-card-label";
  accountLabel.textContent = "ACCOUNT";
  const accountEmail = document.createElement("div");
  accountEmail.className = "settings-account-email";
  // Auth0 `sub` is shaped `<provider>|<user-id>`. Surface both pieces
  // on their own rows so users can recognize which identity they're
  // signed in with at a glance (and confirm the user ID for support
  // requests).
  const accountProvider = document.createElement("div");
  accountProvider.className = "settings-account-meta";
  const accountUserId = document.createElement("div");
  accountUserId.className = "settings-account-meta settings-account-userid";
  const signOutBtn = document.createElement("button");
  signOutBtn.type = "button";
  signOutBtn.className = "btn-ghost-destructive";
  signOutBtn.textContent = "↗ SIGN OUT";
  signOutBtn.addEventListener("click", () => {
    // Clear local tokens then refresh — main.ts re-runs from scratch,
    // sees no paired session, and mounts the pair screen. Matches the
    // way Auth0's hosted logout used to reload the bundle on return.
    void auth.logout().then(() => window.location.reload());
  });
  accountCard.append(accountLabel, accountEmail, accountProvider, accountUserId, signOutBtn);
  modal.appendChild(accountCard);

  // === GLASSES DISPLAY CARD ===
  // Per-mode opt-outs for the glasses double-tap cycle. Disabling a
  // mode removes it from the cycle so the glasses never render it —
  // useful for sluggish text views (transcript especially) when the
  // user only cares about a subset of surfaces. PWA-local: persisted
  // to bridge KV (localStorage on the simulator) under `mc.glassesModes`.
  const glassesCard = document.createElement("section");
  glassesCard.className = "settings-card";
  const glassesLabel = document.createElement("div");
  glassesLabel.className = "settings-card-label";
  glassesLabel.textContent = "GLASSES DISPLAY";
  const glassesHint = document.createElement("p");
  glassesHint.className = "settings-card-body";
  glassesHint.textContent =
    "Pick what double-tap cycles through on the glasses. Hiding heavy views (like transcript) speeds up the glasses display.";
  const glassesToggles = document.createElement("div");
  glassesToggles.className = "settings-glasses-toggles";
  glassesCard.append(glassesLabel, glassesHint, glassesToggles);
  modal.appendChild(glassesCard);

  // The toggle list is server-driven (rendered from `availableModes`)
  // so a future new mode appears here without UI changes. Rendered
  // imperatively on every store change because `availableModes`
  // arrives async after WS handshake.
  function renderGlassesToggles() {
    const s = store.get();
    const cycleModes: ModeOption[] = s.availableModes.filter(
      (m) => !GLASSES_CYCLE_HIDDEN.has(m.id),
    );
    glassesToggles.replaceChildren();
    if (cycleModes.length === 0) {
      const empty = document.createElement("div");
      empty.className = "settings-card-body settings-muted";
      empty.textContent = "(connect to a meeting once to populate this list)";
      glassesToggles.appendChild(empty);
      return;
    }
    // Count of "would-be-enabled" toggles, used to lock the last
    // remaining one so the user can't strand the cycle empty.
    const enabledCount = cycleModes.filter((m) => s.settings.glassesModes[m.id] !== false).length;
    for (const mode of cycleModes) {
      const enabled = s.settings.glassesModes[mode.id] !== false;
      const isLastEnabled = enabled && enabledCount === 1;
      const row = document.createElement("label");
      row.className = "settings-glasses-toggle-row";
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = enabled;
      cb.disabled = isLastEnabled;
      cb.addEventListener("change", () => {
        void onToggle(mode.id, cb.checked);
      });
      const lbl = document.createElement("span");
      lbl.className = "settings-glasses-toggle-label";
      lbl.textContent = mode.label;
      row.append(cb, lbl);
      if (isLastEnabled) {
        const note = document.createElement("span");
        note.className = "settings-muted settings-glasses-toggle-note";
        note.textContent = "(at least one must stay on)";
        row.appendChild(note);
      }
      glassesToggles.appendChild(row);
    }
  }

  async function onToggle(modeId: string, nextEnabled: boolean): Promise<void> {
    const s = store.get();
    // Compose the new map. Explicitly write `true` (rather than
    // deleting the key) so the persisted blob is the literal truth
    // table — easier to debug, and `loadSettings` doesn't have to
    // guess which keys "absent means yes".
    const nextModes: Record<string, boolean> = { ...s.settings.glassesModes };
    nextModes[modeId] = nextEnabled;

    // Auto-advance if disabling the mode the user is currently
    // looking at on the glasses. Pick the first still-enabled
    // cycle-eligible mode; if none qualify (shouldn't happen given
    // the last-enabled guard above, but defensive) leave the
    // current mode alone.
    let nextGlassesMode = s.glassesCurrentMode;
    if (!nextEnabled && modeId === s.glassesCurrentMode) {
      const candidates = s.availableModes
        .filter((m) => !GLASSES_CYCLE_HIDDEN.has(m.id))
        .map((m) => m.id)
        .filter((id) => nextModes[id] !== false);
      if (candidates.length > 0 && candidates[0]) {
        nextGlassesMode = candidates[0];
      }
    }

    store.update({
      settings: { ...s.settings, glassesModes: nextModes },
      glassesCurrentMode: nextGlassesMode,
    });
    await saveSetting(bridge, "glassesModes", nextModes);
  }

  // === ABOUT CARD ===
  const aboutCard = document.createElement("section");
  aboutCard.className = "settings-card";
  const aboutLabel = document.createElement("div");
  aboutLabel.className = "settings-card-label";
  aboutLabel.textContent = "ABOUT";
  const aboutBody = document.createElement("p");
  aboutBody.className = "settings-card-body";
  aboutBody.textContent =
    "auris listens to your meetings — on mac, on the web, and now on your phone. crafted by tiago oliveira.";
  const aboutLink = document.createElement("a");
  aboutLink.className = "settings-about-link";
  aboutLink.href = "https://github.com/tiagodeoliveira/auris";
  aboutLink.target = "_blank";
  aboutLink.rel = "noopener noreferrer";
  aboutLink.textContent = "github.com/tiagodeoliveira/auris";
  // Connected-device row — the glasses' own serial ("<sn> (G2)"), so
  // the user can tell a real pair from the simulator (each reports a
  // distinct serial) and confirm which device they're paired to before
  // unpairing. Resolved async on open; reads "not connected" in
  // prototype mode (plain browser tab / glasses offline).
  const aboutDevice = document.createElement("div");
  aboutDevice.className = "settings-account-meta settings-account-userid";
  aboutDevice.textContent = "device · …";
  aboutCard.append(aboutLabel, aboutBody, aboutLink, aboutDevice);
  modal.appendChild(aboutCard);

  function refreshDeviceLine(): void {
    void resolveDeviceLabel(bridge).then((label) => {
      aboutDevice.textContent = label ? `device · ${label}` : "device · not connected";
    });
  }

  // === VISIBILITY WIRING ===
  // We populate the email on every open so a sign-in / sign-out
  // round-trip refreshes the displayed identity.
  let wasOpen = false;
  function syncOpenState() {
    const s = store.get();
    const isOpen = s.settingsModalOpen;
    overlay.classList.toggle("open", isOpen);
    if (isOpen && !wasOpen) {
      const id = s.auth;
      accountEmail.textContent = id?.email ?? id?.name ?? id?.sub ?? "(not signed in)";
      accountProvider.textContent = id?.sub ? `via ${providerLabel(id.sub)}` : "";
      accountUserId.textContent = id?.sub ? userIdTail(id.sub) : "";
      // Re-resolve on each open so connecting/disconnecting the glasses
      // between opens is reflected without a reload.
      refreshDeviceLine();
    }
    wasOpen = isOpen;
  }
  syncOpenState();
  store.subscribe((s) => s.settingsModalOpen, syncOpenState);
  store.subscribe((s) => (s.auth ? s.auth.sub : ""), syncOpenState);
  // Re-render the glasses-display toggles when `availableModes`
  // arrives (server-driven, post-WS-handshake) and when the user's
  // own setting changes (so the "last enabled" disabled-attribute
  // updates without a modal close/open).
  renderGlassesToggles();
  store.subscribe(
    (s) =>
      `${s.availableModes.map((m) => m.id).join(",")}|${Object.entries(s.settings.glassesModes)
        .map(([k, v]) => `${k}=${v}`)
        .join(",")}`,
    renderGlassesToggles,
  );
}

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

/// Return the user-id portion of an Auth0 `sub` (the part after
/// `|`). Falls back to the full sub if no separator is present.
function userIdTail(sub: string): string {
  const idx = sub.indexOf("|");
  return idx === -1 ? sub : sub.slice(idx + 1);
}

/// Auris brand mark — duplicates the SVG built in `top-bar.ts` so
/// the settings modal doesn't depend on UI-tree ordering. Slightly
/// larger viewBox-relative stroke so it reads at 52px without
/// thinning out.
function makeBrandMark(): SVGSVGElement {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 44 44");
  svg.setAttribute("aria-hidden", "true");
  const outer = document.createElementNS(NS, "path");
  outer.setAttribute("d", "M 26 8 A 14 14 0 0 0 26 36");
  outer.setAttribute("fill", "none");
  outer.setAttribute("stroke", "currentColor");
  outer.setAttribute("stroke-width", "3");
  outer.setAttribute("stroke-linecap", "round");
  svg.appendChild(outer);
  const inner = document.createElementNS(NS, "path");
  inner.setAttribute("d", "M 26 14 A 8 8 0 0 0 26 30");
  inner.setAttribute("fill", "none");
  inner.setAttribute("stroke", "currentColor");
  inner.setAttribute("stroke-width", "3");
  inner.setAttribute("stroke-linecap", "round");
  svg.appendChild(inner);
  const dot = document.createElementNS(NS, "circle");
  dot.setAttribute("cx", "22");
  dot.setAttribute("cy", "22");
  dot.setAttribute("r", "2.5");
  dot.setAttribute("fill", "var(--brand-coral)");
  svg.appendChild(dot);
  return svg;
}
