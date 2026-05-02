import type { GlassesView, MeetingState } from "./types";

export type GlassesEvent =
  | { kind: "describe_meeting" }
  | { kind: "commit_listening" }
  | { kind: "cancel_listening" }
  | { kind: "ring_click" }
  | { kind: "ring_double_click" }
  | { kind: "ring_scroll_top" }
  | { kind: "ring_scroll_bottom" }
  | { kind: "meeting_state_changed"; state: MeetingState };

export function computeNextGlassesView(
  current: GlassesView,
  event: GlassesEvent,
  _ctx: object,
): GlassesView {
  switch (event.kind) {
    case "describe_meeting":
      return current === "idle" ? "listening" : current;
    case "commit_listening":
    case "cancel_listening":
      return "idle";
    case "ring_click":
      if (current === "active_list") return "active_detail";
      if (current === "active_detail") return "active_list";
      return current;
    case "meeting_state_changed":
      if (event.state === "idle") return "idle";
      if (event.state === "active") {
        if (current === "idle") return "active_list";
        return current; // e.g., resume from paused: stay in active_list/detail
      }
      // paused: stay where we are (no separate paused view)
      return current;
    default:
      return current;
  }
}
