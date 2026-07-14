import type { RawItem, RawMeetingDetail, RawMeetingSummary } from "./client.js";

export interface MeetingSummaryOut {
  id: string;
  title: string;
  project: string | null;
  started_at: string;
  ended_at: string | null;
  duration_min: number | null;
}

export interface MomentOut {
  kind: string;
  t: number;
  note: string | null;
  summary: string | null;
}

export interface MeetingBriefingOut {
  id: string;
  title: string;
  project: string | null;
  started_at: string;
  ended_at: string | null;
  duration_min: number | null;
  wrap_up_status: string | null;
  summary: string[];
  highlights: string[];
  actions: string[];
  open_questions: string[];
  moments: MomentOut[];
}

export interface TranscriptPage {
  total: number;
  offset: number;
  items: { id: string; t: number; text: string }[];
}

export interface SearchFilters {
  query?: string;
  project?: string;
  since?: string;
  until?: string;
}

/** Read a string field out of the free-form `metadata` JSON, else null. */
function metaField(metadata: unknown, key: string): string | null {
  if (metadata && typeof metadata === "object" && !Array.isArray(metadata)) {
    const v = (metadata as Record<string, unknown>)[key];
    if (typeof v === "string") return v;
  }
  return null;
}

function meetingTitle(raw: { metadata: unknown; description: string | null }): string {
  return metaField(raw.metadata, "title") ?? raw.description ?? "(untitled)";
}

function meetingProject(raw: { metadata: unknown }): string | null {
  return metaField(raw.metadata, "project");
}

function durationMin(startedAt: string, endedAt: string | null): number | null {
  if (!endedAt) return null;
  const ms = new Date(endedAt).getTime() - new Date(startedAt).getTime();
  return Math.round(ms / 60000);
}

export function toSummary(raw: RawMeetingSummary): MeetingSummaryOut {
  return {
    id: raw.id,
    title: meetingTitle(raw),
    project: meetingProject(raw),
    started_at: raw.started_at,
    ended_at: raw.ended_at,
    duration_min: durationMin(raw.started_at, raw.ended_at),
  };
}

export function matchesFilters(raw: RawMeetingSummary, f: SearchFilters): boolean {
  if (f.query) {
    const haystack = `${meetingTitle(raw)} ${raw.description ?? ""}`.toLowerCase();
    if (!haystack.includes(f.query.toLowerCase())) return false;
  }
  if (f.project && meetingProject(raw) !== f.project) return false;
  const day = raw.started_at.slice(0, 10); // YYYY-MM-DD, lexicographically comparable
  if (f.since && day < f.since) return false;
  if (f.until && day > f.until) return false;
  return true;
}

function modeTexts(d: RawMeetingDetail, mode: string): string[] {
  return (d.items_by_mode[mode] ?? []).map((i: RawItem) => i.text);
}

export function toBriefing(d: RawMeetingDetail): MeetingBriefingOut {
  return {
    id: d.id,
    title: meetingTitle(d),
    project: meetingProject(d),
    started_at: d.started_at,
    ended_at: d.ended_at,
    duration_min: durationMin(d.started_at, d.ended_at),
    wrap_up_status: d.wrap_up_status,
    summary: modeTexts(d, "summary"),
    highlights: modeTexts(d, "highlights"),
    actions: modeTexts(d, "actions"),
    open_questions: modeTexts(d, "open_questions"),
    moments: d.moments.map((m) => ({
      kind: m.kind,
      t: m.t,
      note: m.note,
      summary: m.summary,
    })),
  };
}

export function paginateTranscript(
  d: RawMeetingDetail,
  offset: number,
  limit: number,
): TranscriptPage {
  const items = d.transcript
    .slice(offset, offset + limit)
    .map((i) => ({ id: i.id, t: i.t, text: i.text }));
  return { total: d.transcript.length, offset, items };
}
