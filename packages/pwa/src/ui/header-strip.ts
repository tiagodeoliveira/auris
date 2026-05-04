//! Active-meeting header. Title + elapsed timer + subtitle.
//! See `docs/specs/pwa-ux-redesign.md` §3.4.

import type { Store } from "../store";

function fmtElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60)
    .toString()
    .padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}

export function mountHeaderStrip(parent: HTMLElement, store: Store): void {
  const strip = document.createElement("header");
  strip.className = "header-strip";
  parent.appendChild(strip);

  const titleEl = document.createElement("h1");
  titleEl.className = "header-title";
  strip.appendChild(titleEl);

  const subtitleEl = document.createElement("div");
  subtitleEl.className = "header-subtitle label-mono";
  strip.appendChild(subtitleEl);

  let timerInterval: ReturnType<typeof setInterval> | null = null;

  function updateSubtitle() {
    const s = store.get();
    const startedAt = s.meetingStartedAt ?? Date.now();
    const elapsed = fmtElapsed(Date.now() - startedAt);
    const project = s.metadata.project ? ` · ${s.metadata.project}` : "";
    subtitleEl.textContent = `${elapsed}${project}`;
  }

  function render() {
    const s = store.get();
    const isActive = s.meetingState === "active" || s.meetingState === "paused";
    if (!isActive) {
      strip.style.display = "none";
      if (timerInterval) {
        clearInterval(timerInterval);
        timerInterval = null;
      }
      return;
    }
    strip.style.display = "block";

    const title = s.metadata.title || "Meeting in progress";
    titleEl.textContent = title;

    updateSubtitle();
    if (!timerInterval) timerInterval = setInterval(updateSubtitle, 1000);
  }

  render();
  store.subscribe(
    (s) =>
      `${s.meetingState}|${s.metadata.title ?? ""}|${s.metadata.project ?? ""}|${s.meetingStartedAt}`,
    render,
  );
}
