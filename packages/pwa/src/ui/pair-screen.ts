//! Pre-auth landing screen for the device-pairing flow.
//!
//! Replaces the old Auth0 redirect login. There are two ways the
//! user reaches this screen:
//!
//!   1. First-time pair — no tokens in localStorage. Plain landing.
//!   2. Re-pair after token expiry / revocation — `bannerText` is
//!      set so the user sees what happened ("Your session expired").
//!
//! Submitting the code calls `auth.redeem(code)`; on success, the
//! caller (main.ts) tears this screen down and mounts the
//! authenticated UI.

import type { AuthBundle } from "../auth";

/// Canonical code length — matches the server's `pairing::CODE_LEN`.
const CODE_LEN = 8;

/// Format an 8-char canonical code as `XXXX-XXXX`. Drives both the
/// on-the-fly input formatting and the placeholder rendering.
function formatDisplay(code: string): string {
  const upper = code.toUpperCase().replace(/[^A-Z0-9]/g, "");
  const trimmed = upper.slice(0, CODE_LEN);
  if (trimmed.length <= CODE_LEN / 2) return trimmed;
  return `${trimmed.slice(0, CODE_LEN / 2)}-${trimmed.slice(CODE_LEN / 2)}`;
}

/// Strip non-alphanumerics and uppercase — gives the canonical form
/// the server expects. Caller submits this, not the display form.
function canonicalize(code: string): string {
  return code
    .toUpperCase()
    .replace(/[^A-Z0-9]/g, "")
    .slice(0, CODE_LEN);
}

/// Empty an element by removing children. Safer than `innerHTML = ""`
/// in security-tooling eyes (no innerHTML write at all) and faster
/// in browsers that special-case the children API.
function emptyElement(el: HTMLElement): void {
  while (el.firstChild) el.removeChild(el.firstChild);
}

interface MountOptions {
  /// Optional banner text shown above the title — used when we
  /// kicked the user back here because their tokens died (revoked
  /// device, expired refresh).
  bannerText?: string;
  /// Called after a successful redeem. The caller hands off to the
  /// authenticated boot path; this component has nothing to do
  /// after submitting.
  onPaired: () => void;
}

export function mountPairScreen(parent: HTMLElement, auth: AuthBundle, opts: MountOptions): void {
  emptyElement(parent);

  const wrap = document.createElement("section");
  wrap.className = "pair-screen";

  if (opts.bannerText) {
    const banner = document.createElement("div");
    banner.className = "pair-screen-banner";
    banner.textContent = opts.bannerText;
    wrap.appendChild(banner);
  }

  const title = document.createElement("h1");
  title.className = "pair-screen-title";
  title.textContent = "Auris";

  const subtitle = document.createElement("p");
  subtitle.className = "pair-screen-subtitle";
  subtitle.textContent = "Pair this device to start capturing meetings.";

  const helper = document.createElement("p");
  helper.className = "pair-screen-helper";
  helper.textContent =
    "Open Auris on your phone, tap “Pair new device” in Settings, then enter the code below.";

  const input = document.createElement("input");
  input.className = "pair-screen-input";
  input.type = "text";
  input.inputMode = "text";
  input.autocomplete = "off";
  input.autocapitalize = "characters";
  input.spellcheck = false;
  input.placeholder = "XXXX-XXXX";
  // maxLength accounts for the inserted hyphen — 9 chars on screen.
  input.maxLength = CODE_LEN + 1;
  input.setAttribute("aria-label", "Pairing code");

  // Live-format as the user types: keep the cursor at the end after
  // each keystroke (cheaper than computing the exact post-edit
  // caret position, and acceptable UX for an 8-char field that
  // users mostly type left-to-right).
  input.addEventListener("input", () => {
    const formatted = formatDisplay(input.value);
    if (formatted !== input.value) {
      input.value = formatted;
      input.setSelectionRange(formatted.length, formatted.length);
    }
    updateSubmitState();
  });

  // Feature-detect the async Clipboard API. EvenHub's WebView is
  // stripped down and frequently doesn't expose `navigator.clipboard`
  // at all (iOS WKWebView gating + secure-context rules). When the
  // API isn't available, hide the button entirely and rely on the
  // long-press → paste affordance the input already supports
  // natively — pointing fingers at a non-functional button is worse
  // UX than just not offering it.
  const clipboardAvailable =
    typeof navigator !== "undefined" && typeof navigator.clipboard?.readText === "function";

  const pasteBtn = document.createElement("button");
  pasteBtn.type = "button";
  pasteBtn.className = "pair-screen-paste";
  pasteBtn.textContent = "Paste from phone";
  if (!clipboardAvailable) {
    pasteBtn.style.display = "none";
  }
  pasteBtn.addEventListener("click", async () => {
    try {
      const text = await navigator.clipboard.readText();
      input.value = formatDisplay(text);
      input.dispatchEvent(new Event("input"));
      input.focus();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't read clipboard.");
    }
  });

  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = "btn-primary pair-screen-submit";
  submit.textContent = "Pair";

  const errorEl = document.createElement("p");
  errorEl.className = "pair-screen-error";
  errorEl.style.visibility = "hidden";
  errorEl.textContent = " "; // reserve space so layout doesn't jump

  function setError(text: string): void {
    errorEl.textContent = text;
    errorEl.style.visibility = "visible";
  }
  function clearError(): void {
    errorEl.style.visibility = "hidden";
  }

  function updateSubmitState(): void {
    submit.disabled = canonicalize(input.value).length !== CODE_LEN;
  }
  updateSubmitState();

  const form = document.createElement("form");
  form.className = "pair-screen-form";
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const code = canonicalize(input.value);
    if (code.length !== CODE_LEN) return;
    clearError();
    submit.disabled = true;
    submit.textContent = "Pairing…";
    try {
      await auth.redeem(code);
      opts.onPaired();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Pairing failed.");
      submit.disabled = false;
      submit.textContent = "Pair";
    }
  });
  form.append(input, pasteBtn, submit, errorEl);

  wrap.append(title, subtitle, helper, form);
  parent.appendChild(wrap);

  // Defer focus to next tick so the WebView doesn't pop the on-screen
  // keyboard before the screen has finished painting (looks janky on
  // first launch).
  setTimeout(() => input.focus(), 0);
}
