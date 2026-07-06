import { describe, expect, test, vi } from "vitest";
import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import {
  createGestureCoalescer,
  firstEnabledGlassesMode,
  handleBridgeEvent,
} from "./gesture-router";
import { createStore } from "../store";
import { defaultAppState } from "../types";
import { summaryMaxOffset } from "../glasses/layout-history-summary";

describe("gesture-router", () => {
  test("ring CLICK during active meeting dispatches mark_moment", () => {
    // active_list no longer navigates to a detail view; single-tap
    // is now the mark_moment shortcut while a meeting is active.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ type: "mark_moment" }));
    // No view transition — `active_list` stays put.
    expect(store.get().glassesView).toBe("active_list");
  });

  test("single-tap mark_moment carries elapsed-ms t from meetingStartedAt", () => {
    // The wearer taps 90s into the meeting — the intent must carry
    // ~90_000, not 0. With t:0 the server's moment worker windows
    // the transcript at [0, 60s] and summarizes the wrong content.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      meetingStartedAt: Date.now() - 90_000,
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(send).toHaveBeenCalledTimes(1);
    const intent = send.mock.calls[0][0] as { type: string; t: number };
    expect(intent.type).toBe("mark_moment");
    expect(intent.t).toBeGreaterThanOrEqual(90_000);
    expect(intent.t).toBeLessThan(91_000);
  });

  test("listEvent DOUBLE_CLICK mark_moment carries elapsed-ms t (defensive path)", () => {
    // Belt-and-suspenders branch in handleListEvent for firmware that
    // routes a list double-tap through listEvent — must compute the
    // same offset as the primary path.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      meetingStartedAt: Date.now() - 90_000,
    });
    handleBridgeEvent(
      { listEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(send).toHaveBeenCalledTimes(1);
    const intent = send.mock.calls[0][0] as { type: string; t: number };
    expect(intent.type).toBe("mark_moment");
    expect(intent.t).toBeGreaterThanOrEqual(90_000);
    expect(intent.t).toBeLessThan(91_000);
  });

  test("mark_moment falls back to t:0 when meetingStartedAt is null", () => {
    // t:0 is the wire sentinel for "client doesn't know" — the
    // server substitutes its own meeting-clock offset (Task 2).
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(), // meetingStartedAt defaults to null
      glassesView: "active_list",
      meetingState: "active",
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(send).toHaveBeenCalledTimes(1);
    const intent = send.mock.calls[0][0] as { type: string; t: number };
    expect(intent.type).toBe("mark_moment");
    expect(intent.t).toBe(0);
  });

  test("ring SCROLL events on active_list transcript are no-ops (tail-only)", () => {
    // Transcript is live speech — a tail display, newest-at-bottom.
    // Touchpad scroll fires but the router ignores it for transcript;
    // history lives on the phone.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      glassesCurrentMode: "transcript",
      itemsByMode: {
        transcript: [
          { id: "a", text: "x", t: 0 },
          { id: "b", text: "y", t: 0 },
        ],
      },
    });
    const before = store.get();
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } }, store, send);
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.SCROLL_BOTTOM_EVENT } },
      store,
      send,
    );
    expect(store.get()).toBe(before);
    expect(send).not.toHaveBeenCalled();
  });

  test("SCROLL on active_list summary pages back through older items (clamped)", () => {
    // Summary is a digestible list the wearer can read back through.
    // 12 single-line items vs an 8-line budget → max offset 4. A swipe
    // up moves the bottom-anchored offset by SCROLL_STEP (3) toward
    // older content; a second swipe clamps at the max.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      glassesCurrentMode: "summary",
      itemsByMode: {
        summary: Array.from({ length: 12 }, (_, i) => ({ id: `s${i}`, text: `point ${i}`, t: 0 })),
      },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } }, store, send);
    expect(store.get().glassesActiveListLineOffset).toBe(3);
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } }, store, send);
    expect(store.get().glassesActiveListLineOffset).toBe(4); // clamped at max
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.SCROLL_BOTTOM_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesActiveListLineOffset).toBe(1); // back toward latest
    expect(send).not.toHaveBeenCalled();
  });

  test("SCROLL on active_list highlights scrolls too (same scrollable surface)", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      glassesCurrentMode: "highlights",
      itemsByMode: {
        highlights: Array.from({ length: 12 }, (_, i) => ({ id: `h${i}`, text: `hi ${i}`, t: 0 })),
      },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } }, store, send);
    expect(store.get().glassesActiveListLineOffset).toBe(3);
  });

  test("SCROLL on a summary that fits on screen is a no-op", () => {
    // 3 single-line items < the 8-line budget → max offset 0, so there
    // is nothing to scroll into and state stays put.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      glassesCurrentMode: "summary",
      itemsByMode: {
        summary: [
          { id: "s0", text: "a", t: 0 },
          { id: "s1", text: "b", t: 0 },
          { id: "s2", text: "c", t: 0 },
        ],
      },
    });
    const before = store.get();
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } }, store, send);
    expect(store.get()).toBe(before);
  });

  test("DOUBLE_CLICK skips chat — no text-input path from glasses", () => {
    // Chat is filtered out of the glasses cycle. Starting on
    // transcript with [transcript, chat, highlights] available,
    // double-tap should land on highlights (chat is skipped).
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [
        { id: "transcript", label: "Transcript", update_strategy: "append" },
        { id: "chat", label: "Chat", update_strategy: "replace" },
        { id: "highlights", label: "Highlights", update_strategy: "replace" },
      ],
      glassesCurrentMode: "transcript",
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesCurrentMode).toBe("highlights");
    expect(send).not.toHaveBeenCalledWith(expect.objectContaining({ type: "set_mode" }));
  });

  test("DOUBLE_CLICK skips modes the user has disabled via Settings", () => {
    // settings.glassesModes is the user's per-mode cycle opt-out
    // (Settings → Glasses display). With highlights disabled, the
    // cycle should jump straight from transcript past highlights
    // to summary.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [
        { id: "transcript", label: "Transcript", update_strategy: "append" },
        { id: "highlights", label: "Highlights", update_strategy: "replace" },
        { id: "summary", label: "Summary", update_strategy: "replace" },
      ],
      glassesCurrentMode: "transcript",
      settings: {
        serverToken: "",
        lastMetadata: {},
        glassesModes: { highlights: false },
      },
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesCurrentMode).toBe("summary");
  });

  test("DOUBLE_CLICK falls back to hardcoded cycle if user disabled every mode", () => {
    // Defensive: if the user managed to disable every cycle-eligible
    // mode (the Settings UI prevents this, but cross-device sync or
    // direct storage tampering could land such a state), the cycle
    // should still escape via the hardcoded-filtered set rather than
    // degenerate to just the stop sentinel.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [
        { id: "transcript", label: "Transcript", update_strategy: "append" },
        { id: "highlights", label: "Highlights", update_strategy: "replace" },
      ],
      glassesCurrentMode: "transcript",
      settings: {
        serverToken: "",
        lastMetadata: {},
        glassesModes: { transcript: false, highlights: false },
      },
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    // Cycle continues as if no user-opt-out existed.
    expect(store.get().glassesCurrentMode).toBe("highlights");
  });

  test("ring DOUBLE_CLICK while active cycles to next mode", () => {
    // Glasses-only behavior: double-tap cycles through the
    // server-provided modes (with a trailing client-only "stop"
    // sentinel handled in a separate test).
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [
        { id: "transcript", label: "Transcript", update_strategy: "append" },
        { id: "highlights", label: "Highlights", update_strategy: "replace" },
      ],
      glassesCurrentMode: "transcript",
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesCurrentMode).toBe("highlights");
    expect(send).not.toHaveBeenCalledWith(expect.objectContaining({ type: "set_mode" }));
  });

  test("CLICK normalized from undefined eventType still hits mark_moment", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent({ textEvent: { eventType: undefined } }, store, send);
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ type: "mark_moment" }));
  });

  // The simulator (and likely real glasses for primary tap) emits the
  // click as a sysEvent with eventSource=1 (TOUCH_EVENT_FROM_GLASSES_R)
  // and no eventType (proto3 default-omits zero == CLICK_EVENT).
  test("sysEvent click (no eventType, eventSource=1) fires mark_moment", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent({ sysEvent: { eventSource: 1 } }, store, send);
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ type: "mark_moment" }));
  });

  test("sysEvent double-click (eventType=3, eventSource=1) cycles mode", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [
        { id: "transcript", label: "Transcript", update_strategy: "append" },
        { id: "highlights", label: "Highlights", update_strategy: "replace" },
      ],
      glassesCurrentMode: "transcript",
    });
    handleBridgeEvent(
      { sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT, eventSource: 1 } },
      store,
      send,
    );
    expect(store.get().glassesCurrentMode).toBe("highlights");
    expect(send).not.toHaveBeenCalledWith(expect.objectContaining({ type: "set_mode" }));
  });

  test("entry list CLICK on `Start meeting` advances to describe_idle", () => {
    // The new entry menu's first row should kick the meeting-start
    // flow into the dedicated describe screen — replaces the old
    // direct edge into the audio-source picker.
    const send = vi.fn();
    const store = createStore({ ...defaultAppState(), glassesView: "idle" });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 1, // ENTRY_LIST_CONTAINER_ID
          currentSelectItemIndex: 0, // ENTRY_ITEM_START
        },
      },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("describe_idle");
    expect(send).not.toHaveBeenCalled();
  });

  test("describe_idle body CLICK begins describing (listening)", () => {
    // Single-tap on the prompt body advances into capture. Mic open
    // is handled in main.ts via the view-change subscriber, not here.
    const send = vi.fn();
    const store = createStore({ ...defaultAppState(), glassesView: "describe_idle" });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("listening");
  });

  test("listening body CLICK commits description (advance to confirm)", () => {
    // Single-tap during capture is a manual commit — same as VAD
    // silence. Transcript stays put for the confirm preview.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "listening",
      listeningTranscript: "kept across the transition",
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("describe_confirm");
    expect(store.get().listeningTranscript).toBe("kept across the transition");
  });

  test("listening commit promotes interim into the final transcript", () => {
    // Soniox keeps the latest words in `listeningInterim` until they
    // commit — for short captures that "still streaming" tail never
    // makes it into `listeningTranscript` on its own. The commit tap
    // has to merge them in the same update as the view change, or
    // the confirm screen renders an empty preview (the body reads
    // only `listeningTranscript`). Regression cover for that bug.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "listening",
      listeningTranscript: "Domain. ",
      listeningInterim: "Testing if this thing works",
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(store.get().listeningTranscript).toBe("Domain. Testing if this thing works");
    expect(store.get().listeningInterim).toBe("");
  });

  test("confirm list CLICK `Start the meeting` advances to picker (no extract intent)", () => {
    // The Start tap is now a pure view transition — no extract_metadata
    // dispatch. Server auto-extracts when start_meeting fires from the
    // source-pick step; tags arrive asynchronously via MetadataChanged.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "describe_confirm",
      listeningTranscript: "weekly standup",
    });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 3, // DESCRIBE_LIST_ID
          currentSelectItemIndex: 0, // CONFIRM_ITEM_GENERATE
        },
      },
      store,
      send,
    );
    expect(send).not.toHaveBeenCalled();
    expect(store.get().glassesView).toBe("select_audio_source");
  });

  test("confirm list CLICK `Try again` clears transcript and returns to describe_idle", () => {
    // "Try again" semantics: discard the captured text and restart
    // from the empty describe screen.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "describe_confirm",
      listeningTranscript: "not what I meant",
      listeningInterim: "still streaming",
    });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 3, // DESCRIBE_LIST_ID
          currentSelectItemIndex: 1, // CONFIRM_ITEM_TRY_AGAIN
        },
      },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("describe_idle");
    expect(store.get().listeningTranscript).toBe("");
    expect(store.get().listeningInterim).toBe("");
    expect(send).not.toHaveBeenCalled();
  });

  test("DOUBLE_CLICK on describe_idle (sysEvent) backs out to the menu", () => {
    // The screen the user reported as a dead-end: tapping "Start
    // meeting" lands here with no on-screen back. Double-tap now exits
    // to the entry menu, mirroring the history surface's back gesture.
    const send = vi.fn();
    const store = createStore({ ...defaultAppState(), glassesView: "describe_idle" });
    handleBridgeEvent({ sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("idle");
    expect(send).not.toHaveBeenCalled();
  });

  test("DOUBLE_CLICK on listening backs out to describe_idle and discards the transcript", () => {
    // One level up from live capture is the empty describe screen, not
    // the menu — the captured text is dropped (mic teardown happens in
    // main.ts's glassesView reactor, not the router).
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "listening",
      listeningTranscript: "half a sentence",
      listeningInterim: "still streaming",
    });
    handleBridgeEvent({ sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("describe_idle");
    expect(store.get().listeningTranscript).toBe("");
    expect(store.get().listeningInterim).toBe("");
  });

  test("DOUBLE_CLICK on describe_confirm backs out to describe_idle and discards the transcript", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "describe_confirm",
      listeningTranscript: "weekly standup",
    });
    handleBridgeEvent({ sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("describe_idle");
    expect(store.get().listeningTranscript).toBe("");
  });

  test("DOUBLE_CLICK on the audio picker backs out to confirm and KEEPS the transcript", () => {
    // Unlike the describe_idle edges, backing out of the picker
    // preserves the description so the user can re-pick a source
    // without re-describing.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "select_audio_source",
      listeningTranscript: "weekly standup",
    });
    handleBridgeEvent({ sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("describe_confirm");
    expect(store.get().listeningTranscript).toBe("weekly standup");
    expect(send).not.toHaveBeenCalled();
  });

  test("audio-source CLICK fires start_meeting with description + source", () => {
    // Source pick is the trigger for start_meeting. The server auto-
    // extracts tags from the description in the background; the view
    // stays on the picker until `meeting_state_changed{active}` lands
    // (then the state machine advances to active_list).
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "select_audio_source",
      listeningTranscript: "weekly standup",
      availableDevices: [
        { id: "dev-1", hostname: "MacBook", capabilities: ["audio_capture"], online: true },
      ],
    });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 2, // AUDIO_LIST_CONTAINER_ID
          currentSelectItemIndex: 0,
        },
      },
      store,
      send,
    );
    expect(send).toHaveBeenCalledWith({
      type: "start_meeting",
      description: "weekly standup",
      audio_source_device_id: "dev-1",
    });
  });

  test("stop sentinel first CLICK arms the confirmation without stopping", () => {
    // The destructive "end meeting" action now needs a deliberate
    // second tap. The first tap only arms the prompt — no intent is
    // sent and the meeting keeps running.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [{ id: "transcript", label: "Transcript", update_strategy: "append" }],
      glassesCurrentMode: "__stop__",
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(send).not.toHaveBeenCalled();
    expect(store.get().glassesStopArmed).toBe(true);
    expect(store.get().glassesCurrentMode).toBe("__stop__");
  });

  test("stop sentinel second CLICK (armed) fires stop_meeting and resets", () => {
    // Once armed, the next tap confirms: stop the meeting, rewind the
    // mode so the next meeting opens fresh, and disarm.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [{ id: "transcript", label: "Transcript", update_strategy: "append" }],
      glassesCurrentMode: "__stop__",
      glassesStopArmed: true,
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(send).toHaveBeenCalledWith({ type: "stop_meeting" });
    expect(store.get().glassesCurrentMode).toBe("transcript");
    expect(store.get().glassesStopArmed).toBe(false);
  });

  test("DOUBLE_CLICK while armed cancels: disarms and cycles away", () => {
    // The mode-cycle double-tap is the cancel path off the armed
    // stop prompt — it must clear the armed flag and move to a real
    // mode rather than leaving a primed stop behind.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [{ id: "transcript", label: "Transcript", update_strategy: "append" }],
      glassesCurrentMode: "__stop__",
      glassesStopArmed: true,
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesStopArmed).toBe(false);
    expect(store.get().glassesCurrentMode).toBe("transcript");
    expect(send).not.toHaveBeenCalledWith({ type: "stop_meeting" });
  });

  test("DOUBLE_CLICK onto the stop sentinel enters disarmed", () => {
    // Cycling INTO the stop sentinel must start disarmed so a stray
    // single tap can't end the meeting without the explicit arm step.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [{ id: "transcript", label: "Transcript", update_strategy: "append" }],
      // Single real mode → next double-tap lands on the stop sentinel.
      glassesCurrentMode: "transcript",
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesCurrentMode).toBe("__stop__");
    expect(store.get().glassesStopArmed).toBe(false);
  });

  test("sysEvent FOREGROUND_EXIT does NOT trigger a click in gesture-router", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent(
      { sysEvent: { eventType: OsEventTypeList.FOREGROUND_EXIT_EVENT } },
      store,
      send,
    );
    // Stays in active_list — lifecycle.ts handles foreground events
    // separately; gesture-router must not also treat them as clicks.
    expect(store.get().glassesView).toBe("active_list");
    expect(send).not.toHaveBeenCalled();
  });

  test("assist popup: CLICK dismisses without touching the underlying view", () => {
    // The popup is a page-swap modal — but the underlying view
    // state (glassesView, glassesCurrentMode) is preserved so we
    // can rebuild back to it on dismiss. The click handler must
    // ONLY clear `assistShown` and otherwise leave state alone.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      glassesCurrentMode: "highlights",
      assistShown: { id: "as-1", text: "Sodium hydroxide", t: 0, meta: { type: "definition" } },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(store.get().assistShown).toBeNull();
    expect(store.get().glassesView).toBe("active_list");
    expect(store.get().glassesCurrentMode).toBe("highlights");
    // No mark_moment fired: the click was consumed by the popup
    // before falling through to the active_list click handler.
    expect(send).not.toHaveBeenCalled();
  });

  test("assist popup: DOUBLE_CLICK also dismisses (no mode cycle)", () => {
    // Any input event clears the popup so the user doesn't have to
    // be precise about gesture type. Verifying double-tap does NOT
    // cycle the mode — the popup intercepts first.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [
        { id: "transcript", label: "Transcript", update_strategy: "append" },
        { id: "highlights", label: "Highlights", update_strategy: "replace" },
      ],
      glassesCurrentMode: "transcript",
      assistShown: { id: "as-1", text: "Hi", t: 0, meta: { type: "coach" } },
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(store.get().assistShown).toBeNull();
    expect(store.get().glassesCurrentMode).toBe("transcript");
  });

  test("assist popup: SCROLL also dismisses", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      assistShown: { id: "as-1", text: "Hi", t: 0, meta: { type: "memory" } },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } }, store, send);
    expect(store.get().assistShown).toBeNull();
  });
});

