//! Active-meeting header. Title + elapsed timer + subtitle + memory badge.

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

  // Memory badge — visible only when mnemo recall populated prior context.
  // Hidden otherwise (idle, no mnemo, recall failed, recall returned empty).
  const memoryBadge = document.createElement("div");
  memoryBadge.className = "header-memory-badge label-mono";
  strip.appendChild(memoryBadge);

  let timerInterval: ReturnType<typeof setInterval> | null = null;

  function updateSubtitle() {
    const s = store.get();
    const startedAt = s.meetingStartedAt ?? Date.now();
    const elapsed = fmtElapsed(Date.now() - startedAt);
    const project = s.metadata.project ? ` · ${s.metadata.project}` : "";
    subtitleEl.textContent = `${elapsed}${project}`;
  }

  function updateMemoryBadge() {
    const pc = store.get().priorContext;
    if (!pc) {
      memoryBadge.style.display = "none";
      return;
    }
    const total = pc.preferences + pc.facts + pc.episodes + pc.project_memories;
    if (total === 0) {
      memoryBadge.style.display = "none";
      return;
    }
    memoryBadge.style.display = "inline-flex";
    memoryBadge.textContent = `★ memory · ${total} recalled`;
    // Full breakdown on hover; also self-explanatory via spelled-out labels.
    const parts: string[] = [];
    if (pc.preferences > 0) parts.push(`${pc.preferences} preferences`);
    if (pc.facts > 0) parts.push(`${pc.facts} facts`);
    if (pc.episodes > 0) parts.push(`${pc.episodes} past discussions`);
    if (pc.project_memories > 0) parts.push(`${pc.project_memories} project memories`);
    memoryBadge.title = `Prior context loaded for the LLM extractors:\n${parts.join("\n")}`;
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
    updateMemoryBadge();
    if (!timerInterval) timerInterval = setInterval(updateSubtitle, 1000);
  }

  render();
  store.subscribe(
    (s) =>
      `${s.meetingState}|${s.metadata.title ?? ""}|${s.metadata.project ?? ""}|${s.meetingStartedAt}`,
    render,
  );
  store.subscribe(
    (s) =>
      s.priorContext === null
        ? "null"
        : `${s.priorContext.preferences}|${s.priorContext.facts}|${s.priorContext.episodes}|${s.priorContext.project_memories}`,
    updateMemoryBadge,
  );
}
