import type { Config } from "./config.js";

/** Meeting list row — mirrors auris `MeetingSummary` JSON. */
export interface RawMeetingSummary {
  id: string;
  description: string | null;
  metadata: unknown;
  started_at: string;
  ended_at: string | null;
}

/** Transcript / mode item — mirrors auris `protocol::Item` JSON. No speaker field. */
export interface RawItem {
  id: string;
  text: string;
  t: number;
  detail?: string;
  meta?: unknown;
}

/** Moment — mirrors auris `MomentDto` JSON. */
export interface RawMoment {
  id: string;
  kind: string;
  t: number;
  note: string | null;
  summary: string | null;
  summary_status: string;
  screenshot_url: string | null;
}

/** Meeting detail — the subset of auris `MeetingDetail` this MCP consumes. */
export interface RawMeetingDetail {
  id: string;
  description: string | null;
  metadata: unknown;
  started_at: string;
  ended_at: string | null;
  transcript: RawItem[];
  moments: RawMoment[];
  items_by_mode: Record<string, RawItem[]>;
  wrap_up_status: string | null;
}

export class AuthError extends Error {
  constructor() {
    super("auris token expired or invalid — refresh AURIS_MCP_TOKEN.");
    this.name = "AuthError";
  }
}

export class NotFoundError extends Error {
  constructor() {
    super("meeting not found (or not owned by this token).");
    this.name = "NotFoundError";
  }
}

export class HttpError extends Error {
  constructor(
    public readonly status: number,
    detail: string,
  ) {
    super(`auris request failed (${status}): ${detail}`);
    this.name = "HttpError";
  }
}

export interface MeetingApi {
  listMeetings(): Promise<RawMeetingSummary[]>;
  getMeeting(id: string): Promise<RawMeetingDetail>;
}

export class AurisClient implements MeetingApi {
  constructor(private readonly config: Config) {}

  listMeetings(): Promise<RawMeetingSummary[]> {
    return this.request<RawMeetingSummary[]>("/meetings");
  }

  getMeeting(id: string): Promise<RawMeetingDetail> {
    return this.request<RawMeetingDetail>(`/meetings/${encodeURIComponent(id)}`);
  }

  private async request<T>(path: string): Promise<T> {
    let res: Response;
    try {
      res = await fetch(`${this.config.baseUrl}${path}`, {
        headers: {
          Authorization: `Bearer ${this.config.token}`,
          Accept: "application/json",
        },
      });
    } catch (e) {
      throw new HttpError(0, `network error contacting auris: ${(e as Error).message}`);
    }
    if (res.status === 401) throw new AuthError();
    if (res.status === 404) throw new NotFoundError();
    if (!res.ok) {
      const body = await res.text().catch(() => "");
      throw new HttpError(res.status, body.slice(0, 200));
    }
    return (await res.json()) as T;
  }
}