describe("firstEnabledGlassesMode", () => {
  test("returns preferred when enabled", () => {
    expect(firstEnabledGlassesMode([], {})).toBe("transcript");
    expect(firstEnabledGlassesMode([], { transcript: true })).toBe("transcript");
  });

  test("skips preferred when user has opted out", () => {
    // Boot-time case: `available` is empty, so the static fallback
    // order kicks in. Transcript is opted out → highlights wins.
    expect(firstEnabledGlassesMode([], { transcript: false })).toBe("highlights");
  });

  test("honors server-provided available modes when present", () => {
    // Post-handshake case: only server-emitted modes are candidates.
    // Even though the static fallback would include `summary`,
    // server's `available` constrains the answer to highlights here.
    const available = [
      { id: "transcript", label: "Transcript", update_strategy: "append" as const },
      { id: "highlights", label: "Highlights", update_strategy: "replace" as const },
    ];
    expect(firstEnabledGlassesMode(available, { transcript: false })).toBe("highlights");
  });

  test("falls back to preferred if every candidate is opted out", () => {
    // Defensive: settings UI prevents all-disabled, but a tampered
    // localStorage blob could land here. Don't return undefined.
    expect(
      firstEnabledGlassesMode([], {
        transcript: false,
        highlights: false,
        summary: false,
        actions: false,
        open_questions: false,
      }),
    ).toBe("transcript");
  });
});

