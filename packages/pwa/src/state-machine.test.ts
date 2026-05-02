import { describe, expect, test } from "vitest";
import { computeNextGlassesView, type GlassesEvent } from "./state-machine";
import type { GlassesView } from "./types";

function transition(from: GlassesView, event: GlassesEvent, ctx = {}) {
  return computeNextGlassesView(from, event, ctx);
}

describe("glasses view state machine", () => {
  test("idle + describe_meeting -> listening", () => {
    expect(transition("idle", { kind: "describe_meeting" })).toBe("listening");
  });
  test("listening + cancel -> idle", () => {
    expect(transition("listening", { kind: "cancel_listening" })).toBe("idle");
  });
  test("listening + commit -> idle (server confirmation moves to active_list)", () => {
    expect(transition("listening", { kind: "commit_listening" })).toBe("idle");
  });
  test("idle + meeting_state_changed{active} -> active_list", () => {
    expect(transition("idle", { kind: "meeting_state_changed", state: "active" })).toBe(
      "active_list",
    );
  });
  test("active_list + ring tap on highlighted -> active_detail", () => {
    expect(transition("active_list", { kind: "ring_click" })).toBe("active_detail");
  });
  test("active_detail + ring tap -> active_list", () => {
    expect(transition("active_detail", { kind: "ring_click" })).toBe("active_list");
  });
  test("active_list + meeting_state_changed{idle} -> idle", () => {
    expect(transition("active_list", { kind: "meeting_state_changed", state: "idle" })).toBe(
      "idle",
    );
  });
  test("active_detail + meeting_state_changed{idle} -> idle", () => {
    expect(transition("active_detail", { kind: "meeting_state_changed", state: "idle" })).toBe(
      "idle",
    );
  });
  test("paused stays in active_list", () => {
    expect(transition("active_list", { kind: "meeting_state_changed", state: "paused" })).toBe(
      "active_list",
    );
  });
  test("unrelated events return current view", () => {
    expect(transition("active_list", { kind: "ring_double_click" })).toBe("active_list");
  });
});
