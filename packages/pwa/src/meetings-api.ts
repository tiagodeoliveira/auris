//! REST client for the server's `/meetings` endpoints. Mirror of
//! `packages/mac/Sources/MeetingCompanion/Net/MeetingsAPI.swift`.
//!
//! Convention: API URL = WS URL with scheme upgraded (`ws→http`,
//! `wss→https`) and port bumped by +1, matching the server's
//! `--api-port` default. Both clients agree on this so the user
//! only configures the WS URL in settings.

import type { Item } from "./contract";

export interface MeetingSummary {
  id: string;
  description: string | null;
  metadata: Record<string, string>;
  /** RFC 3339 / ISO 8601 string (with or without fractional seconds). */
  started_at: string;
  ended_at: string | null;
}

export interface MeetingDetail extends MeetingSummary {
  /** Inlined transcript items, one per finalized utterance. Empty
   * array when the meeting has no committed transcript yet. */
  transcript: Item[];
  /** Moments captured during this meeting, oldest first. Older
   * server builds (before the moments-API commit) omit the field;
   * the optional shape keeps clients forward-compat. */
  moments?: Moment[];
}

export interface Moment {
  id: string;
  kind: string;
  /** ms-since-meeting-start. */
  t: number;
  note: string | null;
  summary: string | null;
  summary_status: "pending" | "done" | "failed" | string;
  /** Server-rooted relative path, e.g. `/meetings/abc/moments/def/screenshot`,
   * or `null` when no screenshot was captured. Resolve against the
   * REST base when fetching with the bearer header. */
  screenshot_url: string | null;
  created_at: string;
}

export class MeetingsApiError extends Error {
  readonly status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
    this.name = "MeetingsApiError";
  }
}

/**
 * Build the base URL of the REST API from the WS URL the user typed
 * into Settings. WS and REST share a single port now (axum routes
 * both); we just upgrade the scheme and strip the path/query.
 */
export function deriveApiBase(wsUrl: string): string | null {
  try {
    const url = new URL(wsUrl);
    if (url.protocol === "ws:") url.protocol = "http:";
    else if (url.protocol === "wss:") url.protocol = "https:";
    else return null;
    url.pathname = "";
    url.search = "";
    return url.origin;
  } catch {
    return null;
  }
}

export class MeetingsApi {
  constructor(
    private readonly baseUrl: string,
    private readonly token: string,
  ) {}

  /** Build from `store.settings.serverUrl` + `serverToken`. */
  static from(serverUrl: string, token: string): MeetingsApi | null {
    const base = deriveApiBase(serverUrl);
    if (!base || !token) return null;
    return new MeetingsApi(base, token);
  }

  list(): Promise<MeetingSummary[]> {
    return this.request<MeetingSummary[]>("/meetings");
  }

  detail(id: string): Promise<MeetingDetail> {
    return this.request<MeetingDetail>(`/meetings/${encodeURIComponent(id)}`);
  }

  /**
   * Fetch a screenshot's bytes with the bearer header and return a
   * blob URL the caller can put in `<img src>`. Caller is responsible
   * for `URL.revokeObjectURL` on the returned URL when it's no longer
   * displayed (we typically revoke when the modal closes).
   */
  async fetchScreenshot(relativeOrFullUrl: string): Promise<string> {
    const path = relativeOrFullUrl.startsWith("/") ? relativeOrFullUrl.slice(1) : relativeOrFullUrl;
    const url = this.baseUrl + "/" + path;
    let resp: Response;
    try {
      resp = await fetch(url, {
        headers: { Authorization: `Bearer ${this.token}` },
        cache: "no-store",
      });
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new MeetingsApiError(`screenshot fetch returned ${resp.status}`, resp.status);
    }
    const blob = await resp.blob();
    return URL.createObjectURL(blob);
  }

  private async request<T>(path: string): Promise<T> {
    let resp: Response;
    try {
      resp = await fetch(this.baseUrl + path, {
        headers: { Authorization: `Bearer ${this.token}` },
        cache: "no-store",
      });
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      let message: string;
      switch (resp.status) {
        case 401:
          message = "Server rejected the token (401). Check Settings.";
          break;
        case 404:
          message = "Meeting not found (404).";
          break;
        default:
          message = `Server returned HTTP ${resp.status}.`;
      }
      throw new MeetingsApiError(message, resp.status);
    }
    return (await resp.json()) as T;
  }
}