describe("gesture-router — glasses history surface", () => {
  test("entry CLICK on `List meetings` opens the history list (loading)", () => {
    const send = vi.fn();
    const store = createStore({ ...defaultAppState(), glassesView: "idle" });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 1, // ENTRY_LIST_CONTAINER_ID
          currentSelectItemIndex: 1, // ENTRY_ITEM_LIST_MEETINGS
        },
      },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("history_list");
    expect(store.get().glassesHistoryLoading).toBe(true);
    expect(send).not.toHaveBeenCalled();
  });

  test("CLICK on a history row selects it and opens the summary (loading)", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistory: [
        { id: "m-1", description: "First", metadata: {}, started_at: "", ended_at: null },
        { id: "m-2", description: "Second", metadata: {}, started_at: "", ended_at: null },
      ],
    });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 1, // HISTORY_LIST_CONTAINER_ID
          currentSelectItemIndex: 1,
        },
      },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("history_summary");
    expect(store.get().glassesHistorySelectedId).toBe("m-2");
    expect(store.get().glassesHistorySummaryLoading).toBe(true);
  });

  test("CLICK on an out-of-range history row is ignored", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistory: [],
    });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 1,
          currentSelectItemIndex: 0,
        },
      },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("history_list");
    expect(store.get().glassesHistorySelectedId).toBeNull();
  });

  test("single tap on a loading/empty history list (textEvent) is a no-op", () => {
    // The loading/empty/error list screens render as a text container,
    // so their taps arrive via the text-event channel (handleInput).
    // Single-tap there does nothing — only double-tap backs out.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistoryLoading: true,
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("history_list");
    expect(store.get().glassesHistoryLoading).toBe(true);
    expect(send).not.toHaveBeenCalled();
  });

  test("DOUBLE_CLICK on a populated history list (sysEvent — real hardware) returns to the menu", () => {
    // Ground truth (EvenHub SDK): a double-press on a focused
    // ListContainer is delivered as a sysEvent (eventType 3), NOT a
    // listEvent. This is the path real glasses take to back out of a
    // populated list — it must reach handleHistoryBack via handleInput.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistory: [
        { id: "m-1", description: "First", metadata: {}, started_at: "", ended_at: null },
      ],
    });
    handleBridgeEvent({ sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("idle");
    expect(store.get().glassesHistory).toEqual([]);
    expect(send).not.toHaveBeenCalled();
  });

  test("DOUBLE_CLICK on the populated history list via listEvent also backs out (defensive)", () => {
    // Belt-and-suspenders: if a firmware/config ever routes a list
    // double-tap through listEvent instead of sysEvent, the back-out
    // branch in handleListEvent still catches it.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistory: [
        { id: "m-1", description: "First", metadata: {}, started_at: "", ended_at: null },
      ],
    });
    handleBridgeEvent(
      { listEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT, containerID: 1 } },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("idle");
    expect(store.get().glassesHistory).toEqual([]);
    expect(send).not.toHaveBeenCalled();
  });

  test("DOUBLE_CLICK on the empty/error history list (sysEvent) returns to the menu", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistoryError: "Server returned HTTP 500.",
    });
    handleBridgeEvent({ sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("idle");
    expect(store.get().glassesHistoryError).toBeNull();
  });

  test("DOUBLE_CLICK on the summary popup returns to the list", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySelectedId: "m-2",
      glassesHistorySummary: { title: "Second", body: "• point" },
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("history_list");
    expect(store.get().glassesHistorySelectedId).toBeNull();
    expect(store.get().glassesHistorySummary).toBeNull();
  });

  test("single tap on the summary popup is a no-op", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySelectedId: "m-2",
      glassesHistorySummary: { title: "Second", body: "• point" },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, send);
    expect(store.get().glassesView).toBe("history_summary");
    expect(store.get().glassesHistorySelectedId).toBe("m-2");
  });

  // A 30-bullet body is comfortably longer than one screen, so the body
  // window can scroll (summaryMaxOffset > the 3-line step).
  const longBody = Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n");
  const longSummary = { title: "Long", body: longBody };
  const SCROLL_STEP = 3; // lines per swipe (mirrors gesture-router constant)

  test("SCROLL_BOTTOM advances the body window by one scroll step", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySummary: longSummary,
      glassesHistorySummaryLineOffset: 0,
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.SCROLL_BOTTOM_EVENT } },
      store,
      send,
    );
    expect(store.get().glassesHistorySummaryLineOffset).toBe(SCROLL_STEP);
    expect(store.get().glassesView).toBe("history_summary"); // no view change
  });

  test("SCROLL_BOTTOM delivered as a sysEvent (real glasses) also advances", () => {
    // Hardware fans touchpad input out as a sysEvent, not a focused-
    // container textEvent. sysEventAsInput must forward scrolls (not
    // just clicks) or scrolling works in the sim but never on glasses.
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySummary: longSummary,
      glassesHistorySummaryLineOffset: 0,
    });
    handleBridgeEvent(
      { sysEvent: { eventType: OsEventTypeList.SCROLL_BOTTOM_EVENT, eventSource: 1 } },
      store,
      send,
    );
    expect(store.get().glassesHistorySummaryLineOffset).toBe(SCROLL_STEP);
  });

  test("SCROLL_TOP at offset 0 is clamped — stays put", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySummary: longSummary,
      glassesHistorySummaryLineOffset: 0,
    });
    const before = store.get();
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } }, store, send);
    expect(store.get()).toBe(before); // no-op leaves state untouched
  });

  test("SCROLL_BOTTOM clamps at summaryMaxOffset", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySummary: longSummary,
      glassesHistorySummaryLineOffset: 0,
    });
    // Scroll down far past the end; the clamp pins us at the last window.
    for (let i = 0; i < 50; i++) {
      handleBridgeEvent(
        { textEvent: { eventType: OsEventTypeList.SCROLL_BOTTOM_EVENT } },
        store,
        send,
      );
    }
    expect(store.get().glassesHistorySummaryLineOffset).toBe(summaryMaxOffset(longSummary));
    const before = store.get();
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.SCROLL_BOTTOM_EVENT } },
      store,
      send,
    );
    expect(store.get()).toBe(before); // already at end → no-op
  });

  test("opening a meeting resets the summary line offset to 0", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistory: [
        {
          id: "m-1",
          description: "First",
          metadata: {},
          started_at: "2026-01-01T00:00:00Z",
          ended_at: null,
        },
      ],
      glassesHistorySummaryLineOffset: 21, // stale offset from a previous read
    });
    handleBridgeEvent(
      {
        listEvent: {
          eventType: OsEventTypeList.CLICK_EVENT,
          containerID: 1,
          currentSelectItemIndex: 0,
        },
      },
      store,
      send,
    );
    expect(store.get().glassesView).toBe("history_summary");
    expect(store.get().glassesHistorySummaryLineOffset).toBe(0);
  });
});

