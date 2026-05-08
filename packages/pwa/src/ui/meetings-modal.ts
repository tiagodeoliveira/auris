//! Meetings browser modal. Master/detail over `GET /meetings` and
//! `GET /meetings/:id`. Mirrors the Mac app's Settings → Meetings tab.
//!
//! Reuses the `.settings-overlay` / `.settings-modal` styles for the
//! container so it inherits the same chrome the user already knows;
//! the inner master/detail layout is its own thing.

import type { Store } from "../store";
import type { AuthBundle } from "../auth";
import type { Item } from "../types";
import {
  MeetingsApi,
  MeetingsApiError,
  type MeetingDetail,
  type MeetingSummary,
} from "../meetings-api";
import { SERVER_URL } from "../server-url";

export function mountMeetingsModal(parent: HTMLElement, store: Store, auth: AuthBundle): void {
  const overlay = document.createElement("div");
  overlay.className = "settings-overlay meetings-overlay";
  parent.appendChild(overlay);

  const modal = document.createElement("div");
  modal.className = "settings-modal meetings-modal";
  overlay.appendChild(modal);

  // Header. The back button is only meaningful on narrow viewports
  // when a meeting is selected (we hide the list, show the detail) —
  // CSS gates its visibility via the `.meetings-modal[data-pane=...]`
  // attribute below.
  const header = document.createElement("div");
  header.className = "meetings-header";
  const backBtn = document.createElement("button");
  backBtn.className = "btn-ghost meetings-back-btn";
  backBtn.setAttribute("aria-label", "Back to list");
  backBtn.innerHTML = "‹";
  backBtn.addEventListener("click", () => {
    selectedId = null;
    renderList();
    renderDetail(null);
    syncPane();
  });
  const title = document.createElement("h2");
  title.className = "settings-title";
  title.textContent = "Meetings";
  const closeBtn = document.createElement("button");
  closeBtn.className = "btn-ghost";
  closeBtn.textContent = "Close";
  closeBtn.addEventListener("click", () => store.update({ meetingsModalOpen: false }));
  const reloadBtn = document.createElement("button");
  reloadBtn.className = "btn-ghost";
  reloadBtn.textContent = "Reload";
  reloadBtn.addEventListener("click", () => void reloadList());
  header.append(backBtn, title, reloadBtn, closeBtn);
  modal.appendChild(header);

  // Body: list (left) + detail (right)
  const body = document.createElement("div");
  body.className = "meetings-body";
  modal.appendChild(body);

  const listPane = document.createElement("div");
  listPane.className = "meetings-list";
  body.appendChild(listPane);

  const detailPane = document.createElement("div");
  detailPane.className = "meetings-detail";
  body.appendChild(detailPane);

  // Local UI state — kept inside the closure rather than the store
  // because nothing else cares about the open meeting / loading flag.
  let meetings: MeetingSummary[] = [];
  let selectedId: string | null = null;
  let listError: string | null = null;
  let detailError: string | null = null;

  function makeApi(): MeetingsApi | null {
    return MeetingsApi.from(SERVER_URL, () => auth.getAccessToken());
  }

  async function reloadList(): Promise<void> {
    const api = makeApi();
    if (!api) {
      listError = "Server URL or token missing — open Settings.";
      meetings = [];
      renderList();
      return;
    }
    listError = null;
    listPane.classList.add("loading");
    renderList();
    try {
      meetings = await api.list();
    } catch (e) {
      listError = errorMessage(e);
      meetings = [];
    } finally {
      listPane.classList.remove("loading");
      // If the previously-selected meeting is gone, drop it.
      if (selectedId && !meetings.some((m) => m.id === selectedId)) {
        selectedId = null;
        renderDetail(null);
      }
      renderList();
    }
  }

  async function loadDetail(id: string): Promise<void> {
    const api = makeApi();
    if (!api) return;
    selectedId = id;
    detailError = null;
    syncPane();
    renderDetail(null, /* loading */ true);
    try {
      const detail = await api.detail(id);
      renderDetail(detail);
    } catch (e) {
      detailError = errorMessage(e);
      renderDetail(null);
    }
  }

  /// Reflect the master/detail state on the modal so the CSS knows
  /// which pane to show on narrow viewports. `list` = the row picker;
  /// `detail` = a meeting is selected. On wide viewports the CSS
  /// shows both panes regardless of this attribute.
  function syncPane(): void {
    modal.setAttribute("data-pane", selectedId ? "detail" : "list");
  }

  function renderList(): void {
    listPane.innerHTML = "";
    if (listError) {
      const err = document.createElement("div");
      err.className = "meetings-empty";
      err.textContent = listError;
      listPane.appendChild(err);
      return;
    }
    if (meetings.length === 0) {
      const empty = document.createElement("div");
      empty.className = "meetings-empty";
      empty.textContent = "No meetings yet.";
      listPane.appendChild(empty);
      return;
    }
    // Group rows by relative bucket (Today / Yesterday / This week
    // / Older) so a long history scans at a glance. Group order is
    // fixed; rows within a group keep server order (newest first).
    let currentBucket: string | null = null;
    for (const m of meetings) {
      const bucket = relativeBucket(m.started_at);
      if (bucket !== currentBucket) {
        const heading = document.createElement("div");
        heading.className = "meetings-group";
        heading.textContent = bucket;
        listPane.appendChild(heading);
        currentBucket = bucket;
      }
      const row = document.createElement("button");
      row.className = "meetings-row";
      if (m.id === selectedId) row.classList.add("selected");
      row.addEventListener("click", () => void loadDetail(m.id));

      const headline = document.createElement("div");
      headline.className = "meetings-row-title";
      headline.textContent = m.description?.trim() || "Untitled meeting";

      const sub = document.createElement("div");
      sub.className = "meetings-row-sub";
      sub.textContent = `${formatDateShort(m.started_at)} · ${formatDuration(m.started_at, m.ended_at)}`;

      row.append(headline, sub);
      listPane.appendChild(row);
    }
  }

  function renderDetail(detail: MeetingDetail | null, loading = false): void {
    detailPane.innerHTML = "";
    if (loading) {
      const spinner = document.createElement("div");
      spinner.className = "meetings-spinner";
      spinner.setAttribute("aria-label", "Loading meeting");
      detailPane.appendChild(spinner);
      return;
    }
    if (detailError) {
      const err = document.createElement("div");
      err.className = "meetings-empty";
      err.textContent = detailError;
      detailPane.appendChild(err);
      return;
    }
    if (!detail) {
      const hint = document.createElement("div");
      hint.className = "meetings-empty";
      hint.textContent = meetings.length === 0 ? "" : "Select a meeting.";
      detailPane.appendChild(hint);
      return;
    }
    detailPane.appendChild(buildDetailView(detail));
  }

  function buildDetailView(detail: MeetingDetail): HTMLElement {
    const root = document.createElement("div");
    root.className = "meetings-detail-inner";

    const head = document.createElement("h3");
    head.className = "meetings-detail-title";
    head.textContent = detail.description?.trim() || "Untitled meeting";
    root.appendChild(head);

    // Timing row
    const timing = document.createElement("div");
    timing.className = "meetings-detail-timing";
    timing.appendChild(timingCell("Started", formatDateLong(detail.started_at)));
    if (detail.ended_at) {
      timing.appendChild(timingCell("Ended", formatDateLong(detail.ended_at)));
    } else {
      timing.appendChild(timingCell("Status", "in progress", "in-progress"));
    }
    root.appendChild(timing);

    // Metadata
    const metaKeys = Object.keys(detail.metadata).sort();
    if (metaKeys.length > 0) {
      const block = document.createElement("div");
      block.className = "meetings-detail-block";
      const heading = document.createElement("div");
      heading.className = "meetings-detail-heading";
      heading.textContent = "Metadata";
      block.appendChild(heading);
      for (const k of metaKeys) {
        const row = document.createElement("div");
        row.className = "meetings-meta-row";
        const key = document.createElement("span");
        key.className = "meetings-meta-key";
        key.textContent = k;
        const val = document.createElement("span");
        val.className = "meetings-meta-val";
        val.textContent = detail.metadata[k] ?? "";
        row.append(key, val);
        block.appendChild(row);
      }
      root.appendChild(block);
    }

    // Moments
    if (detail.moments && detail.moments.length > 0) {
      const mBlock = document.createElement("div");
      mBlock.className = "meetings-detail-block";
      const mHead = document.createElement("div");
      mHead.className = "meetings-detail-heading";
      mHead.textContent = "Moments";
      mBlock.appendChild(mHead);
      for (const moment of detail.moments) {
        mBlock.appendChild(buildMomentCard(moment));
      }
      root.appendChild(mBlock);
    }

    // LLM usage rollup. Renders directly under metadata so it's
    // immediately visible; collapses to nothing when no usage was
    // recorded (pre-migration meetings or zero-call meetings).
    const usage = detail.llm_usage;
    if (usage && usage.calls > 0) {
      const block = document.createElement("div");
      block.className = "meetings-detail-block";
      const heading = document.createElement("div");
      heading.className = "meetings-detail-heading";
      heading.textContent = "LLM usage";
      block.appendChild(heading);
      const billable = Math.max(0, usage.input_tokens - usage.cached_input_tokens);
      const rows: Array<[string, string]> = [
        ["calls", String(usage.calls)],
        ["input tokens", usage.input_tokens.toLocaleString()],
        ["billable input", billable.toLocaleString()],
        ["cached input", usage.cached_input_tokens.toLocaleString()],
        ["output tokens", usage.output_tokens.toLocaleString()],
      ];
      if (usage.model_id) {
        rows.push(["model", usage.model_id]);
      }
      if (usage.provider) {
        rows.push(["provider", usage.provider]);
      }
      for (const [k, v] of rows) {
        const row = document.createElement("div");
        row.className = "meetings-meta-row";
        const key = document.createElement("span");
        key.className = "meetings-meta-key";
        key.textContent = k;
        const val = document.createElement("span");
        val.className = "meetings-meta-val";
        val.textContent = v;
        row.append(key, val);
        block.appendChild(row);
      }
      root.appendChild(block);
    }

    // Per-mode persisted items (highlights / actions /
    // open_questions / summary / chat). Render in a fixed order
    // matching the live overlay's tab order so the meeting-detail
    // view feels familiar. Mode is omitted entirely when there
    // are no items for it.
    const items_by_mode = detail.items_by_mode ?? {};
    const MODE_ORDER: Array<{ id: string; label: string }> = [
      { id: "highlights", label: "Highlights" },
      { id: "actions", label: "Actions" },
      { id: "open_questions", label: "Open questions" },
      { id: "summary", label: "Summary" },
      { id: "chat", label: "Chat" },
    ];
    for (const { id, label } of MODE_ORDER) {
      const items = items_by_mode[id] ?? [];
      if (items.length === 0) continue;
      const block = document.createElement("div");
      block.className = "meetings-detail-block";
      const heading = document.createElement("div");
      heading.className = "meetings-detail-heading";
      heading.textContent = label;
      block.appendChild(heading);
      for (const item of items) {
        block.appendChild(buildItemRow(id, item));
      }
      root.appendChild(block);
    }

    // Transcript
    const tBlock = document.createElement("div");
    tBlock.className = "meetings-detail-block";
    const tHead = document.createElement("div");
    tHead.className = "meetings-detail-heading";
    tHead.textContent = "Transcript";
    tBlock.appendChild(tHead);
    if (detail.transcript.length === 0) {
      const empty = document.createElement("div");
      empty.className = "meetings-empty inline";
      empty.textContent = "(no transcript captured)";
      tBlock.appendChild(empty);
    } else {
      for (const item of detail.transcript) {
        const row = document.createElement("div");
        row.className = "meetings-transcript-row";
        const speaker =
          (item.meta as { speaker?: string } | undefined | null)?.speaker?.trim() ?? "";
        if (speaker) {
          const chip = document.createElement("span");
          chip.className = "meetings-transcript-speaker";
          chip.textContent = speaker;
          row.appendChild(chip);
        }
        const text = document.createElement("span");
        text.className = "meetings-transcript-text";
        text.textContent = item.text;
        row.appendChild(text);
        tBlock.appendChild(row);
      }
    }
    root.appendChild(tBlock);

    return root;
  }

  /** Build one persisted-item row. Mirrors the live overlay's
   * items-mirror layout (timestamp pill + bullet + body + per-mode
   * meta chip) so the meeting-detail view feels familiar. Chat
   * items render as bubble pairs instead — matches the live chat
   * surface. */
  function buildItemRow(mode: string, item: Item): HTMLElement {
    const meta = item.meta as Record<string, unknown> | null | undefined;
    if (mode === "chat") {
      const role = (meta?.role as string) ?? "assistant";
      const wrap = document.createElement("div");
      wrap.className = `meetings-chat-bubble meetings-chat-bubble-${role}`;
      const body = document.createElement("div");
      body.className = "meetings-chat-bubble-body";
      body.textContent = item.text;
      wrap.appendChild(body);
      return wrap;
    }
    const row = document.createElement("article");
    row.className = "meetings-item-row";
    const time = document.createElement("div");
    time.className = "meetings-item-time";
    time.textContent = `[${formatItemTime(item.t)}]`;
    row.appendChild(time);
    const body = document.createElement("div");
    body.className = "meetings-item-body";
    body.textContent = item.text;
    row.appendChild(body);
    const metaText = formatItemMeta(mode, meta);
    if (metaText) {
      const m = document.createElement("div");
      m.className = "meetings-item-meta";
      m.textContent = metaText;
      row.appendChild(m);
    }
    // Past-meeting detail view: show the persisted expansion when
    // present. Read-only — the meeting is over, no live expand
    // available, but the user can still read whatever the agent
    // produced during the meeting itself.
    if (item.detail && item.detail.length > 0) {
      const d = document.createElement("div");
      d.className = "meetings-item-detail";
      d.textContent = item.detail;
      row.appendChild(d);
    }
    return row;
  }

  function formatItemTime(t: number): string {
    const total = Math.max(0, Math.floor(t / 1000));
    const m = String(Math.floor(total / 60)).padStart(2, "0");
    const s = String(total % 60).padStart(2, "0");
    return `${m}:${s}`;
  }

  function formatItemMeta(mode: string, meta: Record<string, unknown> | null | undefined): string {
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
    return "";
  }

  /** Build a single moment card. Lives inside the modal closure so
   * it can use `makeApi()` for auth'd screenshot fetches. */
  function buildMomentCard(moment: import("../meetings-api").Moment): HTMLElement {
    const card = document.createElement("div");
    card.className = "meetings-moment-card";

    if (moment.screenshot_url) {
      const img = document.createElement("img");
      img.className = "meetings-moment-thumb";
      img.alt = "moment screenshot";
      img.title = "Click to enlarge";
      // `<img src>` can't carry an Authorization header, so we
      // lazy-fetch the bytes with our token and swap in a blob URL.
      const api = makeApi();
      let blobURL: string | null = null;
      if (api) {
        api
          .fetchScreenshot(moment.screenshot_url)
          .then((url) => {
            blobURL = url;
            img.src = url;
          })
          .catch(() => {
            img.alt = "screenshot failed to load";
            img.classList.add("failed");
          });
      }
      // Click to expand. Reuses the already-fetched blob URL so
      // the lightbox image is instant — no second network round trip.
      img.addEventListener("click", () => {
        if (blobURL) openLightbox(blobURL);
      });
      card.appendChild(img);
    }

    const right = document.createElement("div");
    right.className = "meetings-moment-body";

    const meta = document.createElement("div");
    meta.className = "meetings-moment-meta";
    const tStamp = document.createElement("span");
    tStamp.className = "meetings-moment-t";
    tStamp.textContent = formatOffset(moment.t);
    meta.appendChild(tStamp);
    if (moment.kind && moment.kind !== "manual") {
      const kindBadge = document.createElement("span");
      kindBadge.className = "meetings-moment-kind";
      kindBadge.textContent = moment.kind.toUpperCase();
      meta.appendChild(kindBadge);
    }
    right.appendChild(meta);

    if (moment.note && moment.note.trim()) {
      const note = document.createElement("div");
      note.className = "meetings-moment-note";
      note.textContent = moment.note;
      right.appendChild(note);
    }

    const summary = document.createElement("div");
    summary.className = "meetings-moment-summary";
    if (moment.summary_status === "done" && moment.summary) {
      summary.textContent = moment.summary;
    } else if (moment.summary_status === "pending") {
      summary.textContent = "Generating summary…";
      summary.classList.add("pending");
    } else if (moment.summary_status === "failed") {
      summary.textContent = moment.summary || "Summary failed.";
      summary.classList.add("failed");
    } else {
      summary.textContent = moment.summary || "";
      summary.classList.add("pending");
    }
    right.appendChild(summary);

    card.appendChild(right);
    return card;
  }

  /** Open a fullscreen lightbox showing the screenshot at natural
   * size. Click outside the image or hit Esc to dismiss. The
   * `blobURL` is the same one already attached to the thumbnail —
   * no re-fetch.
   */
  function openLightbox(blobURL: string): void {
    const lightbox = document.createElement("div");
    lightbox.className = "meetings-lightbox";

    const img = document.createElement("img");
    img.src = blobURL;
    img.className = "meetings-lightbox-img";
    // Stop clicks on the image itself from bubbling to the
    // background's dismiss handler.
    img.addEventListener("click", (e) => e.stopPropagation());
    lightbox.appendChild(img);

    const close = document.createElement("button");
    close.className = "meetings-lightbox-close";
    close.setAttribute("aria-label", "Close");
    close.textContent = "✕";
    close.addEventListener("click", (e) => {
      e.stopPropagation();
      dismiss();
    });
    lightbox.appendChild(close);

    function dismiss() {
      document.removeEventListener("keydown", onKey);
      lightbox.remove();
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") dismiss();
    }
    document.addEventListener("keydown", onKey);
    lightbox.addEventListener("click", dismiss);

    document.body.appendChild(lightbox);
  }

  /** Render a `mm:ss` (or `h:mm:ss` for long meetings) offset
   * from a moment's `t` (ms-since-meeting-start). */
  function formatOffset(ms: number): string {
    const total = Math.max(0, Math.floor(ms / 1000));
    const s = total % 60;
    const m = Math.floor(total / 60) % 60;
    const h = Math.floor(total / 3600);
    const pad = (n: number) => String(n).padStart(2, "0");
    if (h > 0) return `${h}:${pad(m)}:${pad(s)}`;
    return `${m}:${pad(s)}`;
  }

  // Open/close synchronization. On open: reload. On close: drop
  // selection so re-opening is fresh.
  let wasOpen = false;
  function syncOpen(): void {
    const isOpen = store.get().meetingsModalOpen;
    overlay.classList.toggle("open", isOpen);
    if (isOpen && !wasOpen) {
      void reloadList();
    } else if (!isOpen && wasOpen) {
      selectedId = null;
      renderDetail(null);
    }
    syncPane();
    wasOpen = isOpen;
  }
  syncOpen();
  store.subscribe((s) => s.meetingsModalOpen, syncOpen);
}

