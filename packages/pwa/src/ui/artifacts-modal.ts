//! Artifact library modal. Mirrors the Mac Settings → Artifacts tab.
//!
//! Triggered by the 📄 icon in `top-bar`; closes via the backdrop or
//! the Close button. Reuses the `.settings-overlay` /
//! `.settings-modal` chrome the user already knows from the Meetings
//! and Settings modals.
//!
//! Polls every 2 s while any row is `pending` to surface the
//! summary worker's transition; stops once everything settles.

import type { Store } from "../store";
import type { AuthBundle } from "../auth";
import { ArtifactsApi, type Artifact } from "../artifacts-api";
import { MeetingsApiError } from "../meetings-api";
import { SERVER_URL } from "../server-url";

export function mountArtifactsModal(parent: HTMLElement, store: Store, auth: AuthBundle): void {
  const overlay = document.createElement("div");
  overlay.className = "settings-overlay artifacts-overlay";
  parent.appendChild(overlay);

  const modal = document.createElement("div");
  modal.className = "settings-modal artifacts-modal";
  overlay.appendChild(modal);

  const header = document.createElement("div");
  header.className = "artifacts-header";
  const title = document.createElement("h2");
  title.className = "settings-title";
  title.textContent = "Artifacts";
  const uploadBtn = document.createElement("button");
  uploadBtn.className = "btn-primary artifacts-upload-btn";
  uploadBtn.textContent = "Upload…";
  const fileInput = document.createElement("input");
  fileInput.type = "file";
  // No accept filter — server's mime whitelist is the source of
  // truth and surfaces a clear 400 if the user picks something
  // unsupported. Keeping the input wide-open avoids drift between
  // client and server allowed lists.
  fileInput.style.display = "none";
  fileInput.addEventListener("change", () => {
    const f = fileInput.files?.[0];
    if (f) void doUpload(f);
    fileInput.value = ""; // allow picking the same file again
  });
  uploadBtn.addEventListener("click", () => fileInput.click());
  const reloadBtn = document.createElement("button");
  reloadBtn.className = "btn-ghost";
  reloadBtn.textContent = "Reload";
  reloadBtn.addEventListener("click", () => void reload());
  const closeBtn = document.createElement("button");
  closeBtn.className = "btn-ghost";
  closeBtn.textContent = "Close";
  closeBtn.addEventListener("click", close);
  header.append(title, uploadBtn, fileInput, reloadBtn, closeBtn);
  modal.appendChild(header);

  const body = document.createElement("div");
  body.className = "artifacts-body";
  modal.appendChild(body);

  // Backdrop click closes (matches meetings-modal behavior).
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) close();
  });

  let library: Artifact[] = [];
  let listError: string | null = null;
  let listLoading = false;
  let uploading = false;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  // Per-row expand state for the long-summary disclosure. Survives
  // re-renders across the same modal session.
  const expanded = new Set<string>();

  function close(): void {
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
    store.update({ artifactsModalOpen: false });
  }

  function makeApi(): ArtifactsApi | null {
    return ArtifactsApi.from(SERVER_URL, () => auth.getAccessToken());
  }

  async function reload(): Promise<void> {
    const api = makeApi();
    if (!api) {
      listError = "Server URL or token missing — open Settings.";
      library = [];
      render();
      return;
    }
    listLoading = true;
    render();
    try {
      library = await api.list();
      listError = null;
    } catch (e) {
      listError = errorMessage(e);
      library = [];
    } finally {
      listLoading = false;
      render();
      schedulePollIfPending();
    }
  }

  function schedulePollIfPending(): void {
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
    const anyPending = library.some((a) => a.summary_status === "pending");
    if (!anyPending) return;
    pollTimer = setTimeout(() => {
      pollTimer = null;
      void reload();
    }, 2000);
  }

  async function doUpload(file: File): Promise<void> {
    const api = makeApi();
    if (!api) {
      listError = "Server URL or token missing — open Settings.";
      render();
      return;
    }
    uploading = true;
    render();
    try {
      await api.upload(file);
      listError = null;
      await reload(); // pulls the new row into view + restarts poll
    } catch (e) {
      listError = errorMessage(e);
    } finally {
      uploading = false;
      render();
    }
  }

  async function doDelete(id: string): Promise<void> {
    if (!confirm("Delete this artifact? This cannot be undone.")) return;
    const api = makeApi();
    if (!api) return;
    // Optimistic remove; revert on failure.
    const before = library;
    library = library.filter((a) => a.id !== id);
    render();
    try {
      await api.delete(id);
    } catch (e) {
      library = before;
      listError = errorMessage(e);
      render();
    }
  }

  function render(): void {
    body.innerHTML = "";
    if (uploading) {
      const banner = document.createElement("div");
      banner.className = "artifacts-uploading";
      banner.textContent = "Uploading…";
      body.appendChild(banner);
    }
    if (listError) {
      const err = document.createElement("div");
      err.className = "artifacts-error";
      err.textContent = listError;
      body.appendChild(err);
    }
    if (listLoading && library.length === 0) {
      const loading = document.createElement("div");
      loading.className = "artifacts-empty";
      loading.textContent = "Loading…";
      body.appendChild(loading);
      return;
    }
    if (library.length === 0 && !listError) {
      const empty = document.createElement("div");
      empty.className = "artifacts-empty";
      empty.innerHTML =
        "<strong>No artifacts yet</strong><br>" +
        "<span class='label-mono'>Upload a document or image to give meeting agents context.</span>";
      body.appendChild(empty);
      return;
    }
    const list = document.createElement("div");
    list.className = "artifacts-list";
    for (const a of library) list.appendChild(rowFor(a));
    body.appendChild(list);
  }

  function rowFor(a: Artifact): HTMLElement {
    const row = document.createElement("div");
    row.className = `artifact-row status-${a.summary_status}`;

    const head = document.createElement("div");
    head.className = "artifact-row-head";

    const badge = document.createElement("span");
    badge.className = "artifact-status-badge";
    badge.textContent =
      a.summary_status === "done"
        ? "✓"
        : a.summary_status === "pending"
          ? "⋯"
          : a.summary_status === "failed"
            ? "!"
            : "?";
    head.appendChild(badge);

    const name = document.createElement("span");
    name.className = "artifact-name";
    name.textContent = a.name;
    head.appendChild(name);

    const mime = document.createElement("span");
    mime.className = "artifact-mime label-mono";
    mime.textContent = a.mime_type;
    head.appendChild(mime);

    const size = document.createElement("span");
    size.className = "artifact-size label-mono";
    size.textContent = humanSize(a.size_bytes);
    head.appendChild(size);

    const canExpand = a.summary_status === "done" && (a.long_summary?.length ?? 0) > 0;
    if (canExpand) {
      const chev = document.createElement("button");
      chev.className = "btn-ghost artifact-chev";
      chev.setAttribute("aria-label", expanded.has(a.id) ? "Hide details" : "Show details");
      chev.textContent = expanded.has(a.id) ? "▾" : "▸";
      chev.addEventListener("click", () => {
        if (expanded.has(a.id)) expanded.delete(a.id);
        else expanded.add(a.id);
        render();
      });
      head.appendChild(chev);
    }

    const del = document.createElement("button");
    del.className = "btn-ghost artifact-del";
    del.setAttribute("aria-label", "Delete artifact");
    del.title = "Delete";
    del.textContent = "🗑";
    del.addEventListener("click", () => void doDelete(a.id));
    head.appendChild(del);

    row.appendChild(head);

    if (a.summary_status === "done" && a.short_summary) {
      const short = document.createElement("div");
      short.className = "artifact-short";
      short.textContent = a.short_summary;
      row.appendChild(short);
    }
    if (a.summary_status === "pending") {
      const note = document.createElement("div");
      note.className = "artifact-note";
      note.textContent = "Generating summary…";
      row.appendChild(note);
    }
    if (a.summary_status === "failed") {
      const note = document.createElement("div");
      note.className = "artifact-note artifact-note-failed";
      note.textContent = "Summary failed — server logs may have more.";
      row.appendChild(note);
    }
    if (canExpand && expanded.has(a.id) && a.long_summary) {
      const long = document.createElement("div");
      long.className = "artifact-long";
      long.textContent = a.long_summary;
      row.appendChild(long);
    }
    return row;
  }

  function errorMessage(e: unknown): string {
    if (e instanceof MeetingsApiError) return e.message;
    if (e instanceof Error) return e.message;
    return String(e);
  }

  function humanSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  // Visibility wiring. Reload on open; teardown poll on close.
  // Uses the `.open` class on `.settings-overlay` for parity with
  // the meetings/settings modals (CSS handles the actual display).
  function syncOpen(): void {
    const open = store.get().artifactsModalOpen;
    overlay.classList.toggle("open", open);
    if (open) {
      void reload();
    } else if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }
  syncOpen();
  store.subscribe((s) => s.artifactsModalOpen, syncOpen);
}
