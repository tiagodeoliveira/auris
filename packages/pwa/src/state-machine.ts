import type { GlassesView, MeetingState } from "./types";

export type GlassesEvent =
  | { kind: "start_meeting_request" }
  | { kind: "begin_describing" }
  | { kind: "commit_listening" }
  | { kind: "cancel_listening" }
  | { kind: "describe_again" }
  | { kind: "generate_tags" }
  | { kind: "cancel_audio_source" }
  | { kind: "ring_click" }
  | { kind: "ring_double_click" }
  | { kind: "ring_scroll_top" }
  | { kind: "ring_scroll_bottom" }
  | { kind: "show_history" }
  | { kind: "open_meeting" }
  | { kind: "history_back" }
  | { kind: "pre_meeting_back" }
  | { kind: "meeting_state_changed"; state: MeetingState };

export function computeNextGlassesView(
  current: GlassesView,
  event: GlassesEvent,
  _ctx: object,
): GlassesView {
  switch (event.kind) {
    case "start_meeting_request":
      // Entry "> Start meeting" → describe screen. Today this is a
      // direct edge; the old detour through `select_audio_source`
      // moved to the post-confirm `generate_tags` event.
      return current === "idle" ? "describe_idle" : current;
    case "begin_describing":
      // Tap on the describe-idle screen → start capture.
      return current === "describe_idle" ? "listening" : current;
    case "commit_listening":
      // VAD silence or single-tap while capturing → confirm screen
      // with the transcript preserved.
      return current === "listening" ? "describe_confirm" : current;
    case "cancel_listening":
      // Phone-side cancel (cancelListening action) — discards the
      // transcript and returns to the entry menu.
      return "idle";
    case "describe_again":
      // "Describe (to try again)" on the confirm screen → discard
      // transcript and go back to the initial describe screen.
      return current === "describe_confirm" ? "describe_idle" : current;
    case "generate_tags":
      // "Start the meeting" on confirm → audio source picker. Tags
      // are extracted server-side asynchronously once the meeting
      // starts; no spinner step, no client-side extract intent.
      return current === "describe_confirm" ? "select_audio_source" : current;
    case "cancel_audio_source":
      // Back-out from the picker returns to the confirm screen so
      // the user can re-pick "try again" if they want.
      return current === "select_audio_source" ? "describe_confirm" : current;
    case "show_history":
      // Entry "List meetings" → history surface.
      return current === "idle" ? "history_list" : current;
    case "open_meeting":
      // Single-tap a row in the history list → its summary popup.
      return current === "history_list" ? "history_summary" : current;
    case "history_back":
      // Double-tap steps back one level: summary → list → menu.
      if (current === "history_summary") return "history_list";
      if (current === "history_list") return "idle";
      return current;
    case "pre_meeting_back":
      // Double-tap steps back one level out of the "Start meeting"
      // flow, mirroring history_back. Each tap climbs one rung;
      // describe_idle is the floor that exits to the menu. listening
      // and describe_confirm both fall back to the empty describe
      // screen (live capture can't be re-entered) — the gesture
      // router discards the captured transcript on those edges, the
      // same as `describe_again`.
      if (current === "select_audio_source") return "describe_confirm";
      if (current === "describe_confirm") return "describe_idle";
      if (current === "listening") return "describe_idle";
      if (current === "describe_idle") return "idle";
      return current;
    case "ring_click":
      // No transition from active_list — single-tap during a
      // meeting is handled directly by the gesture router (stop-mode
      // commit or no-op).
      return current;
    case "meeting_state_changed":
      if (event.state === "idle") return "idle";
      if (event.state === "active") {
        // Any pre-meeting view (entry / describe flow / picker) lands
        // on the live meeting once the server confirms.
        if (
          current === "idle" ||
          current === "describe_idle" ||
          current === "describe_confirm" ||
          current === "select_audio_source" ||
          current === "history_list" ||
          current === "history_summary"
        ) {
          return "active_list";
        }
        return current;
      }
      return current;
    default:
      return current;
  }
}
