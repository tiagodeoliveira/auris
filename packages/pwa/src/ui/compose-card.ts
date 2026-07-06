//! Shared shell for compose-screen sections. Produces a card with a
//! title + optional subtitle + coral underline + a content slot the
//! caller fills with the actual control. Used by every section on
//! the idle compose surface (description / tags / audio source /
//! assist sensitivity / attachments) so the screen reads as one
//! cohesive form instead of a stack of unrelated controls.
//!
//! Mirrors the mobile compose surface (see
//! `packages/mobile/app/(tabs)/index.tsx`) — same section names,
//! same subtitle convention, same coral underline accent.

const CORAL_UNDERLINE_WIDTH_PX = 24;

interface ComposeCardHandle {
  /// The outer `<section>` element. Callers can attach
  /// `style.display = "none"` to self-hide outside idle.
  card: HTMLElement;
  /// Where the caller appends the actual control content.
  content: HTMLElement;
}

export function mountComposeCard(
  parent: HTMLElement,
  title: string,
  subtitle?: string,
): ComposeCardHandle {
  const card = document.createElement("section");
  card.className = "compose-card";

  const head = document.createElement("header");
  head.className = "compose-card-head";

  const titleEl = document.createElement("h2");
  titleEl.className = "compose-card-title";
  titleEl.textContent = title;
  head.appendChild(titleEl);

  // Coral underline accent — small fixed-width swatch sitting under
  // the title. Matches mobile's `SectionRule` component.
  const rule = document.createElement("div");
  rule.className = "compose-card-rule";
  rule.style.width = `${CORAL_UNDERLINE_WIDTH_PX}px`;
  head.appendChild(rule);

  if (subtitle) {
    const subEl = document.createElement("p");
    subEl.className = "compose-card-subtitle";
    subEl.textContent = subtitle;
    head.appendChild(subEl);
  }

  card.appendChild(head);

  const content = document.createElement("div");
  content.className = "compose-card-content";
  card.appendChild(content);

  parent.appendChild(card);
  return { card, content };
}

/// Top-of-screen NEW MEETING heading + DESCRIBE·TAG·CAPTURE eyebrow.
/// Matches the mobile title block. Self-hides outside idle so the
/// compose surface only shows while staging a meeting.
export function mountComposeTitle(
  parent: HTMLElement,
  store: {
    get(): { meetingState: string };
    subscribe(sel: (s: { meetingState: string }) => unknown, cb: () => void): void;
  },
): void {
  const wrap = document.createElement("section");
  wrap.className = "compose-title-block";
  parent.appendChild(wrap);

  const h1 = document.createElement("h1");
  h1.className = "compose-title";
  h1.textContent = "NEW MEETING";
  wrap.appendChild(h1);

  const rule = document.createElement("div");
  rule.className = "compose-title-rule";
  wrap.appendChild(rule);

  const eyebrow = document.createElement("p");
  eyebrow.className = "compose-eyebrow";
  eyebrow.textContent = "DESCRIBE · TAG · CAPTURE";
  wrap.appendChild(eyebrow);

  function syncVisibility() {
    wrap.style.display = store.get().meetingState === "idle" ? "flex" : "none";
  }
  syncVisibility();
  store.subscribe((s) => s.meetingState, syncVisibility);
}
