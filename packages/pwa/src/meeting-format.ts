//! Pure, surface-agnostic formatting for meeting summaries/titles.
//! Lives at the src root (not under `ui/` or `glasses/`) because both
//! the web meetings modal and the glasses history surface consume it.

import type { Item } from "./contract";

/// Title shown for a meeting. Order of preference:
///   1. `metadata.title` — extracted by the LLM, short and clean.
///   2. First non-empty line of the description, truncated to 80 chars.
///   3. "Untitled meeting" fallback.
export function pickDetailTitle(detail: {
  description?: string | null;
  metadata: Record<string, string>;
}): string {
  const meta = detail.metadata.title?.trim();
  if (meta) return meta;
  const desc = detail.description?.trim();
  if (desc) {
    const firstLine =
      desc
        .split("\n")
        .find((l) => l.trim().length > 0)
        ?.trim() ?? "";
    if (firstLine.length <= 80) return firstLine;
    return firstLine.slice(0, 79) + "…";
  }
  return "Untitled meeting";
}

/// Body text for the glasses summary popup. Prefers the extractor's
/// `summary`-mode items; falls back to the freeform description; finally
/// a placeholder. Returned in full — the glasses layer paginates it
/// across screen-sized pages, so there's no length cap here.
///
/// The final-summary worker stores ONE item tagged
/// `meta.kind === "narrative"` — flowing prose, rendered verbatim with
/// no bullet. The live running summary stores plain bullet items, each
/// prefixed with `• `. Mixed input is handled per-item, though in
/// practice a meeting's summary is all-narrative (post-finalize) or
/// all-bullets (live).
export function formatHistorySummaryBody(detail: {
  items_by_mode?: Record<string, Item[]>;
  description?: string | null;
}): string {
  const summaryItems = detail.items_by_mode?.summary ?? [];
  const parts = summaryItems
    .filter((i) => i.text.trim().length > 0)
    .map((i) => (i.meta?.kind === "narrative" ? i.text.trim() : `• ${i.text.trim()}`));
  if (parts.length > 0) {
    return parts.join("\n");
  }
  const desc = detail.description?.trim();
  return desc && desc.length > 0 ? desc : "(no summary yet)";
}
