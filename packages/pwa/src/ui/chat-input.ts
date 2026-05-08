//! Chat input row — mounted only when current mode is `chat`.
//!
//! Single-line input + Send button. Submitting fires
//! `Intent::Chat { text }`; the server kicks the agent and the
//! resulting Q+A pair lands in chat-mode items via the standard
//! ItemsUpdate event. No client-side optimistic echo — we wait
//! for the server's reply so the UI reflects what's actually
//! in the agent's history.

import type { Store } from "../store";
import type { Intent } from "../types";

export function mountChatInput(parent: HTMLElement, store: Store, send: (i: Intent) => void): void {
  const wrap = document.createElement("form");
  wrap.className = "chat-input";
  parent.appendChild(wrap);

  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = "Ask the agent…";
  input.autocomplete = "off";
  input.className = "chat-input-text";
  wrap.appendChild(input);

  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = "chat-input-send";
  submit.textContent = "Send";
  wrap.appendChild(submit);

  // Loading state: disable input + button while waiting for the
  // agent's reply. We treat the arrival of a NEW assistant bubble
  // (item with meta.role === "assistant") as the signal that the
  // round-trip completed. Tracked by the count of chat items at
  // submit-time vs current — when it grows, we re-enable.
  let pendingSinceCount: number | null = null;

  function setBusy(busy: boolean) {
    input.disabled = busy;
    submit.disabled = busy;
    wrap.classList.toggle("chat-input-busy", busy);
  }

  wrap.addEventListener("submit", (e) => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    const s = store.get();
    pendingSinceCount = s.itemsByMode.chat?.length ?? 0;
    setBusy(true);
    send({ type: "chat", text });
    input.value = "";
  });

  function render() {
    const s = store.get();
    const visible =
      s.currentMode === "chat" && (s.meetingState === "active" || s.meetingState === "paused");
    wrap.style.display = visible ? "flex" : "none";

    // Detect the round-trip completion: chat items grew past the
    // pending threshold.
    if (pendingSinceCount !== null) {
      const cur = s.itemsByMode.chat?.length ?? 0;
      if (cur > pendingSinceCount) {
        setBusy(false);
        pendingSinceCount = null;
      }
    }

    if (visible && !input.disabled) {
      // Convenience: focus the input on tab activation.
      // setTimeout 0 to let mode-switch render settle.
      setTimeout(() => input.focus(), 0);
    }
  }

  render();
  store.subscribe((s) => `${s.currentMode}|${s.meetingState}`, render);
  store.subscribe((s) => s.itemsByMode.chat?.length ?? 0, render);
}
