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

  // Loading state: disable input + button while a chat round-trip
  // is in flight. Detected by the presence of an item whose
  // `meta.pending === true` in chat-mode items — set by the
  // optimistic-echo on submit, cleared when the server's
  // ItemsUpdate replaces the placeholder with the real Q+A pair.
  function isPending(): boolean {
    const items = store.get().itemsByMode.chat ?? [];
    return items.some(
      (it) => (it.meta as Record<string, unknown> | null | undefined)?.pending === true,
    );
  }

  function setBusy(busy: boolean) {
    input.disabled = busy;
    submit.disabled = busy;
    wrap.classList.toggle("chat-input-busy", busy);
  }

  wrap.addEventListener("submit", (e) => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    setBusy(true);
    // Optimistic echo: push the user's question + a "thinking…"
    // assistant placeholder into chat-mode items immediately, so
    // the user sees their question lock in instead of staring at
    // an empty pane for 3-10 s while the agent thinks. The
    // server's ItemsUpdate eventually overwrites both with the
    // real Q+A pair via the standard handler.
    const optimisticItems = [
      {
        id: `chat-q-pending-${Date.now()}`,
        text,
        t: 0,
        meta: { role: "user" },
      },
      {
        id: `chat-a-pending-${Date.now()}`,
        text: "Thinking…",
        t: 0,
        meta: { role: "assistant", pending: true },
      },
    ];
    store.update({
      itemsByMode: { ...store.get().itemsByMode, chat: optimisticItems },
    });
    send({ type: "chat", text });
    input.value = "";
  });

  function render() {
    const s = store.get();
    const visible =
      s.currentMode === "chat" && (s.meetingState === "active" || s.meetingState === "paused");
    wrap.style.display = visible ? "flex" : "none";

    // Sync busy state to whether a pending bubble is still present.
    // Server's ItemsUpdate replaces our optimistic placeholders
    // with real Q+A items (no `meta.pending` flag) → we re-enable.
    setBusy(isPending());

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
