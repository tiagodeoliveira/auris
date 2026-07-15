import { z } from "zod";
import type { MeetingApi } from "../core/client.js";
import { AuthError, HttpError, NotFoundError } from "../core/client.js";
import {
  matchesFilters,
  paginateTranscript,
  toBriefing,
  toSummary,
  type SearchFilters,
} from "../core/shape.js";

export interface ToolResult {
  content: ({ type: "text"; text: string } | { type: "image"; data: string; mimeType: string })[];
  isError?: boolean;
}

export interface ToolDef {
  name: string;
  description: string;
  schema: z.ZodRawShape;
  handler: (args: Record<string, unknown>) => Promise<ToolResult>;
}

function ok(data: unknown): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify(data, null, 2) }] };
}

function fail(message: string): ToolResult {
  return { content: [{ type: "text", text: message }], isError: true };
}

function okImage(bytes: Uint8Array, mimeType: string): ToolResult {
  return { content: [{ type: "image", data: Buffer.from(bytes).toString("base64"), mimeType }] };
}

/** Run a handler body, mapping known auris errors to isError tool results. */
async function guarded(fn: () => Promise<ToolResult>): Promise<ToolResult> {
  try {
    return await fn();
  } catch (e) {
    if (e instanceof AuthError || e instanceof NotFoundError || e instanceof HttpError) {
      return fail(e.message);
    }
    return fail(`unexpected error: ${(e as Error).message}`);
  }
}

const limitSchema = z.number().int().min(1).max(100).optional();

export function makeTools(client: MeetingApi): ToolDef[] {
  return [
    {
      name: "list_meetings",
      description:
        "List the user's recent meetings (newest first) as compact summaries. Does not include transcripts.",
      schema: { limit: limitSchema },
      handler: (args) =>
        guarded(async () => {
          const limit = (args.limit as number | undefined) ?? 20;
          const all = await client.listMeetings();
          return ok(all.slice(0, limit).map(toSummary));
        }),
    },
    {
      name: "search_meetings",
      description:
        "Search the user's meetings by title/description substring (query), exact project, and/or started_at date range (since/until, YYYY-MM-DD). Returns compact summaries.",
      schema: {
        query: z.string().optional(),
        project: z.string().optional(),
        since: z
          .string()
          .regex(/^\d{4}-\d{2}-\d{2}$/)
          .optional(),
        until: z
          .string()
          .regex(/^\d{4}-\d{2}-\d{2}$/)
          .optional(),
        limit: limitSchema,
      },
      handler: (args) =>
        guarded(async () => {
          const filters: SearchFilters = {
            query: args.query as string | undefined,
            project: args.project as string | undefined,
            since: args.since as string | undefined,
            until: args.until as string | undefined,
          };
          const limit = (args.limit as number | undefined) ?? 20;
          const all = await client.listMeetings();
          const hits = all.filter((m) => matchesFilters(m, filters)).slice(0, limit);
          return ok(hits.map(toSummary));
        }),
    },
    {
      name: "get_meeting",
      description:
        "Fetch one meeting as a briefing: metadata plus the extracted summary, highlights, actions, open questions, and moments. Excludes the raw transcript (use get_meeting_transcript for that).",
      schema: { id: z.string().min(1) },
      handler: (args) =>
        guarded(async () => {
          const detail = await client.getMeeting(args.id as string);
          return ok(toBriefing(detail));
        }),
    },
    {
      name: "get_meeting_transcript",
      description:
        'Fetch a page of one meeting\'s verbatim transcript items. Each item has a `speaker` field (e.g. "Speaker 1", or null when unknown). Use offset/limit to page through long meetings.',
      schema: {
        id: z.string().min(1),
        offset: z.number().int().min(0).optional(),
        limit: z.number().int().min(1).max(500).optional(),
      },
      handler: (args) =>
        guarded(async () => {
          const offset = (args.offset as number | undefined) ?? 0;
          const limit = (args.limit as number | undefined) ?? 200;
          const detail = await client.getMeeting(args.id as string);
          return ok(paginateTranscript(detail, offset, limit));
        }),
    },
    {
      name: "get_moment_screenshot",
      description:
        "Fetch the screenshot image for a specific meeting moment (get its id + has_screenshot from get_meeting). Returns a PNG image.",
      schema: { meeting_id: z.string().min(1), moment_id: z.string().min(1) },
      handler: (args) =>
        guarded(async () => {
          const { bytes, mimeType } = await client.getMomentScreenshot(
            args.meeting_id as string,
            args.moment_id as string,
          );
          return okImage(bytes, mimeType);
        }),
    },
  ];
}
