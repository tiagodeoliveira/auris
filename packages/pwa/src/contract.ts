export const PROTOCOL_VERSION = 1 as const;

export type MeetingState = "idle" | "active" | "paused";
export type UpdateStrategy = "replace" | "append";

export interface ModeOption {
  id: string;
  label: string;
  update_strategy: UpdateStrategy;
}

export interface Item {
  id: string;
  text: string;
  detail?: string;
  t: number;
  meta?: Record<string, unknown>;
}

export interface Status {
  listening: boolean;
  paused: boolean;
  error?: string;
}

export type Intent =
  | { type: "start_meeting"; description?: string; metadata?: Record<string, string> }
  | { type: "stop_meeting" }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "set_mode"; mode: string }
  | { type: "set_metadata"; key: string; value: string | null }
  | { type: "mark_moment"; t: number; note?: string }
  | { type: "expand_item"; item_id: string };

export type Event =
  | {
      type: "snapshot";
      protocol_version: number;
      meeting_state: MeetingState;
      available_modes: ModeOption[];
      mode: string;
      display_tag?: string;
      metadata: Record<string, string>;
      items: Item[];
      status: Status;
    }
  | { type: "meeting_state_changed"; meeting_state: MeetingState }
  | { type: "available_modes_changed"; available_modes: ModeOption[] }
  | { type: "mode_changed"; mode: string; display_tag?: string; items: Item[] }
  | { type: "display_tag_changed"; tag?: string }
  | { type: "metadata_changed"; metadata: Record<string, string> }
  | { type: "items_update"; mode: string; items: Item[] }
  | { type: "transcript_interim"; text: string }
  | { type: "status"; status: Status }
  | { type: "error"; code: string; message: string; intent_ref?: string };

export type ErrorCode =
  | "bad_json"
  | "unknown_intent"
  | "bad_payload"
  | "unknown_mode"
  | "unknown_item";
