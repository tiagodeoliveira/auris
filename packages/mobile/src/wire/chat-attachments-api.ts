// REST client for POST /meetings/:id/chat_attachments. The endpoint
// takes RAW image bytes (Content-Type header, not multipart) and
// stages the image for the next Chat intent; the server correlates
// staged attachments to that turn. Returns the assigned attachment id.
//
// Uses fetch with a Blob body (not expo-file-system) so it stays a
// pure module the node-env vitest runner can exercise with a mocked
// global.fetch. Camera photos are downscaled to a few hundred KB
// before upload, so holding the Blob in memory is fine.

import { deriveApiBase, MeetingsApiError } from "./meetings-api";

export class ChatAttachmentsApi {
  constructor(
    private readonly baseUrl: string,
    private readonly tokenProvider: () => Promise<string>,
  ) {}

  static from(
    serverUrl: string,
    tokenProvider: () => Promise<string>,
  ): ChatAttachmentsApi | null {
    const base = deriveApiBase(serverUrl);
    if (!base) return null;
    return new ChatAttachmentsApi(base, tokenProvider);
  }

  /// Upload one staged image. `mime` becomes the Content-Type the
  /// server validates (image/png | image/jpeg). Resolves to the
  /// server-assigned attachment id.
  async upload(meetingId: string, body: Blob, mime: string): Promise<string> {
    const token = await this.tokenProvider();
    const url = `${this.baseUrl}/meetings/${encodeURIComponent(meetingId)}/chat_attachments`;
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
        `Attachment upload failed (HTTP ${resp.status})`,
        resp.status,
      );
    }
    const json = (await resp.json()) as { id: string };
    return json.id;
  }
}