/// Coarse bucket for grouping meeting rows by recency. Returns one
/// of the fixed bucket names so heading order is stable.
function relativeBucket(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "Older";
  const now = new Date();
  const startOfDay = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const today = startOfDay(now);
  const dayMs = 24 * 60 * 60 * 1000;
  const target = startOfDay(d);
  if (target === today) return "Today";
  if (target === today - dayMs) return "Yesterday";
  if (target >= today - 6 * dayMs) return "This week";
  return "Older";
}

function errorMessage(e: unknown): string {
  if (e instanceof MeetingsApiError) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

function formatDateShort(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function formatDateLong(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  // Drop the year + the localized "at" connector: on narrow viewports
  // the long form ("May 6, 2026 at 8:51 AM") wraps awkwardly. Year is
  // implicit (current year for any meeting you'd practically open).
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function formatDuration(startedAt: string, endedAt: string | null): string {
  if (!endedAt) return "in progress";
  const start = new Date(startedAt).getTime();
  const end = new Date(endedAt).getTime();
  if (Number.isNaN(start) || Number.isNaN(end)) return "";
  const seconds = Math.max(0, Math.round((end - start) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const mins = Math.floor(seconds / 60);
  const rem = seconds % 60;
  if (mins < 60) return `${mins}m ${rem}s`;
  const hours = Math.floor(mins / 60);
  return `${hours}h ${mins % 60}m`;
}

function timingCell(label: string, value: string, cls?: "in-progress"): HTMLElement {
  const cell = document.createElement("div");
  cell.className = "meetings-timing-cell";
  const labelEl = document.createElement("div");
  labelEl.className = "label-mono meetings-timing-label";
  labelEl.textContent = label;
  const valueEl = document.createElement("div");
  valueEl.className = "meetings-timing-value";
  if (cls === "in-progress") valueEl.classList.add("in-progress");
  valueEl.textContent = value;
  cell.append(labelEl, valueEl);
  return cell;
}
