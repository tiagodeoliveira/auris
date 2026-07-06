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
  const wrap = document.createElement("div");
  wrap.className = "chat-input-wrap";
  parent.appendChild(wrap);

  // Quick-asks chip row — saved prompts the user can fire with one
  // click. Chips read from `itemsByMode["quick_asks"]`, which the
  // server keeps in sync via items_update broadcasts. Clicks dispatch
  // the same `Intent::Chat` the textarea uses, just with the snippet's
  // full text instead of typed input.
  const chips = document.createElement("div");
  chips.className = "chat-input-chips";
  chips.style.display = "flex";
  chips.style.gap = "6px";
  chips.style.flexWrap = "wrap";
  chips.style.marginBottom = "6px";
  wrap.appendChild(chips);

  const form = document.createElement("form");
  form.className = "chat-input";
  wrap.appendChild(form);

  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = "Ask the agent…";
  input.autocomplete = "off";
  input.className = "chat-input-text";
  form.appendChild(input);

  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = "chat-input-send";
  submit.textContent = "Send";
  form.appendChild(submit);

  // Loading state: disable input + button while a chat round-trip
  // is in flight. The server emits a placeholder bubble with
  // `meta.role == "assistant-pending"` the moment a chat intent
  // lands, then flips it to role="assistant" + meta.streaming=true
  // during the stream, then meta.streaming=false on terminal.
  // Lock the input across both phases so the user can't fire a
  // second chat mid-stream.
  //
  // 60s safety timeout: if a WS reconnect mid-stream returns a
  // snapshot with streaming=true but no further deltas arrive,
  // the lock would be stuck forever. After 60s of continuous
  // lock, release locally; if the server later emits a terminal
  // streaming=false, that's harmless.
  let lockObservedAt: number | null = null;
  let lockExpireTimer: ReturnType<typeof setTimeout> | null = null;
  const LOCK_TIMEOUT_MS = 60_000;

  function hasPendingItem(): boolean {
    const items = store.get().itemsByMode.chat ?? [];
    return items.some((it) => {
      const m = it.meta as Record<string, unknown> | null | undefined;
      return m?.role === "assistant-pending" || m?.streaming === true;
    });
  }

  function isPending(): boolean {
    if (!hasPendingItem()) {
      // Cleared — reset bookkeeping so the next round starts fresh.
      lockObservedAt = null;
      if (lockExpireTimer !== null) {
        clearTimeout(lockExpireTimer);
        lockExpireTimer = null;
      }
      return false;
    }
    // First observation in this round — arm the safety timer.
    if (lockObservedAt === null) {
      lockObservedAt = Date.now();
      lockExpireTimer = setTimeout(() => {
        lockExpireTimer = null;
        render(); // force a re-evaluation; isPending will return false
      }, LOCK_TIMEOUT_MS);
    }
    return Date.now() - lockObservedAt < LOCK_TIMEOUT_MS;
  }

  function setBusy(busy: boolean) {
    input.disabled = busy;
    submit.disabled = busy;
    form.classList.toggle("chat-input-busy", busy);
  }

  function renderChips(): void {
    while (chips.firstChild) chips.removeChild(chips.firstChild);
    const asks = store.get().itemsByMode.quick_asks ?? [];
    for (const ask of asks) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "chat-input-chip";
      btn.textContent = ask.text; // label is packed into `text` server-side
      const fullPrompt = (ask.detail ?? "").trim();
      btn.title = fullPrompt;
      btn.disabled = isPending() || fullPrompt.length === 0;
      btn.addEventListener("click", () => {
        if (!fullPrompt || isPending()) return;
        send({ type: "chat", text: fullPrompt });
      });
      chips.appendChild(btn);
    }
  }

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    // No client-side optimistic echo — the server emits the user
    // bubble + an `assistant-pending` placeholder the moment the
    // chat intent lands and broadcasts to every connected surface.
    // Adding our OWN bubbles here would create a duplicate pair
    // with mismatched ids that items-mirror.ts's merge-by-id
    // can't reconcile against the server's emission.
    send({ type: "chat", text });
    input.value = "";
  });

  function render() {
    const s = store.get();
    const visible = s.currentMode === "chat" && s.meetingState === "active";
    wrap.style.display = visible ? "" : "none";

    // Sync busy state to whether a pending bubble is still present.
    // Server's ItemsUpdate replaces our optimistic placeholders
    // with real Q+A items (no `meta.pending` flag) → we re-enable.
    setBusy(isPending());
    renderChips();

    if (visible && !input.disabled) {
      // Convenience: focus the input on tab activation.
      // setTimeout 0 to let mode-switch render settle.
      setTimeout(() => input.focus(), 0);
    }
  }

  render();
  store.subscribe((s) => `${s.currentMode}|${s.meetingState}`, render);
  // Subscribe to the array reference, not just its length —
  // `apply-items-update` returns a new array on every items_update
  // (including in-place upserts by id, like the chat-mode
  // pending → final transition). Length-only checks miss that case
  // and leave the input/chips stuck in the busy/disabled state
  // after the assistant-pending placeholder gets replaced by the
  // real response. Same fix as items-mirror.
  store.subscribe((s) => s.itemsByMode.chat, render);
  store.subscribe((s) => s.itemsByMode.quick_asks, render);
}