describe("gesture coalescer", () => {
  test("accepts the first delivery of a gesture", () => {
    const c = createGestureCoalescer(200);
    expect(c.accept(OsEventTypeList.DOUBLE_CLICK_EVENT, 1000)).toBe(true);
  });

  test("drops a same-type duplicate within the window (the hardware fan-out)", () => {
    const c = createGestureCoalescer(200);
    expect(c.accept(OsEventTypeList.DOUBLE_CLICK_EVENT, 1000)).toBe(true);
    // text+sys for one temple-tap land a few ms apart → second dropped.
    expect(c.accept(OsEventTypeList.DOUBLE_CLICK_EVENT, 1005)).toBe(false);
    // A third delivery in the same burst is also collapsed (window
    // stays anchored to the first accepted delivery, not the dupes).
    expect(c.accept(OsEventTypeList.DOUBLE_CLICK_EVENT, 1150)).toBe(false);
  });

  test("accepts the same gesture again once the window has passed", () => {
    const c = createGestureCoalescer(200);
    expect(c.accept(OsEventTypeList.DOUBLE_CLICK_EVENT, 1000)).toBe(true);
    // A deliberate second double-tap is always well beyond the window.
    expect(c.accept(OsEventTypeList.DOUBLE_CLICK_EVENT, 1400)).toBe(true);
  });

  test("does not coalesce across different gesture types", () => {
    const c = createGestureCoalescer(200);
    // A click then a double-click within the window are distinct
    // intents and must both pass.
    expect(c.accept(OsEventTypeList.CLICK_EVENT, 1000)).toBe(true);
    expect(c.accept(OsEventTypeList.DOUBLE_CLICK_EVENT, 1005)).toBe(true);
  });

  test("handleBridgeEvent with a coalescer advances the cycle only once per burst", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
      availableModes: [
        { id: "transcript", label: "Transcript", update_strategy: "append" },
        { id: "highlights", label: "Highlights", update_strategy: "replace" },
        { id: "summary", label: "Summary", update_strategy: "replace" },
      ],
      glassesCurrentMode: "transcript",
    });
    const c = createGestureCoalescer();
    // One physical double-tap, fanned out as text + sys back-to-back.
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
      c,
    );
    handleBridgeEvent(
      { sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT, eventSource: 1 } },
      store,
      send,
      c,
    );
    // Advanced exactly one step — not two.
    expect(store.get().glassesCurrentMode).toBe("highlights");
  });
});
