//! Items pane for the active meeting.

import type { Store } from "../store";
import type { Item, Intent } from "../types";
import { activeItems } from "../types";

function fmtTime(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60)
    .toString()
    .padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}

function renderItemMeta(mode: string, item: Item): string {
  const meta = item.meta as Record<string, unknown> | null | undefined;
  if (!meta) return "";
  if (mode === "actions") {
    const owner = meta.owner ? `OWNER · ${meta.owner}` : "";
    const due = meta.due ? `DUE · ${meta.due}` : "";
    return [owner, due].filter(Boolean).join(" · ");
  }
  if (mode === "highlights") {
    return meta.importance ? `IMPORTANCE · ${meta.importance}` : "";
  }
  if (mode === "open_questions") {
    const kind = (meta.kind as string)?.toUpperCase() ?? "";
    const ctx = meta.context ? ` · ${meta.context}` : "";
    return kind ? `${kind}${ctx}` : "";
  }
  if (mode === "transcript" && meta.speaker) {
    return `SPEAKER · ${meta.speaker}`;
  }
  return "";
}

export function mountItemsMirror(
  parent: HTMLElement,
  store: Store,
  send: (i: Intent) => void,
): void {
  const pane = document.createElement("div");
  pane.className = "items-pane";
  parent.appendChild(pane);

  // Per-item expand state. Local to this items-mirror instance —
  // not in the store because it's pure UI ephemera (mode-switch
  // doesn't need to preserve which row was expanded). Tracks ids
  // across all modes; cleared on meeting-state transitions
  // through a subscriber below.
  const expandedIds = new Set<string>();

  function toggleExpanded(item: Item) {
    if (expandedIds.has(item.id)) {
      expandedIds.delete(item.id);
    } else {
      expandedIds.add(item.id);
      // First expand on an item without detail → ask the agent.
      // The reply arrives via `item_updated` and the renderer
      // swaps the placeholder for the real expansion.
      if (!item.detail || item.detail.length === 0) {
        send({ type: "expand_item", item_id: item.id });
      }
    }
    render();
  }

  function render() {
    const s = store.get();
    if (s.meetingState !== "active" && s.meetingState !== "paused") {
      pane.style.display = "none";
      return;
    }
    pane.style.display = "block";
    const items = activeItems(s);
    pane.innerHTML = "";

    const showLive =
      s.currentMode === "transcript" &&
      s.meetingState === "active" &&
      s.liveTranscriptInterim.trim().length > 0;

    if (items.length === 0 && !showLive) {
      const empty = document.createElement("div");
      empty.className = "items-empty label-mono";
      const placeholder =
        s.currentMode === "chat"
          ? "─ ask the agent anything"
          : `─ no ${s.currentMode.replace("_", " ")} yet`;
      empty.textContent = placeholder;
      pane.appendChild(empty);
      return;
    }

    // Chat mode renders bubble-style with role-aware alignment.
    // Q+A pairs replace each other on each exchange; no thread.
    if (s.currentMode === "chat") {
      for (const item of items) {
        const meta = item.meta as Record<string, unknown> | null | undefined;
        const role = (meta?.role as string) ?? "assistant";
        const pending = meta?.pending === true;
        const row = document.createElement("article");
        row.className = `chat-bubble chat-bubble-${role}${pending ? " chat-bubble-pending" : ""}`;
        const body = document.createElement("div");
        body.className = "chat-bubble-body";
        body.textContent = item.text;
        row.appendChild(body);
        pane.appendChild(row);
      }
      pane.scrollTop = pane.scrollHeight;
      return;
    }

    for (const item of items) {
      const row = document.createElement("article");
      row.className = "item";

      const time = document.createElement("div");
      time.className = "item-time";
      time.textContent = `[${fmtTime(item.t)}]`;
      row.appendChild(time);

      const body = document.createElement("div");
      body.className = "item-body";
      body.textContent = item.text;
      row.appendChild(body);

      // Chevron toggle — always present so the user can ask the
      // agent to expand any item. First click on an item without
      // detail fires `expand_item`; subsequent clicks toggle the
      // panel locally without re-billing.
      const expanded = expandedIds.has(item.id);
      const chevron = document.createElement("button");
      chevron.type = "button";
      chevron.className = "item-chevron";
      chevron.textContent = expanded ? "▾" : "▸";
      chevron.title = expanded ? "Hide detail" : "Show detail";
      chevron.addEventListener("click", (e) => {
        e.stopPropagation();
        toggleExpanded(item);
      });
      row.appendChild(chevron);

      const metaText = renderItemMeta(s.currentMode, item);
      if (metaText) {
        const meta = document.createElement("div");
        meta.className = "item-meta label-mono";
        meta.textContent = metaText;
        row.appendChild(meta);
      }

      if (expanded) {
        const detailRow = document.createElement("div");
        detailRow.className = "item-detail";
        if (item.detail && item.detail.length > 0) {
          detailRow.textContent = item.detail;
        } else {
          detailRow.classList.add("item-detail-pending");
          detailRow.textContent = "Expanding…";
        }
        row.appendChild(detailRow);
      }

      pane.appendChild(row);
    }

    // "Live" row showing the in-flight transcript before sentence-end
    // promotes it to a real Item. Only visible in transcript mode while
    // the meeting is actively capturing audio.
    if (showLive) {
      const live = document.createElement("article");
      live.className = "item item-live";
      const liveTime = document.createElement("div");
      liveTime.className = "item-time";
      liveTime.textContent = "[ ⋯ ]";
      live.appendChild(liveTime);
      const liveBody = document.createElement("div");
      liveBody.className = "item-body";
      liveBody.textContent = s.liveTranscriptInterim;
      live.appendChild(liveBody);
      pane.appendChild(live);
    }

    // Auto-scroll to bottom for transcript mode (live append).
    if (s.currentMode === "transcript") {
      pane.scrollTop = pane.scrollHeight;
    }
  }

  render();
  store.subscribe((s) => s.meetingState, render);
  store.subscribe((s) => s.currentMode, render);
  // Gate the interim-transcript subscription on the current mode.
  // Interim text updates several times per second during active
  // speech; without the gate, every chat / summary / etc. re-render
  // would `pane.innerHTML = ""` + full rebuild on each interim
  // packet, flickering the whole pane. The actual interim line is
  // rendered only in transcript mode anyway.
  store.subscribe((s) => (s.currentMode === "transcript" ? s.liveTranscriptInterim : ""), render);
  store.subscribe(
    (s) =>
      `${s.itemsByMode[s.currentMode]?.length ?? 0}|${s.itemsByMode[s.currentMode]?.[s.itemsByMode[s.currentMode].length - 1]?.id ?? ""}`,
    render,
  );
}
