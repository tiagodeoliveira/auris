export const PROTOCOL_VERSION = 1 as const;

export type MeetingState = "idle" | "active";
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
  error?: string;
}

export interface PriorContextSummary {
  preferences: number;
  facts: number;
  episodes: number;
  project_memories: number;
}

export type Capability = "audio_capture" | "screen_capture" | "control_surface" | "system_audio";

/** Per-meeting assist surface sensitivity. Mirrors
 * `packages/server/src/protocol/mod.rs` `AssistSensitivity`.
 * Aggressive = lower threshold + prompt nudge to fire often.
 * Moderate = historical default. Minimal = high threshold + only-on-
 * unmistakable-signal nudge. The PWA's compose screen offers a
 * segmented picker; mid-meeting toggle reuses the same value set. */
export type AssistSensitivity = "aggressive" | "moderate" | "minimal";

export interface Device {
  id: string;
  hostname: string;
  capabilities: Capability[];
  online: boolean;
}

export type Intent =
  | {
      type: "start_meeting";
      description?: string;
      metadata?: Record<string, string>;
      /** Device id the server should bind as the audio source for
       * the new meeting. The chosen device sees the resulting
       * `audio_source_device_changed` event and starts streaming
       * `/audio`. Omit for a silent meeting (no source bound). */
      audio_source_device_id?: string;
      /** Per-meeting assist sensitivity. Omitted = server default
       * (Moderate, matching the historical behavior pre-feature). */
      assist_sensitivity?: AssistSensitivity;
    }
  | { type: "stop_meeting" }
  | { type: "set_assist_sensitivity"; value: AssistSensitivity }
  | { type: "set_mode"; mode: string }
  | { type: "set_metadata"; key: string; value: string | null }
  | {
      type: "upsert_quick_ask";
      id: string;
      label: string;
      text: string;
      position: number;
    }
  | { type: "delete_quick_ask"; id: string }
  | {
      type: "register_device";
      hostname: string;
      capabilities: Capability[];
      /** Stable device id persisted across reconnects so the server
       * keeps our identity (and audio-source binding) when the socket
       * drops + reconnects. See storage.getOrCreateDeviceId. */
      device_id?: string;
    }
  | { type: "mark_moment"; t: number; note?: string }
  | { type: "expand_item"; item_id: string }
  | { type: "chat"; text: string };

export type Event =
  | {
      type: "snapshot";
      protocol_version: number;
      meeting_state: MeetingState;
      /** Server-assigned id of the active meeting. `Some` while
       * active; `None` when idle. Used to attach artifacts to the
       * running meeting via `POST /meetings/:id/artifacts`. */
      meeting_id?: string;
      available_modes: ModeOption[];
      mode: string;
      display_tag?: string;
      metadata: Record<string, string>;
      items: Item[];
      status: Status;
      prior_context?: PriorContextSummary;
      devices: Device[];
      audio_source_device_id?: string;
      /** Past meetings attached to this meeting (see
       * `AttachedMeetingsChanged` for live updates). Snapshot ships
       * an empty list; the server fires a synthetic
       * `AttachedMeetingsChanged` immediately after the snapshot
       * when the active meeting has attachments. */
      attached_meeting_ids?: string[];
      /** Active meeting's assist sensitivity (or default when idle).
       * Mid-meeting changes fire `assist_sensitivity_changed`. */
      assist_sensitivity?: AssistSensitivity;
    }
  | { type: "meeting_state_changed"; meeting_state: MeetingState; meeting_id?: string }
  | { type: "assist_sensitivity_changed"; value: AssistSensitivity }
  | { type: "prior_context_changed"; summary: PriorContextSummary }
  | { type: "device_registered"; device: Device }
  | { type: "devices_changed"; devices: Device[] }
  | { type: "audio_source_device_changed"; device_id?: string }
  | { type: "mode_changed"; mode: string; display_tag?: string; items: Item[] }
  | { type: "display_tag_changed"; tag?: string }
  | { type: "metadata_changed"; metadata: Record<string, string> }
  | { type: "items_update"; mode: string; items: Item[] }
  | { type: "item_updated"; mode: string; item: Item }
  | { type: "transcript_interim"; text: string }
  | { type: "status"; status: Status }
  | { type: "error"; code: string; message: string; intent_ref?: string }
  | { type: "artifacts_changed"; artifact_ids: string[] }
  | { type: "attached_meetings_changed"; meeting_ids: string[] };

export type ErrorCode =
  | "bad_json"
  | "unknown_intent"
  | "bad_payload"
  | "unknown_mode"
  | "unknown_item";
