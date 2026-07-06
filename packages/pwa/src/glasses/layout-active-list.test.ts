import { describe, expect, test } from "vitest";
import { defaultAppState } from "../types";
import type { AppState, AudioCaptureState } from "../types";
import {
  GLASSES_STOP_MODE,
  activityFrame,
  activityIndicatorRestingContent,
  buildBodyUpgrade,
  buildHeaderUpgrade,
} from "./layout-active-list";

function headerContent(over: Partial<AppState>): string {
  const state: AppState = {
    ...defaultAppState(),
    availableModes: [{ id: "transcript", label: "Transcript", update_strategy: "replace" }],
    glassesCurrentMode: "transcript",
    ...over,
  };
  // buildHeaderUpgrade returns an SDK TextContainerUpgrade carrying the
  // rendered header string in `.content`.
  return (buildHeaderUpgrade(state) as unknown as { content: string }).content;
}

function bodyContent(over: Partial<AppState>): string {
  const state: AppState = { ...defaultAppState(), ...over };
  return (buildBodyUpgrade(state) as unknown as { content: string }).content;
}

describe("glasses header audio warning", () => {
  test("no warning when streaming during an active meeting", () => {
    const content = headerContent({
      meetingState: "active",
      audioCaptureState: { kind: "streaming", since: 0 } as AudioCaptureState,
    });
    expect(content).toBe("> Transcript");
  });

  test("shows NO AUDIO while reconnecting during an active meeting", () => {
    const content = headerContent({
      meetingState: "active",
      audioCaptureState: { kind: "reconnecting", attempt: 2, since: 0 } as AudioCaptureState,
    });
    expect(content).toContain("NO AUDIO");
    expect(content).toContain("> Transcript");
  });

  test("shows AUDIO LOST when capture failed during an active meeting", () => {
    const content = headerContent({
      meetingState: "active",
      audioCaptureState: { kind: "failed", reason: "x" } as AudioCaptureState,
    });
    expect(content).toContain("AUDIO LOST");
  });

  test("no warning when idle (silent meeting is valid)", () => {
    const content = headerContent({
      meetingState: "active",
      audioCaptureState: { kind: "idle" } as AudioCaptureState,
    });
    expect(content).toBe("> Transcript");
  });

  test("no warning outside an active meeting", () => {
    const content = headerContent({
      meetingState: "idle",
      audioCaptureState: { kind: "reconnecting", attempt: 1, since: 0 } as AudioCaptureState,
    });
    expect(content).toBe("> Transcript");
  });
});

describe("activity indicator audio health", () => {
  function indicator(over: Partial<AppState>): string {
    return activityIndicatorRestingContent({ ...defaultAppState(), ...over });
  }

  test("shows an animation frame while audio is flowing in an active meeting", () => {
    expect(
      indicator({
        meetingState: "active",
        status: { listening: true },
        audioCaptureState: { kind: "streaming", since: 0 } as AudioCaptureState,
      }),
    ).toBe(activityFrame(0));
  });

  test("warns (!!) when this client's audio WS is reconnecting mid-meeting", () => {
    expect(
      indicator({
        meetingState: "active",
        status: { listening: true },
        audioCaptureState: { kind: "reconnecting", attempt: 1, since: 0 } as AudioCaptureState,
      }),
    ).toBe("!!");
  });

  test("warns (!!) when audio capture has failed mid-meeting", () => {
    expect(
      indicator({
        meetingState: "active",
        audioCaptureState: { kind: "failed", reason: "x" } as AudioCaptureState,
      }),
    ).toBe("!!");
  });

  test("blank when not flowing (idle / silent meeting)", () => {
    expect(
      indicator({
        meetingState: "active",
        status: { listening: false },
        audioCaptureState: { kind: "idle" } as AudioCaptureState,
      }),
    ).toBe(" ");
  });

  test("no warning outside an active meeting even if capture state is dirty", () => {
    expect(
      indicator({
        meetingState: "idle",
        audioCaptureState: { kind: "reconnecting", attempt: 1, since: 0 } as AudioCaptureState,
      }),
    ).toBe(" ");
  });
});

describe("glasses stop-mode confirmation body", () => {
  test("disarmed shows the resting '> Stop' affordance only", () => {
    const content = bodyContent({ glassesCurrentMode: GLASSES_STOP_MODE, glassesStopArmed: false });
    expect(content).toContain("> Stop");
    expect(content.toLowerCase()).not.toContain("again");
  });

  test("armed shows a confirm prompt with a cancel hint", () => {
    const content = bodyContent({ glassesCurrentMode: GLASSES_STOP_MODE, glassesStopArmed: true });
    expect(content).toContain("Tap again to end");
    expect(content.toLowerCase()).toContain("cancel");
  });
});
