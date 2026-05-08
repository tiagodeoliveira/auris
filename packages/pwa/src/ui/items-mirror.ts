//! Items pane for the active meeting.

import type { Store } from "../store";
import type { Item } from "../types";
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

export function mountItemsMirror(parent: HTMLElement, store: Store): void {
  const pane = document.createElement("div");
  pane.className = "items-pane";
  parent.appendChild(pane);

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
        const role =
          ((item.meta as Record<string, unknown> | null | undefined)?.role as string) ??
          "assistant";
        const row = document.createElement("article");
        row.className = `chat-bubble chat-bubble-${role}`;
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

      const metaText = renderItemMeta(s.currentMode, item);
      if (metaText) {
        const meta = document.createElement("div");
        meta.className = "item-meta label-mono";
        meta.textContent = metaText;
        row.appendChild(meta);
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
  store.subscribe((s) => s.liveTranscriptInterim, render);
  store.subscribe(
    (s) =>
      `${s.itemsByMode[s.currentMode]?.length ?? 0}|${s.itemsByMode[s.currentMode]?.[s.itemsByMode[s.currentMode].length - 1]?.id ?? ""}`,
    render,
  );
}
