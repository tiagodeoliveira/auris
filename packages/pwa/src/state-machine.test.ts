import { describe, expect, test } from "vitest";
import { computeNextGlassesView, type GlassesEvent } from "./state-machine";
import type { GlassesView } from "./types";

function transition(from: GlassesView, event: GlassesEvent, ctx = {}) {
  return computeNextGlassesView(from, event, ctx);
}

describe("glasses view state machine", () => {
  test("idle + start_meeting_request -> describe_idle", () => {
    expect(transition("idle", { kind: "start_meeting_request" })).toBe("describe_idle");
  });

  test("describe_idle + begin_describing -> listening", () => {
    expect(transition("describe_idle", { kind: "begin_describing" })).toBe("listening");
  });

  test("listening + commit_listening -> describe_confirm", () => {
    expect(transition("listening", { kind: "commit_listening" })).toBe("describe_confirm");
  });

  test("listening + cancel_listening -> idle", () => {
    expect(transition("listening", { kind: "cancel_listening" })).toBe("idle");
  });

  test("describe_confirm + describe_again -> describe_idle", () => {
    expect(transition("describe_confirm", { kind: "describe_again" })).toBe("describe_idle");
  });

  test("describe_confirm + generate_tags -> select_audio_source", () => {
    expect(transition("describe_confirm", { kind: "generate_tags" })).toBe("select_audio_source");
  });

  test("select_audio_source + cancel returns to describe_confirm", () => {
    expect(transition("select_audio_source", { kind: "cancel_audio_source" })).toBe(
      "describe_confirm",
    );
  });

  test("select_audio_source + meeting_state_changed{active} -> active_list", () => {
    // Server confirms the meeting started — the post-source-pick
    // transition fires as soon as `meeting_state_changed { active }`
    // lands. No intermediate "starting" view; tags arrive
    // asynchronously via MetadataChanged.
    expect(
      transition("select_audio_source", { kind: "meeting_state_changed", state: "active" }),
    ).toBe("active_list");
  });

  test("active_list + ring tap is a no-op at the state-machine level", () => {
    // Single-tap during active_list is meaningful only in the gesture
    // router (mark_moment when on a real mode, stop_meeting on the
    // stop sentinel) — there's no glasses-view transition.
    expect(transition("active_list", { kind: "ring_click" })).toBe("active_list");
  });

  test("active_list + meeting_state_changed{idle} -> idle", () => {
    expect(transition("active_list", { kind: "meeting_state_changed", state: "idle" })).toBe(
      "idle",
    );
  });

  test("describe_confirm + meeting_state_changed{active} jumps to active_list", () => {
    // Covers the race where the server confirms a meeting started
    // by another client while the user is still on the confirm
    // screen — we should follow the meeting, not get stranded.
    expect(transition("describe_confirm", { kind: "meeting_state_changed", state: "active" })).toBe(
      "active_list",
    );
  });

  test("unrelated events return current view", () => {
    expect(transition("active_list", { kind: "ring_double_click" })).toBe("active_list");
  });
});

describe("computeNextGlassesView — history surface", () => {
  test("show_history: idle → history_list", () => {
    expect(computeNextGlassesView("idle", { kind: "show_history" }, {})).toBe("history_list");
  });

  test("show_history is a no-op off the idle menu", () => {
    expect(computeNextGlassesView("active_list", { kind: "show_history" }, {})).toBe("active_list");
  });

  test("open_meeting: history_list → history_summary", () => {
    expect(computeNextGlassesView("history_list", { kind: "open_meeting" }, {})).toBe(
      "history_summary",
    );
  });

  test("history_back: summary → list, list → idle", () => {
    expect(computeNextGlassesView("history_summary", { kind: "history_back" }, {})).toBe(
      "history_list",
    );
    expect(computeNextGlassesView("history_list", { kind: "history_back" }, {})).toBe("idle");
  });

  test("history_back elsewhere is a no-op", () => {
    expect(computeNextGlassesView("active_list", { kind: "history_back" }, {})).toBe("active_list");
  });

  test("pre_meeting_back steps one level up to the menu", () => {
    const back = (v: GlassesView) => computeNextGlassesView(v, { kind: "pre_meeting_back" }, {});
    expect(back("select_audio_source")).toBe("describe_confirm");
    expect(back("describe_confirm")).toBe("describe_idle");
    expect(back("listening")).toBe("describe_idle");
    expect(back("describe_idle")).toBe("idle");
  });

  test("pre_meeting_back elsewhere is a no-op", () => {
    expect(computeNextGlassesView("active_list", { kind: "pre_meeting_back" }, {})).toBe(
      "active_list",
    );
    expect(computeNextGlassesView("history_list", { kind: "pre_meeting_back" }, {})).toBe(
      "history_list",
    );
  });

  test("a meeting starting while browsing history jumps to active_list", () => {
    expect(
      computeNextGlassesView(
        "history_list",
        { kind: "meeting_state_changed", state: "active" },
        {},
      ),
    ).toBe("active_list");
    expect(
      computeNextGlassesView(
        "history_summary",
        { kind: "meeting_state_changed", state: "active" },
        {},
      ),
    ).toBe("active_list");
  });
});
