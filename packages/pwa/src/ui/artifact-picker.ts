//! Reusable artifact picker modal. Promise-based: caller awaits
//! `pickArtifacts(...)` and gets back the user's selection (or
//! `null` if they cancelled). Fresh DOM per call — modal is
//! ephemeral.
//!
//! Compose flow stages picks into `pendingArtifactAttachments`; the
//! mid-meeting flow fires attach POSTs directly. Both reuse this
//! same picker — only the `onConfirm` semantics differ.

import type { AuthBundle } from "../auth";
import { ArtifactsApi, type Artifact } from "../artifacts-api";
import { MeetingsApiError } from "../meetings-api";
import { SERVER_URL } from "../server-url";

export interface PickArtifactsOptions {
  /// IDs to pre-check on open. Existing compose selection or the
  /// meeting's currently-attached set, depending on caller.
  alreadySelectedIds: readonly string[];
  /// Title shown in the modal header.
  title?: string;
  /// Confirm button label (defaults to "Attach").
  confirmLabel?: string;
  auth: AuthBundle;
}

/// Returns the chosen artifacts on confirm, `null` on cancel /
/// backdrop close. Selection is filtered to `summary_status: done`
/// rows — pending/failed are unselectable in the UI but we
/// double-check here in case of races.
export function pickArtifacts(opts: PickArtifactsOptions): Promise<Artifact[] | null> {
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
    title.textContent = opts.title ?? "Attach artifacts";
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

    let library: Artifact[] = [];
    const selected = new Set<string>(opts.alreadySelectedIds);

    function close(result: Artifact[] | null): void {
      overlay.remove();
      resolve(result);
    }

    function refreshCounter(): void {
      counter.textContent = `${selected.size} selected`;
    }

    function render(): void {
      body.innerHTML = "";
      if (library.length === 0) {
        const empty = document.createElement("div");
        empty.className = "artifacts-empty";
        empty.textContent = "No artifacts yet — upload from the 📄 menu first.";
        body.appendChild(empty);
        return;
      }
      const list = document.createElement("div");
      list.className = "artifact-picker-list";
      for (const a of library) {
        const isDone = a.summary_status === "done";
        const isChecked = selected.has(a.id);
        const row = document.createElement("button");
        row.type = "button";
        row.className = `artifact-picker-row${isChecked ? " checked" : ""}${isDone ? "" : " disabled"}`;
        row.disabled = !isDone && !isChecked;

        const box = document.createElement("span");
        box.className = "artifact-picker-checkbox";
        box.textContent = isChecked ? "☑" : "☐";
        row.appendChild(box);

        const info = document.createElement("span");
        info.className = "artifact-picker-info";
        const name = document.createElement("span");
        name.className = "artifact-name";
        name.textContent = a.name;
        info.appendChild(name);
        if (a.short_summary) {
          const short = document.createElement("span");
          short.className = "artifact-short";
          short.textContent = a.short_summary;
          info.appendChild(short);
        }
        if (a.summary_status === "pending") {
          const note = document.createElement("span");
          note.className = "artifact-note";
          note.textContent = "Generating summary…";
          info.appendChild(note);
        } else if (a.summary_status === "failed") {
          const note = document.createElement("span");
          note.className = "artifact-note artifact-note-failed";
          note.textContent = "Summary failed";
          info.appendChild(note);
        }
        row.appendChild(info);

        row.addEventListener("click", () => {
          if (selected.has(a.id)) {
            selected.delete(a.id);
          } else if (a.summary_status === "done") {
            selected.add(a.id);
          }
          render();
          refreshCounter();
        });
        list.appendChild(row);
      }
      body.appendChild(list);
    }

    cancelBtn.addEventListener("click", () => close(null));
    confirmBtn.addEventListener("click", () => {
      const picked = library.filter((a) => selected.has(a.id));
      close(picked);
    });
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) close(null);
    });

    refreshCounter();
    body.textContent = "Loading…";

    void (async () => {
      const api = ArtifactsApi.from(SERVER_URL, () => opts.auth.getAccessToken());
      if (!api) {
        body.innerHTML = "";
        const err = document.createElement("div");
        err.className = "artifacts-error";
        err.textContent = "Server URL or token missing — open Settings.";
        body.appendChild(err);
        return;
      }
      try {
        library = await api.list();
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
