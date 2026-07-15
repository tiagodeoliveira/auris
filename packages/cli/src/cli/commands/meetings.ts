import { AurisClient, type MeetingApi } from "../../core/client.js";
import { getAccessToken } from "../../core/auth.js";
import { resolveConfig } from "../../core/config.js";
import {
  matchesFilters,
  paginateTranscript,
  toBriefing,
  toSummary,
  type MeetingSummaryOut,
  type SearchFilters,
} from "../../core/shape.js";

/** Build the real client from config: env token wins, else the logged-in token. */
export function makeClient(): AurisClient {
  const cfg = resolveConfig();
  return new AurisClient(
    cfg.baseUrl,
    async () => cfg.envToken ?? (await getAccessToken(cfg.auth0)),
  );
}

function fmtSummaries(rows: MeetingSummaryOut[]): string {
  if (rows.length === 0) return "(no meetings)";
  return rows
    .map((m) => {
      const dur = m.duration_min == null ? "ongoing" : `${m.duration_min}m`;
      const proj = m.project ? ` [${m.project}]` : "";
      return `${m.id}  ${m.started_at.slice(0, 10)}  ${dur}${proj}  ${m.title.replace(/\n/g, " ")}`;
    })
    .join("\n");
}

export async function listCmd(
  api: MeetingApi,
  o: { limit?: number; json?: boolean },
): Promise<string> {
  const rows = (await api.listMeetings()).slice(0, o.limit ?? 20).map(toSummary);
  return o.json ? JSON.stringify(rows, null, 2) : fmtSummaries(rows);
}

export async function searchCmd(
  api: MeetingApi,
  o: {
    query?: string;
    project?: string;
    since?: string;
    until?: string;
    limit?: number;
    json?: boolean;
  },
): Promise<string> {
  const f: SearchFilters = { query: o.query, project: o.project, since: o.since, until: o.until };
  const rows = (await api.listMeetings())
    .filter((m) => matchesFilters(m, f))
    .slice(0, o.limit ?? 20)
    .map(toSummary);
  return o.json ? JSON.stringify(rows, null, 2) : fmtSummaries(rows);
}

export async function getCmd(api: MeetingApi, id: string, o: { json?: boolean }): Promise<string> {
  const b = toBriefing(await api.getMeeting(id));
  return JSON.stringify(b, null, 2); // briefing is structured; JSON is the useful form
}

export async function transcriptCmd(
  api: MeetingApi,
  id: string,
  o: { offset?: number; limit?: number; json?: boolean },
): Promise<string> {
  const page = paginateTranscript(await api.getMeeting(id), o.offset ?? 0, o.limit ?? 200);
  return o.json
    ? JSON.stringify(page, null, 2)
    : page.items.map((i) => (i.speaker ? `[${i.speaker}] ${i.text}` : i.text)).join("\n");
}
