//! Items pane for the active meeting. See `docs/specs/pwa-ux-redesign.md` §3.4.

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

    if (items.length === 0) {
      const empty = document.createElement("div");
      empty.className = "items-empty label-mono";
      empty.textContent = `─ no ${s.currentMode.replace("_", " ")} yet`;
      pane.appendChild(empty);
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

    // Auto-scroll to bottom for transcript mode (live append).
    if (s.currentMode === "transcript") {
      pane.scrollTop = pane.scrollHeight;
    }
  }

  render();
  store.subscribe((s) => s.meetingState, render);
  store.subscribe((s) => s.currentMode, render);
  store.subscribe(
    (s) =>
      `${s.itemsByMode[s.currentMode]?.length ?? 0}|${s.itemsByMode[s.currentMode]?.[s.itemsByMode[s.currentMode].length - 1]?.id ?? ""}`,
    render,
  );
}
