//! REST client for the server's `/meetings` endpoints. Mirror of
//! `packages/mac/Sources/Auris/Net/MeetingsAPI.swift`.
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
  /** Persisted items per non-transcript mode, populated by the
   * items-persistence task. Keyed by mode id (highlights / actions /
   * open_questions / summary / chat). Older server builds (before
   * 0003_items.sql) omit the field; treat absent as empty. */
  items_by_mode?: Record<string, Item[]>;
  /** LLM usage rollup persisted at meeting stop. All zero +
   * provider/model_id null on meetings ended before
   * 0004_meeting_llm_usage.sql, or that hit a failure path
   * bypassing the rollup. Multiply tokens by per-model rates
   * to compute $. Older server builds omit the field. */
  llm_usage?: MeetingLlmUsage;
}

export interface MeetingLlmUsage {
  calls: number;
  input_tokens: number;
  output_tokens: number;
  cached_input_tokens: number;
  /** "bedrock" / "openai" / "anthropic". Null for pre-migration meetings. */
  provider: string | null;
  /** e.g. "claude-opus-4-7". Null for pre-migration meetings. */
  model_id: string | null;
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
    /// Async-callable so each request fetches a fresh JWT — the
    /// Auth0 SDK rotates these silently. Caching at the SDK layer
    /// means most calls are cheap; we don't need to memoize here.
    private readonly tokenProvider: () => Promise<string>,
  ) {}

  /** Build from the build-time `SERVER_URL` constant + a token provider. */
  static from(serverUrl: string, tokenProvider: () => Promise<string>): MeetingsApi | null {
    const base = deriveApiBase(serverUrl);
    if (!base) return null;
    return new MeetingsApi(base, tokenProvider);
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
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(url, {
        headers: { Authorization: `Bearer ${token}` },
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

  /// Attach a past meeting to the active meeting. Server is
  /// idempotent (`ON CONFLICT DO NOTHING`); duplicate attaches
  /// are no-ops. Mirrors `ArtifactsApi.attach`.
  async attach(parentId: string, attachedId: string): Promise<void> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(
        this.baseUrl + "/meetings/" + encodeURIComponent(parentId) + "/attached_meetings",
        {
          method: "POST",
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ attached_meeting_id: attachedId }),
        },
      );
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new MeetingsApiError(`Attach failed (HTTP ${resp.status})`, resp.status);
    }
  }

  async detach(parentId: string, attachedId: string): Promise<void> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(
        this.baseUrl +
          "/meetings/" +
          encodeURIComponent(parentId) +
          "/attached_meetings/" +
          encodeURIComponent(attachedId),
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        },
      );
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new MeetingsApiError(`Detach failed (HTTP ${resp.status})`, resp.status);
    }
  }

  private async request<T>(path: string): Promise<T> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(this.baseUrl + path, {
        headers: { Authorization: `Bearer ${token}` },
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
