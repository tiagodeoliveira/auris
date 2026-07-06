//! REST client for the server's `/artifacts` endpoints. Mirror of
//! `packages/mac/Sources/Auris/Net/ArtifactsAPI.swift`.
//! Same WS-URL → REST-base derivation as `MeetingsApi`.

import { deriveApiBase, MeetingsApiError } from "./meetings-api";

/** Wire shape returned by `/artifacts` — matches `ArtifactDto` in
 *  `packages/server/src/api.rs`. */
export interface Artifact {
  id: string;
  name: string;
  mime_type: string;
  short_summary: string | null;
  long_summary: string | null;
  summary_status: "pending" | "done" | "failed" | string;
  size_bytes: number;
  created_at: string;
}

export class ArtifactsApi {
  constructor(
    private readonly baseUrl: string,
    private readonly tokenProvider: () => Promise<string>,
  ) {}

  static from(serverUrl: string, tokenProvider: () => Promise<string>): ArtifactsApi | null {
    const base = deriveApiBase(serverUrl);
    if (!base) return null;
    return new ArtifactsApi(base, tokenProvider);
  }

  list(): Promise<Artifact[]> {
    return this.request<Artifact[]>("/artifacts");
  }

  get(id: string): Promise<Artifact> {
    return this.request<Artifact>(`/artifacts/${encodeURIComponent(id)}`);
  }

  /// Multipart upload with a single `file` field. Returns the
  /// freshly-inserted row with `summary_status: "pending"`; the
  /// async summarizer worker fills summaries shortly after.
  async upload(file: File): Promise<Artifact> {
    const token = await this.tokenProvider();
    const form = new FormData();
    // Browsers infer `Content-Type` per part from the File's type;
    // explicit `name=file` matches the server's expected field name.
    form.append("file", file, file.name);
    let resp: Response;
    try {
      resp = await fetch(this.baseUrl + "/artifacts", {
        method: "POST",
        headers: { Authorization: `Bearer ${token}` },
        body: form,
      });
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      // Surface the server's BadRequest detail (mime rejected, file
      // too large) so users get an actionable message.
      let detail = "";
      try {
        const body = (await resp.json()) as { error?: string; detail?: string };
        detail = body.detail ?? body.error ?? "";
      } catch {
        // Non-JSON error body — fall through to status-code message.
      }
      throw new MeetingsApiError(
        detail ? `Upload failed: ${detail}` : `Upload failed (HTTP ${resp.status})`,
        resp.status,
      );
    }
    return (await resp.json()) as Artifact;
  }

  async delete(id: string): Promise<void> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(this.baseUrl + "/artifacts/" + encodeURIComponent(id), {
        method: "DELETE",
        headers: { Authorization: `Bearer ${token}` },
      });
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new MeetingsApiError(`Delete failed (HTTP ${resp.status})`, resp.status);
    }
  }

  /// Attach a library artifact to a meeting. Server is idempotent
  /// (`ON CONFLICT DO NOTHING`); duplicate attaches are no-ops.
  async attach(meetingId: string, artifactId: string): Promise<void> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(
        this.baseUrl + "/meetings/" + encodeURIComponent(meetingId) + "/artifacts",
        {
          method: "POST",
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ artifact_id: artifactId }),
        },
      );
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new MeetingsApiError(`Attach failed (HTTP ${resp.status})`, resp.status);
    }
  }

  async detach(meetingId: string, artifactId: string): Promise<void> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(
        this.baseUrl +
          "/meetings/" +
          encodeURIComponent(meetingId) +
          "/artifacts/" +
          encodeURIComponent(artifactId),
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
          message = "Artifact not found (404).";
          break;
        default:
          message = `Server returned HTTP ${resp.status}.`;
      }
      throw new MeetingsApiError(message, resp.status);
    }
    return (await resp.json()) as T;
  }
}
