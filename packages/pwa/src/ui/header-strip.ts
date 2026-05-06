//! Active-meeting header. Two compact rows:
//!  Row 1: timer · project chip (if any)  — the always-glanced metadata
//!         alongside the memory pill (icon + count, hover for breakdown).
//!  Row 2: meeting title, truncated with ellipsis on overflow.
//!
//! Replaces the prior layout that stacked title (large), subtitle, and a
//! verbose "★ MEMORY · 30 RECALLED" pill, which combined ate ~120px of
//! vertical space on mobile.

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

  // Row 1: timer + project + memory pill
  const metaRow = document.createElement("div");
  metaRow.className = "header-meta-row";
  strip.appendChild(metaRow);

  const timerEl = document.createElement("span");
  timerEl.className = "header-timer";
  metaRow.appendChild(timerEl);

  const projectEl = document.createElement("span");
  projectEl.className = "header-project";
  metaRow.appendChild(projectEl);

  const memoryBadge = document.createElement("span");
  memoryBadge.className = "header-memory-badge";
  metaRow.appendChild(memoryBadge);

  // Row 2: title (truncated)
  const titleEl = document.createElement("h1");
  titleEl.className = "header-title";
  strip.appendChild(titleEl);

  let timerInterval: ReturnType<typeof setInterval> | null = null;

  function updateTimer() {
    const s = store.get();
    const startedAt = s.meetingStartedAt ?? Date.now();
    timerEl.textContent = fmtElapsed(Date.now() - startedAt);
  }

  function updateProject() {
    const project = store.get().metadata.project;
    if (project && project.trim()) {
      projectEl.textContent = project;
      projectEl.style.display = "inline-flex";
    } else {
      projectEl.style.display = "none";
    }
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
    memoryBadge.innerHTML = `<span class="header-memory-icon">★</span><span class="header-memory-count">${total}</span>`;
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
    strip.style.display = "flex";

    titleEl.textContent = s.metadata.title || "Meeting in progress";
    updateTimer();
    updateProject();
    updateMemoryBadge();
    if (!timerInterval) timerInterval = setInterval(updateTimer, 1000);
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
