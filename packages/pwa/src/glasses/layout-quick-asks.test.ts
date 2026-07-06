import { describe, expect, test } from "vitest";
import { defaultAppState } from "../types";
import type { AppState } from "../types";
import { ACTIVITY_NAME, activityFrame } from "./layout-active-list";
import { buildQuickAsksLayout, quickAsksSubstate } from "./layout-quick-asks";

type Prop = { containerName?: string; content?: string };

function layout(over: Partial<AppState>) {
  const state: AppState = {
    ...defaultAppState(),
    itemsByMode: { quick_asks: [{ id: "q1", text: "Status report", t: 0 }] },
    ...over,
  };
  return buildQuickAsksLayout(state) as unknown as {
    containerTotalNum?: number;
    listObject?: Prop[];
    textObject?: Prop[];
  };
}

function activity(textObject: Prop[] | undefined): Prop | undefined {
  return (textObject ?? []).find((c) => c.containerName === ACTIVITY_NAME);
}

describe("quick-asks recording indicator", () => {
  test("list substate carries the activity indicator alongside the list", () => {
    const state = { status: { listening: true } } as Partial<AppState>;
    expect(quickAsksSubstate({ ...defaultAppState(), ...state } as AppState)).toBe("list");
    const page = layout(state);
    expect(page.listObject).toHaveLength(1);
    expect(page.containerTotalNum).toBe(2);
    expect(activity(page.textObject)?.content).toBe(activityFrame(0));
  });

  test("waiting substate keeps both the spinner body and the indicator", () => {
    const page = layout({ quickAskWaiting: true, status: { listening: true } });
    expect(page.textObject).toHaveLength(2);
    expect(page.textObject?.some((c) => (c.content ?? "").includes("Asking"))).toBe(true);
    expect(activity(page.textObject)?.content).toBe(activityFrame(0));
    expect(page.containerTotalNum).toBe(2);
  });

  test("answer substate keeps both the answer body and the indicator", () => {
    const page = layout({ quickAskAnswerText: "the answer", status: { listening: true } });
    expect(page.textObject).toHaveLength(2);
    expect(page.textObject?.some((c) => (c.content ?? "").includes("the answer"))).toBe(true);
    expect(activity(page.textObject)?.content).toBe(activityFrame(0));
    expect(page.containerTotalNum).toBe(2);
  });

  test("indicator shows the idle glyph (single space) when audio is not flowing", () => {
    const page = layout({ status: { listening: false } });
    expect(activity(page.textObject)?.content).toBe(" ");
  });
});
