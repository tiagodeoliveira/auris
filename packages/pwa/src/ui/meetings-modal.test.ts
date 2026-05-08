import { describe, expect, test } from "vitest";
import { pickDetailTitle } from "./meetings-modal";

describe("pickDetailTitle", () => {
  test("prefers metadata.title when present", () => {
    expect(
      pickDetailTitle({
        description: "lots of pasted job-description boilerplate that runs forever and ever",
        metadata: { title: "Recruiter Interview", project: "Blue Origin" },
      }),
    ).toBe("Recruiter Interview");
  });

  test("falls back to first line of description, trimmed and clipped", () => {
    expect(
      pickDetailTitle({
        description: "Quarterly review with Acme.\nSusan + 2 engineers.",
        metadata: {},
      }),
    ).toBe("Quarterly review with Acme.");
  });

  test("clips long single-line descriptions to 80 chars with ellipsis", () => {
    const long = "a".repeat(120);
    const out = pickDetailTitle({ description: long, metadata: {} });
    expect(out.length).toBeLessThanOrEqual(80);
    expect(out.endsWith("…")).toBe(true);
  });

  test("ignores empty metadata.title and falls through to description", () => {
    expect(
      pickDetailTitle({
        description: "Onboarding sync with Tom",
        metadata: { title: "  " },
      }),
    ).toBe("Onboarding sync with Tom");
  });

  test('returns "Untitled meeting" when nothing usable', () => {
    expect(pickDetailTitle({ description: "", metadata: {} })).toBe("Untitled meeting");
    expect(pickDetailTitle({ description: null, metadata: {} })).toBe("Untitled meeting");
  });
});
