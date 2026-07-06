// Pure helpers shared by the meeting history list + detail screens.
// Mirror of the PWA's `pickDetailTitle` + `relativeBucket` so visual
// behavior stays consistent across clients.

/// Title shown for a meeting. Order of preference:
///   1. metadata.title (LLM-extracted, short, clean)
///   2. first non-empty line of description, clipped to 80 chars
///   3. "Untitled meeting"
export function pickMeetingTitle(detail: {
  description?: string | null;
  metadata?: Record<string, string>;
}): string {
  const title = detail.metadata?.title?.trim();
  if (title) return title;
  const desc = detail.description?.trim();
  if (desc) {
    const firstLine =
      desc
        .split("\n")
        .find((l) => l.trim().length > 0)
        ?.trim() ?? "";
    if (firstLine.length === 0) return "Untitled meeting";
    if (firstLine.length <= 80) return firstLine;
    return firstLine.slice(0, 79) + "…";
  }
  return "Untitled meeting";
}

/// Bucket label for a list grouped by recency. Same buckets the PWA
/// uses; lets the UI render section headers without re-bucketing on
/// every render.
export function relativeBucket(iso: string): string {
  const date = new Date(iso);
  const now = new Date();
  const dayMs = 24 * 60 * 60 * 1000;
  const startOfDay = (d: Date) => {
    const out = new Date(d);
    out.setHours(0, 0, 0, 0);
    return out;
  };
  const today = startOfDay(now);
  const that = startOfDay(date);
  const diff = (today.getTime() - that.getTime()) / dayMs;
  if (diff < 1) return "Today";
  if (diff < 2) return "Yesterday";
  if (diff < 7) return "This week";
  return "Older";
}

/// Short date+time for the row sub-line (e.g. "May 8, 1:06 PM").
export function formatDateShort(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

/// Long date+time for the detail header.
export function formatDateLong(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    weekday: "short",
    month: "long",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

/// Human duration. "in progress" while ended_at is null so the
/// list naturally distinguishes the active meeting from past ones.
export function formatDuration(startedAt: string, endedAt: string | null): string {
  if (!endedAt) return "in progress";
  const seconds = Math.floor((new Date(endedAt).getTime() - new Date(startedAt).getTime()) / 1000);
  if (seconds < 60) return `${seconds}s`;
  const mins = Math.floor(seconds / 60);
  const rem = seconds % 60;
  if (mins < 60) return `${mins}m ${rem}s`;
  const hours = Math.floor(mins / 60);
  return `${hours}h ${mins % 60}m`;
}

/// Format a number with thousands separators (e.g. 49,249).
export function formatTokens(n: number): string {
  return n.toLocaleString();
}
