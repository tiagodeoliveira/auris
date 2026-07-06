// REST client for the server's `/artifacts` endpoints. Hand-ported
// from packages/pwa/src/artifacts-api.ts.
//
// The shape of `upload()` differs from the PWA: React Native's
// FormData expects `{ uri, name, type }` shapes (paths into the
// device filesystem) rather than browser `File` objects. The
// caller passes the result of `expo-image-picker` /
// `expo-document-picker` directly.

import { deriveApiBase, MeetingsApiError } from "./meetings-api";

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

/// Mobile-specific upload payload. Mirrors the shape returned by
/// `expo-image-picker` (asset.uri / .fileName / .mimeType) and
/// `expo-document-picker` (DocumentPickerAsset).
export interface UploadFile {
  uri: string;
  name: string;
  type: string;
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

  /// List artifacts attached to a meeting. Returns an empty array
  /// when nothing is attached; a 404 is treated as a real error
  /// (unknown / not-owned meeting) and surfaced via MeetingsApiError.
  async listForMeeting(meetingId: string): Promise<Artifact[]> {
    const body = await this.request<{ artifacts: Artifact[] }>(
      `/meetings/${encodeURIComponent(meetingId)}/artifacts`,
    );
    return body.artifacts ?? [];
  }

  /// Multipart upload of a file referenced by its on-device URI.
  /// React Native's FormData picks up `{ uri, name, type }` shapes
  /// natively — the runtime streams the file from disk rather than
  /// loading the bytes into memory the way the browser's `File`
  /// path does.
  async upload(file: UploadFile): Promise<Artifact> {
    const token = await this.tokenProvider();
    const form = new FormData();
    // The cast quiets TS — RN extends FormData beyond Blob | string
    // to include the {uri,name,type} shape, but the @types/react-native
    // declarations don't always reflect it cleanly.
    form.append("file", { uri: file.uri, name: file.name, type: file.type } as unknown as Blob);
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
  /// (`ON CONFLICT DO NOTHING`).
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
