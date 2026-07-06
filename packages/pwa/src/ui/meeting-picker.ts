//! Reusable past-meeting picker modal. Promise-based mirror of
//! `artifact-picker.ts` — caller awaits `pickMeetings(...)` and gets
//! back the user's selection (or `null` if they cancelled). Past
//! meetings provide carry-over context: the selected meetings become
//! attached to the active meeting, and the agent gets new
//! `fetch_meeting_summary` / `fetch_meeting` tools to consult them.
//!
//! Compose flow stages picks into `pendingAttachedMeetings`; the
//! mid-meeting flow fires attach POSTs directly. Both reuse this
//! same picker — only the `onConfirm` semantics differ.

import type { AuthBundle } from "../auth";
import { MeetingsApi, MeetingsApiError, type MeetingSummary } from "../meetings-api";
import { SERVER_URL } from "../server-url";

export interface PickMeetingsOptions {
  /// IDs to pre-check on open. Existing compose selection or the
  /// meeting's currently-attached set, depending on caller.
  alreadySelectedIds: readonly string[];
  /// Optional id of the active meeting. When provided, the picker
  /// hides it from the list (a meeting can't attach to itself; the
  /// server enforces this with a CHECK constraint).
  excludeMeetingId?: string | null;
  /// Title shown in the modal header.
  title?: string;
  /// Confirm button label (defaults to "Attach").
  confirmLabel?: string;
  auth: AuthBundle;
}

/// Returns the chosen meetings on confirm, `null` on cancel /
/// backdrop close. The list is sorted newest-first (server already
/// orders by `started_at DESC`).
export function pickMeetings(opts: PickMeetingsOptions): Promise<MeetingSummary[] | null> {
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "settings-overlay artifact-picker-overlay open";
    document.body.appendChild(overlay);

    const modal = document.createElement("div");
    modal.className = "settings-modal artifact-picker-modal";
    overlay.appendChild(modal);

    const header = document.createElement("div");
    header.className = "artifact-picker-header";
    const title = document.createElement("h2");
    title.className = "settings-title";
    title.textContent = opts.title ?? "Attach past meetings";
    const counter = document.createElement("span");
    counter.className = "artifact-picker-counter label-mono";
    header.append(title, counter);
    modal.appendChild(header);

    const body = document.createElement("div");
    body.className = "artifact-picker-body";
    modal.appendChild(body);

    const footer = document.createElement("div");
    footer.className = "artifact-picker-footer";
    const cancelBtn = document.createElement("button");
    cancelBtn.className = "btn-ghost";
    cancelBtn.textContent = "Cancel";
    const confirmBtn = document.createElement("button");
    confirmBtn.className = "btn-primary";
    confirmBtn.textContent = opts.confirmLabel ?? "Attach";
    footer.append(cancelBtn, confirmBtn);
    modal.appendChild(footer);

    let library: MeetingSummary[] = [];
    const selected = new Set<string>(opts.alreadySelectedIds);

    function close(result: MeetingSummary[] | null): void {
      overlay.remove();
      resolve(result);
    }

    function refreshCounter(): void {
      counter.textContent = `${selected.size} selected`;
    }

    function formatWhen(iso: string): string {
      // Parsed as local time. The server emits RFC 3339 with `Z`,
      // so `Date` handles it; `toLocaleString` is the cheapest
      // human-readable rendering and matches the meetings-modal
      // convention.
      const d = new Date(iso);
      if (Number.isNaN(d.getTime())) return iso;
      return d.toLocaleString();
    }

    function pickLabel(m: MeetingSummary): string {
      const desc = (m.description ?? "").trim();
      if (desc) return desc;
      // Fallback mirrors the server's `pick_meeting_title` helper:
      // try metadata.title, else "Meeting".
      const meta = m.metadata?.title;
      if (meta && meta.trim()) return meta.trim();
      return "Meeting";
    }

    function render(): void {
      body.innerHTML = "";
      if (library.length === 0) {
        const empty = document.createElement("div");
        empty.className = "artifacts-empty";
        empty.textContent = "No past meetings yet.";
        body.appendChild(empty);
        return;
      }
      const list = document.createElement("div");
      list.className = "artifact-picker-list";
      for (const m of library) {
        const isChecked = selected.has(m.id);
        const row = document.createElement("button");
        row.type = "button";
        row.className = `artifact-picker-row${isChecked ? " checked" : ""}`;

        const box = document.createElement("span");
        box.className = "artifact-picker-checkbox";
        box.textContent = isChecked ? "☑" : "☐";
        row.appendChild(box);

        const info = document.createElement("span");
        info.className = "artifact-picker-info";
        const name = document.createElement("span");
        name.className = "artifact-name";
        name.textContent = pickLabel(m);
        info.appendChild(name);
        const when = document.createElement("span");
        when.className = "artifact-short";
        when.textContent = formatWhen(m.started_at);
        info.appendChild(when);
        row.appendChild(info);

        row.addEventListener("click", () => {
          if (selected.has(m.id)) selected.delete(m.id);
          else selected.add(m.id);
          render();
          refreshCounter();
        });
        list.appendChild(row);
      }
      body.appendChild(list);
    }

    cancelBtn.addEventListener("click", () => close(null));
    confirmBtn.addEventListener("click", () => {
      const picked = library.filter((m) => selected.has(m.id));
      close(picked);
    });
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) close(null);
    });

    refreshCounter();
    body.textContent = "Loading…";

    void (async () => {
      const api = MeetingsApi.from(SERVER_URL, () => opts.auth.getAccessToken());
      if (!api) {
        body.innerHTML = "";
        const err = document.createElement("div");
        err.className = "artifacts-error";
        err.textContent = "Server URL or token missing — open Settings.";
        body.appendChild(err);
        return;
      }
      try {
        const all = await api.list();
        library = opts.excludeMeetingId ? all.filter((m) => m.id !== opts.excludeMeetingId) : all;
        render();
      } catch (e) {
        body.innerHTML = "";
        const err = document.createElement("div");
        err.className = "artifacts-error";
        err.textContent = e instanceof MeetingsApiError ? e.message : String(e);
        body.appendChild(err);
      }
    })();
  });
}
