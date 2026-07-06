//! Items pane for the active meeting.
//!
//! The render loop diffs against existing DOM keyed by `item.id`
//! rather than rebuilding `pane.innerHTML` on every store change.
//! That preserves node identity for unchanged rows — which lets the
//! `items-fade` CSS animation play once on append (the desired
//! behavior) rather than restarting on every interim-transcript tick
//! (which is why the animation was disabled previously).

import type { Store } from "../store";
import type { Item, Intent } from "../types";
import { activeItems } from "../types";
import { renderChatMarkdown } from "./markdown";

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
  // across all modes.
  //
  // Two sets so cross-client auto-expand works without overriding
  // explicit user collapses:
  //   - expandedIds:        explicitly opened (by chevron click)
  //   - manuallyCollapsed:  explicitly closed by the user
  // An item is rendered expanded iff:
  //   expandedIds.has(id) || (item.detail && !manuallyCollapsed.has(id))
  // — so when the OTHER client expands an item (its detail flows in
  // via item_updated), THIS client auto-opens it on the next render
  // unless the user has already collapsed it locally.
  const expandedIds = new Set<string>();
  const manuallyCollapsed = new Set<string>();

  // Diffing state — the single source of truth for what's currently
  // mounted in `pane`. Keyed by item.id; preserved across renders
  // unless the row's visible signature changes (text / detail /
  // expanded-state / meta).
  const rowNodes = new Map<string, HTMLElement>();
  let emptyRow: HTMLElement | null = null;
  let liveRow: HTMLElement | null = null;
  // Sentinels that force a full rebuild — switching mode or the
  // active/paused boundary changes the render path entirely
  // (chat-bubble vs .item, or hidden-pane vs visible-pane), and
  // diffing across those transitions isn't worth the complexity.
  let lastMode: string | null = null;
  let lastMeetingState: string | null = null;

  function isEffectivelyExpanded(item: Item): boolean {
    if (manuallyCollapsed.has(item.id)) return false;
    if (expandedIds.has(item.id)) return true;
    return !!item.detail && item.detail.length > 0;
  }

  function toggleExpanded(item: Item) {
    if (isEffectivelyExpanded(item)) {
      expandedIds.delete(item.id);
      manuallyCollapsed.add(item.id);
    } else {
      expandedIds.add(item.id);
      manuallyCollapsed.delete(item.id);
      if (!item.detail || item.detail.length === 0) {
        send({ type: "expand_item", item_id: item.id });
      }
    }
    render();
  }

  function rowSignature(mode: string, item: Item, expanded: boolean): string {
    const meta = item.meta as Record<string, unknown> | null | undefined;
    return [
      item.text,
      item.detail ?? "",
      expanded ? "1" : "0",
      String(item.t),
      meta ? JSON.stringify(meta) : "",
      mode,
    ].join("");
  }

  function chatBubbleSignature(item: Item): string {
    const meta = item.meta as Record<string, unknown> | null | undefined;
    const role = (meta?.role as string) ?? "assistant";
    const pending = meta?.pending === true;
    return [item.text, role, pending ? "1" : "0"].join("");
  }

  function buildItemRow(mode: string, item: Item): HTMLElement {
    const row = document.createElement("article");
    row.className = "item";

    const time = document.createElement("div");
    time.className = "item-time";
    time.textContent = `[${fmtTime(item.t)}]`;
    row.appendChild(time);

    const body = document.createElement("div");
    body.className = "item-body";
    // Assist items prefix the headline with a type chip glyph so
    // the four sub-types are distinguishable at a glance:
    //   📖 definition   ❓ question
    //   🧠 memory       💡 coach
    // The chip is rendered as a leading character on the body text
    // rather than a separate element so it inherits the existing
    // layout and signature-diff machinery for free.
    if (mode === "assist") {
      const meta = item.meta as Record<string, unknown> | null | undefined;
      const t = (meta?.type as string | undefined) ?? "";
      const glyph =
        t === "definition"
          ? "📖"
          : t === "question"
            ? "❓"
            : t === "memory"
              ? "🧠"
              : t === "coach"
                ? "💡"
                : "";
      body.textContent = glyph ? `${glyph}  ${item.text}` : item.text;
    } else {
      body.textContent = item.text;
    }
    row.appendChild(body);

    const expanded = isEffectivelyExpanded(item);
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

    const metaText = renderItemMeta(mode, item);
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

    return row;
  }

  function buildChatBubble(item: Item): HTMLElement {
    const meta = item.meta as Record<string, unknown> | null | undefined;
    const role = (meta?.role as string) ?? "assistant";
    // Treat role="assistant-pending" as pending too (server-emitted
    // optimistic placeholder). The older meta.pending flag is kept
    // as a fallback for any legacy emission path. Also render dots
    // during the brief window where the server has flipped role to
    // "assistant" + meta.streaming = true but no text has accumulated
    // yet (≤500ms before the first delta lands).
    const streaming = meta?.streaming === true;
    const pending =
      role === "assistant-pending" ||
      meta?.pending === true ||
      (role === "assistant" && streaming && (item.text ?? "").length === 0);
    const row = document.createElement("article");
    // Normalize the class to chat-bubble-assistant when pending so
    // the existing assistant styling applies and we only need one
    // extra CSS rule for the typing indicator.
    const classRole = role === "assistant-pending" ? "assistant" : role;
    row.className = `chat-bubble chat-bubble-${classRole}${pending ? " chat-bubble-pending" : ""}`;
    const body = document.createElement("div");
    body.className = "chat-bubble-body";
    if (pending) {
      // Animated typing indicator — three pulsing dots staggered by
      // CSS keyframe delay. No content text inside; the dots ARE
      // the content until the agent's real reply lands.
      const dots = document.createElement("span");
      dots.className = "typing-dots";
      dots.setAttribute("aria-label", "Agent is thinking");
      for (let i = 0; i < 3; i++) {
        const dot = document.createElement("span");
        dot.className = "typing-dot";
        dots.appendChild(dot);
      }
      body.appendChild(dots);
    } else if (role === "assistant") {
      body.innerHTML = renderChatMarkdown(item.text);
    } else {
      body.textContent = item.text;
    }
    row.appendChild(body);
    // Attachment cue on user bubbles that rode screenshot(s): a photo
    // glyph, plus the count when more than one. We don't render the
    // images — just signal they were attached.
    const attachmentIds = meta?.attachment_ids;
    if (role === "user" && Array.isArray(attachmentIds) && attachmentIds.length > 0) {
      row.appendChild(buildChatAttachmentBadge(attachmentIds.length));
    }
    return row;
  }

  /** Small "had N image attachment(s)" badge. 🖼 alone for one, with a
   * count for several. */
  function buildChatAttachmentBadge(count: number): HTMLElement {
    const badge = document.createElement("span");
    badge.className = "chat-attachments-badge";
    badge.textContent = count > 1 ? `🖼 ${count}` : "🖼";
    badge.setAttribute("aria-label", `${count} image attachment${count > 1 ? "s" : ""}`);
    return badge;
  }

  function buildEmptyRow(mode: string): HTMLElement {
    const empty = document.createElement("div");
    empty.className = "items-empty label-mono";
    empty.textContent =
      mode === "chat" ? "─ ask the agent anything" : `─ no ${mode.replace("_", " ")} yet`;
    return empty;
  }

  function buildLiveRow(interim: string): HTMLElement {
    const live = document.createElement("article");
    live.className = "item item-live";
    const liveTime = document.createElement("div");
    liveTime.className = "item-time";
    liveTime.textContent = "[ ⋯ ]";
    live.appendChild(liveTime);
    const liveBody = document.createElement("div");
    liveBody.className = "item-body";
    liveBody.textContent = interim;
    live.appendChild(liveBody);
    return live;
  }

  function clearAll() {
    pane.innerHTML = "";
    rowNodes.clear();
    emptyRow = null;
    liveRow = null;
  }

  function render() {
    const s = store.get();
    if (s.meetingState !== "active") {
      pane.style.display = "none";
      // Don't clear nodes here — when the user re-enters an active
      // meeting we'll restart from a fresh `lastMeetingState` anyway.
      return;
    }
    pane.style.display = "block";

    if (lastMode !== s.currentMode || lastMeetingState !== s.meetingState) {
      clearAll();
      lastMode = s.currentMode;
      lastMeetingState = s.meetingState;
    }

    const items = activeItems(s);
    const showLive =
      s.currentMode === "transcript" &&
      s.meetingState === "active" &&
      s.liveTranscriptInterim.trim().length > 0;

    // Empty state — collapse everything to a single placeholder.
    if (items.length === 0 && !showLive) {
      for (const [id, node] of rowNodes) {
        node.remove();
        rowNodes.delete(id);
      }
      if (liveRow) {
        liveRow.remove();
        liveRow = null;
      }
      if (!emptyRow) {
        emptyRow = buildEmptyRow(s.currentMode);
        pane.appendChild(emptyRow);
      }
      return;
    } else if (emptyRow) {
      emptyRow.remove();
      emptyRow = null;
    }

    // Drop orphaned rows (id no longer in the items list).
    const desiredIds = new Set(items.map((i) => i.id));
    for (const [id, node] of rowNodes) {
      if (!desiredIds.has(id)) {
        node.remove();
        rowNodes.delete(id);
      }
    }

    // Insert / update / reposition rows in the desired order.
    const isChat = s.currentMode === "chat";
    let prevNode: HTMLElement | null = null;
    for (const item of items) {
      const sig = isChat
        ? chatBubbleSignature(item)
        : rowSignature(s.currentMode, item, isEffectivelyExpanded(item));
      let node = rowNodes.get(item.id);

      if (!node || node.dataset.sig !== sig) {
        const fresh = isChat ? buildChatBubble(item) : buildItemRow(s.currentMode, item);
        fresh.dataset.sig = sig;
        if (node) {
          node.replaceWith(fresh);
        }
        rowNodes.set(item.id, fresh);
        node = fresh;
      }

      // Ensure correct position. Only reposition when off — calling
      // .after() / .prepend() on a node already in place would still
      // move it (remove + re-insert), restarting CSS animations.
      if (node.parentNode !== pane || node.previousElementSibling !== prevNode) {
        if (prevNode) prevNode.after(node);
        else pane.prepend(node);
      }
      prevNode = node;
    }

    // Live transcript row (singleton) goes last.
    if (showLive) {
      if (!liveRow) {
        liveRow = buildLiveRow(s.liveTranscriptInterim);
        if (prevNode) prevNode.after(liveRow);
        else pane.prepend(liveRow);
      } else {
        const liveBody = liveRow.querySelector<HTMLElement>(".item-body");
        if (liveBody) liveBody.textContent = s.liveTranscriptInterim;
        if (liveRow.previousElementSibling !== prevNode || liveRow.parentNode !== pane) {
          if (prevNode) prevNode.after(liveRow);
          else pane.prepend(liveRow);
        }
      }
    } else if (liveRow) {
      liveRow.remove();
      liveRow = null;
    }

    // Auto-scroll to bottom for live-append modes.
    if (s.currentMode === "transcript" || isChat) {
      pane.scrollTop = pane.scrollHeight;
    }
  }

  render();
  store.subscribe((s) => s.meetingState, render);
  store.subscribe((s) => s.currentMode, render);
  // Gate the interim-transcript subscription on the current mode.
  // Interim text updates several times per second during active
  // speech; without the gate, every chat / summary / etc. re-render
  // would touch the pane on each interim packet.
  store.subscribe((s) => (s.currentMode === "transcript" ? s.liveTranscriptInterim : ""), render);
  // Subscribe to the items-array reference for the current mode.
  // `apply-items-update.ts` returns a NEW array on every items_update
  // (it starts with `current.slice()` and mutates the copy), so the
  // array reference is a reliable signal of "content changed somehow"
  // — including in-place upserts by id, where length and last-id
  // stay constant but item bodies mutate. The earlier hand-rolled
  // signature `length|lastId|detailSig` missed the chat-mode case
  // where an assistant-pending bubble's text gets replaced in
  // place with the real response (same id, same length, no detail
  // change) → no re-render → DOM frozen on "...".
  store.subscribe((s) => s.itemsByMode[s.currentMode], render);
}
