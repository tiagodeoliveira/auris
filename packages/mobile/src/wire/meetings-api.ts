// REST client for the server's `/meetings` endpoints. Hand-ported
// from packages/pwa/src/meetings-api.ts.
//
// Differences from the PWA port:
//   - `URL.createObjectURL(blob)` doesn't exist in React Native.
//     The screenshot-fetch helper is a stub for now; Phase 5
//     (meeting-detail moments rendering) will swap to writing the
//     blob to a temp file via `expo-file-system` and pointing
//     `expo-image` at the file:// URI. Throws meanwhile.
//   - Otherwise: same auth header, same error mapping, same shape.

import type { Item } from "./contract";

export interface MeetingSummary {
  id: string;
  description: string | null;
  metadata: Record<string, string>;
  /** RFC 3339 / ISO 8601 string. */
  started_at: string;
  ended_at: string | null;
}

export interface MeetingDetail extends MeetingSummary {
  transcript: Item[];
  moments?: Moment[];
  items_by_mode?: Record<string, Item[]>;
  llm_usage?: MeetingLlmUsage;
}

export interface MeetingLlmUsage {
  calls: number;
  input_tokens: number;
  output_tokens: number;
  cached_input_tokens: number;
  provider: string | null;
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
  /** Server-rooted relative path, or `null`. Resolve against the
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
 * Derive the REST base URL from the WS URL. WS and REST share a port
 * (axum routes both); we just upgrade the scheme and drop path/query.
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
    private readonly tokenProvider: () => Promise<string>,
  ) {}

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
   * Fetch a screenshot's bytes with the bearer header. The PWA
   * returns a blob URL via `URL.createObjectURL`; React Native has
   * no equivalent, so the file-based path lands in Phase 5 (we'll
   * write the bytes to `FileSystem.documentDirectory` and pass the
   * resulting `file://` URI to `expo-image`).
   */
  async fetchScreenshot(_relativeOrFullUrl: string): Promise<string> {
    throw new Error("fetchScreenshot not yet implemented on mobile — see MOBILE-PLAN Phase 5");
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
