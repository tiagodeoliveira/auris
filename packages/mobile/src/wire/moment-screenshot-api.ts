// REST client for POST /meetings/:id/moments/:moment_id/screenshot. The
// endpoint takes RAW image bytes (Content-Type header, not multipart)
// and attaches them to an existing moment; it returns 204 with no body.
//
// Mirrors chat-attachments-api: fetch + Blob so it stays a pure module
// the node-env vitest runner can exercise with a mocked global.fetch.

import { deriveApiBase, MeetingsApiError } from "./meetings-api";

export class MomentScreenshotApi {
  constructor(
    private readonly baseUrl: string,
    private readonly tokenProvider: () => Promise<string>,
  ) {}

  static from(
    serverUrl: string,
    tokenProvider: () => Promise<string>,
  ): MomentScreenshotApi | null {
    const base = deriveApiBase(serverUrl);
    if (!base) return null;
    return new MomentScreenshotApi(base, tokenProvider);
  }

  /// Attach one image to an existing moment. `mime` becomes the
  /// Content-Type the server validates (image/png | image/jpeg).
  async upload(
    meetingId: string,
    momentId: string,
    body: Blob,
    mime: string,
  ): Promise<void> {
    const token = await this.tokenProvider();
    const url = `${this.baseUrl}/meetings/${encodeURIComponent(meetingId)}/moments/${encodeURIComponent(momentId)}/screenshot`;
    let resp: Response;
    try {
      resp = await fetch(url, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}`, "Content-Type": mime },
        body,
      });
    } catch (e) {
      throw new MeetingsApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new MeetingsApiError(
        `Moment screenshot upload failed (HTTP ${resp.status})`,
        resp.status,
      );
    }
  }
}
