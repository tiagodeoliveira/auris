import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import { GLASSES_STOP_MODE } from "../glasses/layout-active-list";
import { ACTIVE_LIST_VISIBLE_LINES, activeListMaxOffset } from "../glasses/format-active-list";
import {
  ENTRY_ITEM_START,
  ENTRY_ITEM_LIST_MEETINGS,
  ENTRY_LIST_CONTAINER_ID,
} from "../glasses/layout-entry";
import { HISTORY_LIST_CONTAINER_ID } from "../glasses/layout-history-list";
import { summaryMaxOffset } from "../glasses/layout-history-summary";
import {
  CONFIRM_ITEM_GENERATE,
  CONFIRM_ITEM_TRY_AGAIN,
  DESCRIBE_LIST_ID,
} from "../glasses/layout-describe";
import {
  AUDIO_ITEM_SILENT,
  AUDIO_LIST_CONTAINER_ID,
  audioSourceOptions,
} from "../glasses/layout-select-audio-source";
import { QUICK_ASKS_LIST_ID } from "../glasses/layout-quick-asks";

const QUICK_ASKS_MODE = "quick_asks";
import { computeNextGlassesView } from "../state-machine";
import type { Store } from "../store";
import { activeGlassesItems } from "../types";
import type { Intent } from "../types";
import type { AppState } from "../types";
import type { ModeOption } from "../contract";

/// Cycle through real modes followed by the client-only stop
/// sentinel. Returns the next mode id (always a string — caller
/// decides whether it's a real mode → send `set_mode`, or
/// `GLASSES_STOP_MODE` → set locally).
///
/// Chat is filtered out: there's no text-input path from glasses,
/// so cycling onto chat would strand the user. If the server still
/// pushes `mode_changed { mode: "chat" }` (e.g. the phone switched),
/// the next double-tap escapes via the `-1 → modes[0]` fallthrough.
function nextMode(state: AppState): string {
  const modes = [...glassesModeIds(state), GLASSES_STOP_MODE];
  const idx = modes.indexOf(state.glassesCurrentMode);
  return modes[(idx + 1) % modes.length] ?? modes[0];
}

/// Server-provided mode ids minus the ones we don't show on glasses.
/// Today the only exclusion is `chat` (no input path on glasses).
/// Pick the initial `glassesCurrentMode` that respects the user's
/// per-mode opt-outs (Settings → Glasses display). Used at two
/// chokepoints where the historical default was a hardcoded
/// `"transcript"`:
///   1. boot, after `loadSettings` lands but before WS handshake
///      (so `available` is empty — falls back to a static order),
///   2. meeting-end reset in `ws-handlers.ts` (`available` is the
///      server-provided cycle).
/// `preferred` is the mode we'd pick if user opt-outs let us
/// (defaults to `"transcript"` to preserve historical behavior).
/// Final fallback is `preferred` unchanged so a fresh client with
/// no settings doesn't end up on a bogus mode id even if the
/// `available` list arrives empty.
export function firstEnabledGlassesMode(
  available: ModeOption[],
  glassesModes: Record<string, boolean>,
  preferred: string = "transcript",
): string {
  const cycleEligible = available
    .filter((m) => m.id !== "chat" && m.id !== "assist")
    .map((m) => m.id);
  // Pre-handshake fallback: `available` arrives via the snapshot
  // event, so boot-time callers see an empty list. Use the
  // historically-shipped mode order as the candidate set so we can
  // still honor opt-outs before the server's mode catalog lands.
  const candidates =
    cycleEligible.length > 0
      ? cycleEligible
      : ["transcript", "highlights", "summary", "actions", "open_questions"];
  const enabled = candidates.filter((id) => glassesModes[id] !== false);
  if (enabled.includes(preferred)) return preferred;
  return enabled[0] ?? preferred;
}

export function glassesModeIds(state: AppState): string[] {
  // Three layers of filtering, applied in order:
  //   1. Hardcoded exclusions: `chat` has no text-input path from
  //      glasses; `assist` is companion-only while we calibrate
  //      quality (phase 2 brings it into the cycle).
  //   2. User per-mode opt-out via Settings → Glasses display. Drops
  //      modes the user has set to `false` so the glasses don't
  //      have to render them (perf win for sluggish text views like
  //      transcript).
  //   3. Fallback: if the user disabled every cycle-eligible mode,
  //      return the hardcoded-filtered set unchanged. Otherwise the
  //      cycle would degenerate to just GLASSES_STOP_MODE and a
  //      double-tap couldn't escape back to a real mode.
  const hardcodedFiltered = state.availableModes
    .filter((m) => m.id !== "chat" && m.id !== "assist")
    .map((m) => m.id);
  const userFiltered = hardcodedFiltered.filter((id) => state.settings.glassesModes[id] !== false);
  return userFiltered.length > 0 ? userFiltered : hardcodedFiltered;
}

/// Millisecond offset from meeting start for a `mark_moment`
/// intent — the wire contract's `t`. Falls back to 0 when the
/// client never observed the meeting going active (fresh reload
/// mid-meeting): the server treats `t == 0` as "client doesn't
/// know" and computes the offset from its own meeting clock.
export function markMomentT(state: Pick<AppState, "meetingStartedAt">): number {
  return state.meetingStartedAt ? Math.max(0, Date.now() - state.meetingStartedAt) : 0;
}

interface BridgeEvent {
  textEvent?: { eventType?: number | undefined };
  listEvent?: {
    eventType?: number | undefined;
    containerID?: number;
    currentSelectItemIndex?: number;
  };
  sysEvent?: { eventType?: number; eventSource?: number };
}

type SendIntent = (intent: Intent) => void;

/// Coalesces duplicate hardware deliveries of a single physical
/// gesture. Real glasses fan one temple-tap out across more than one
/// delivery channel (a focused-container `textEvent` AND a system
/// `sysEvent`, arriving as separate `onEvenHubEvent` calls a few ms
/// apart) — so a single double-tap was advancing the mode cycle twice.
/// The simulator delivers one event per gesture, which is why it never
/// reproduces there. `accept` returns false for a same-type delivery
/// that lands within `windowMs` of the last accepted one; two
/// *intentional* gestures are always far further apart than a hardware
/// fan-out burst, so legitimate input is never swallowed.
export interface GestureCoalescer {
  accept(eventType: number, nowMs: number): boolean;
}

/// Window is generous vs the ~ms hardware fan-out but well under the
/// cadence of two deliberate taps (a single double-tap alone spans
/// ~200-300ms, so distinct gestures can't fall inside it).
export const GESTURE_COALESCE_MS = 200;

export function createGestureCoalescer(windowMs: number = GESTURE_COALESCE_MS): GestureCoalescer {
  let lastType: number | null = null;
  let lastTs = -Infinity;
  return {
    accept(eventType, nowMs) {
      if (eventType === lastType && nowMs - lastTs < windowMs) {
        // Duplicate of the same gesture — drop it. Don't advance
        // `lastTs`, so the window stays anchored to the first
        // (accepted) delivery and a 3-way fan-out is fully collapsed
        // without ever extending past a real next gesture.
        return false;
      }
      lastType = eventType;
      lastTs = nowMs;
      return true;
    },
  };
}

export function handleBridgeEvent(
  event: BridgeEvent,
  store: Store,
  send: SendIntent,
  coalescer?: GestureCoalescer,
): void {
  // List events carry the currently selected item index alongside
  // the event type. Route them through a dedicated handler so each
  // menu can dispatch contextually by item.
  if (event.listEvent) {
    handleListEvent(event.listEvent, store, send);
    return;
  }
  const inputEvent = event.textEvent ?? sysEventAsInput(event.sysEvent);
  if (inputEvent) {
    const eventType = normalizeEventType(inputEvent.eventType);
    // Drop hardware-duplicated deliveries of the same gesture (glasses
    // emit text+sys for one temple-tap; the sim emits one). Without a
    // coalescer (tests) every event is processed as before.
    if (coalescer && !coalescer.accept(eventType, Date.now())) return;
    handleInput(eventType, store, send);
  }
}

function normalizeEventType(t: number | undefined): number {
  return t === undefined ? OsEventTypeList.CLICK_EVENT : t;
}

/// Real glasses deliver touchpad input as a `sysEvent` — *not* as a
/// textEvent on the focused container — with `eventSource: 1`
/// (TOUCH_EVENT_FROM_GLASSES_R). proto3 JSON omits scalar zeros, so a
/// sysEvent with no `eventType` field is implicitly CLICK_EVENT (0);
/// double-click arrives serialized as `eventType: 3`, and the two
/// scroll directions as 1/2. Forward clicks AND scrolls so the summary
/// page-flip (driven from `handleInput`) works on hardware, not just in
/// the simulator (where scroll arrives as a textEvent). Other event
/// types (foreground/exit/IMU) are not gestures and are dropped here.
function sysEventAsInput(
  sys: { eventType?: number; eventSource?: number } | undefined,
): { eventType?: number } | undefined {
  if (!sys) return undefined;
  const t = sys.eventType ?? OsEventTypeList.CLICK_EVENT;
  if (
    t === OsEventTypeList.CLICK_EVENT ||
    t === OsEventTypeList.DOUBLE_CLICK_EVENT ||
    t === OsEventTypeList.SCROLL_TOP_EVENT ||
    t === OsEventTypeList.SCROLL_BOTTOM_EVENT
  ) {
    return { eventType: t };
  }
  return undefined;
}

/// Step back one level out of the history surface. `computeNextGlassesView`
/// maps history_summary → history_list and history_list → idle; we clear
/// the slice that's no longer visible (everything when returning to the
/// menu; just the open-summary fields when returning to the list).
function handleHistoryBack(store: Store): void {
  const state = store.get();
  const next = computeNextGlassesView(state.glassesView, { kind: "history_back" }, {});
  if (next === state.glassesView) return; // not in a history view
  if (next === "idle") {
    store.update({
      glassesView: "idle",
      glassesHistory: [],
      glassesHistoryLoading: false,
      glassesHistoryError: null,
      glassesHistorySelectedId: null,
      glassesHistorySummary: null,
      glassesHistorySummaryLoading: false,
      glassesHistorySummaryError: null,
      glassesHistorySummaryLineOffset: 0,
    });
  } else {
    store.update({
      glassesView: next,
      glassesHistorySelectedId: null,
      glassesHistorySummary: null,
      glassesHistorySummaryLoading: false,
      glassesHistorySummaryError: null,
      glassesHistorySummaryLineOffset: 0,
    });
  }
}

/// Lines the summary window moves per swipe. A few rows at a time reads as a
/// continuous scroll while still covering ground (matches the sibling ERGram).
const SCROLL_STEP = 3;

/// Scroll the open summary's body window up/down by `delta` display rows,
/// clamped to [0, summaryMaxOffset]. The bound comes from the same helper the
/// layout renders with, so the clamp can never strand the wearer past the
/// content. A no-op (delta off either end, or no summary loaded) leaves state
/// untouched so the renderer doesn't needlessly rebuild.
function scrollSummaryLines(store: Store, delta: number): void {
  const state = store.get();
  const s = state.glassesHistorySummary;
  if (!s) return;
  const max = summaryMaxOffset(s);
  const next = Math.min(Math.max(state.glassesHistorySummaryLineOffset + delta, 0), max);
  if (next !== state.glassesHistorySummaryLineOffset) {
    store.update({ glassesHistorySummaryLineOffset: next });
  }
}

/// Modes whose live active-meeting body is scrollable (bottom-anchored
/// window) rather than a fixed tail. Only the digestible LLM lists —
/// transcript/chat stay tail-only.
function activeListIsScrollable(mode: string): boolean {
  return mode === "summary" || mode === "highlights";
}

/// Scroll the live summary/highlights window up/down by `delta` display
/// rows, clamped to [0, activeListMaxOffset]. Bottom-anchored: a positive
/// offset reveals older rows. A no-op outside the scrollable modes, or
/// when the delta is clamped away, so the renderer doesn't needlessly
/// rebuild. The clamp uses the same helper the layout renders with, so
/// they can never disagree about how far the wearer can scroll.
function scrollActiveList(store: Store, delta: number): void {
  const state = store.get();
  if (state.glassesView !== "active_list" || !activeListIsScrollable(state.glassesCurrentMode)) {
    return;
  }
  const max = activeListMaxOffset(activeGlassesItems(state), ACTIVE_LIST_VISIBLE_LINES);
  const next = Math.min(Math.max(state.glassesActiveListLineOffset + delta, 0), max);
  if (next !== state.glassesActiveListLineOffset) {
    store.update({ glassesActiveListLineOffset: next });
  }
}

/// Step back one level out of the "Start meeting" flow.
/// `computeNextGlassesView` maps select_audio_source → describe_confirm,
/// describe_confirm/listening → describe_idle, and describe_idle → idle
/// (the menu). Edges that land on describe_idle discard the captured
/// transcript (the empty describe screen implies a fresh start, matching
/// `describe_again`); the back-to-confirm edge preserves it so the user
/// can re-pick an audio source without re-describing.
function handlePreMeetingBack(store: Store): void {
  const state = store.get();
  const next = computeNextGlassesView(state.glassesView, { kind: "pre_meeting_back" }, {});
  if (next === state.glassesView) return; // not in a pre-meeting view
  if (next === "describe_confirm") {
    store.update({ glassesView: next }); // keep the captured transcript
  } else {
    // → describe_idle or the menu: discard any captured text. Leaving
    // "listening" also tears down the mic via the reactor in main.ts.
    store.update({ glassesView: next, listeningTranscript: "", listeningInterim: "" });
  }
}

/// Dispatch for `list_event`. Four distinct lists hit this handler:
///   - the entry menu (start meeting / list meetings)
///   - the history list (list meetings → pick a past meeting)
///   - the describe-confirm menu (generate tags / try again)
///   - the audio-source picker (after confirm, before the meeting)
/// The handler routes by `containerID` (disambiguated by `glassesView`
/// where ids collide) so each menu's semantics stay separate.
function handleListEvent(
  list: { eventType?: number; containerID?: number; currentSelectItemIndex?: number },
  store: Store,
  send: SendIntent,
): void {
  const state = store.get();
  const eventType = normalizeEventType(list.eventType);

  // Double-tap during an active meeting marks a moment. The describe
  // states no longer respond to double-tap (single-tap covers commit
  // via the body container's textEvent path).
  if (eventType === OsEventTypeList.DOUBLE_CLICK_EVENT) {
    // Defensive: on real hardware a double-press — even on a focused
    // ListContainer — is delivered as a `sysEvent` (eventType 3), so the
    // history back-out normally lands in handleInput, NOT here. We keep
    // this branch as belt-and-suspenders in case a firmware/config combo
    // ever routes a list double-tap through listEvent.
    if (state.glassesView === "history_list" || state.glassesView === "history_summary") {
      handleHistoryBack(store);
      return;
    }
    if (state.meetingState === "active") {
      send({ type: "mark_moment", t: markMomentT(state) });
    }
    return;
  }

  if (eventType !== OsEventTypeList.CLICK_EVENT) return;
  // proto3 JSON serialization strips scalar zeros, so a click on
  // the first list item (index 0) arrives with
  // `currentSelectItemIndex: undefined`. Coerce missing → 0 so the
  // top item still dispatches.
  const index = list.currentSelectItemIndex ?? 0;

  // Entry menu: `> Start meeting` advances to the describe screen;
  // `List meetings` opens the history surface (the reactor in main.ts
  // does the async fetch once we flip the view + set the loading flag).
  if (state.glassesView === "idle" && list.containerID === ENTRY_LIST_CONTAINER_ID) {
    if (index === ENTRY_ITEM_START) {
      const next = computeNextGlassesView(state.glassesView, { kind: "start_meeting_request" }, {});
      store.update({ glassesView: next });
    } else if (index === ENTRY_ITEM_LIST_MEETINGS) {
      const next = computeNextGlassesView(state.glassesView, { kind: "show_history" }, {});
      store.update({
        glassesView: next,
        glassesHistory: [],
        glassesHistoryLoading: true,
        glassesHistoryError: null,
        glassesHistorySelectedId: null,
      });
    }
    return;
  }

  // History list: single-tap a row opens its summary. A single-press on
  // a focused ListContainer is delivered as a listEvent carrying the row
  // index — so row SELECT lands here. (Double-tap-to-go-back does NOT:
  // the firmware sends list double-presses as a sysEvent, handled in
  // handleInput. Loading/empty/error are text containers — also
  // handleInput.)
  if (state.glassesView === "history_list" && list.containerID === HISTORY_LIST_CONTAINER_ID) {
    const picked = state.glassesHistory[index];
    if (!picked) return;
    const next = computeNextGlassesView(state.glassesView, { kind: "open_meeting" }, {});
    store.update({
      glassesView: next,
      glassesHistorySelectedId: picked.id,
      glassesHistorySummary: null,
      glassesHistorySummaryLoading: true,
      glassesHistorySummaryError: null,
      glassesHistorySummaryLineOffset: 0,
    });
    return;
  }

  // Confirm menu (post-describe): advance to the audio-source picker,
  // or discard the transcript and restart description capture. No
  // separate "extract tags" intent — server auto-extracts on
  // start_meeting, and tags arrive asynchronously via MetadataChanged
  // once the LLM returns.
  if (state.glassesView === "describe_confirm" && list.containerID === DESCRIBE_LIST_ID) {
    if (index === CONFIRM_ITEM_GENERATE) {
      const next = computeNextGlassesView(state.glassesView, { kind: "generate_tags" }, {});
      store.update({ glassesView: next });
      return;
    }
    if (index === CONFIRM_ITEM_TRY_AGAIN) {
      // Discard the captured transcript and go back to the empty
      // describe screen.
      const next = computeNextGlassesView(state.glassesView, { kind: "describe_again" }, {});
      store.update({
        glassesView: next,
        listeningTranscript: "",
        listeningInterim: "",
      });
      return;
    }
    return;
  }

  // Quick-asks list: tap on an item dispatches the snippet's full
  // text as a Chat intent and flips the view into the "waiting"
  // sub-state. The answer detector in main.ts watches chat-mode
  // items_update to land the response. The `text` field on the wire
  // item is the label; the full prompt lives in `detail`.
  if (
    state.glassesView === "active_list" &&
    state.glassesCurrentMode === QUICK_ASKS_MODE &&
    list.containerID === QUICK_ASKS_LIST_ID
  ) {
    const items = state.itemsByMode[QUICK_ASKS_MODE] ?? [];
    const picked = items[index];
    if (!picked) return;
    const prompt = (picked.detail ?? picked.text).trim();
    if (prompt.length === 0) return;
    // Snapshot the chat tail BEFORE dispatch so the answer detector
    // in main.ts can distinguish "the new answer" from "the previous
    // answer still in history" — critical when the user re-sends the
    // same prompt (chips encourage this; bare lookup-by-latest would
    // lock onto the previous turn until the new one streams in).
    const dispatchAt = state.itemsByMode["chat"]?.length ?? 0;
    send({ type: "chat", text: prompt });
    store.update({
      quickAskWaiting: true,
      quickAskAnswerText: null,
      quickAskDispatchAt: dispatchAt,
    });
    return;
  }

  // Audio-source picker: fire `start_meeting` immediately with the
  // captured description + chosen source. The state machine waits
  // for `meeting_state_changed { active }` to transition the view to
  // `active_list`. Tags trickle in later via `MetadataChanged` and
  // update the active meeting in place.
  if (state.glassesView === "select_audio_source" && list.containerID === AUDIO_LIST_CONTAINER_ID) {
    const options = audioSourceOptions(state);
    const choice = options[index];
    if (!choice) return;
    const description = state.listeningTranscript.trim();
    send({
      type: "start_meeting",
      description: description || undefined,
      audio_source_device_id: choice.key === AUDIO_ITEM_SILENT ? undefined : choice.key,
    });
    return;
  }
}

function handleInput(eventType: number, store: Store, send: SendIntent): void {
  const state = store.get();

  // Assist popup intercepts every input event regardless of the
  // underlying view. Any gesture (single tap, double tap, scroll
  // up/down) clears the popup; we don't penalise the user for
  // tapping slightly off the dedicated dismiss action. The next
  // queued assist item — if any — auto-pops via the detector in
  // main.ts as soon as `assistShown` clears.
  if (state.assistShown !== null) {
    store.update({ assistShown: null });
    return;
  }

  // History surface: this is the PRIMARY back-out path. Per the EvenHub
  // SDK, clicks/double-clicks on text containers AND double-presses on
  // list containers all arrive as sysEvent (normalised to here), while
  // only a list row-SELECT arrives as a listEvent (handled above). So
  // every history double-tap-to-go-back lands here regardless of which
  // screen (populated list / loading / empty / error / summary popup) is
  // showing. Single-tap on this channel is a deliberate no-op.
  if (state.glassesView === "history_summary") {
    // Double-tap backs out to the list; scroll up/down slides the body
    // window by a few rows. Single-tap stays a deliberate no-op.
    if (eventType === OsEventTypeList.DOUBLE_CLICK_EVENT) handleHistoryBack(store);
    else if (eventType === OsEventTypeList.SCROLL_TOP_EVENT)
      scrollSummaryLines(store, -SCROLL_STEP);
    else if (eventType === OsEventTypeList.SCROLL_BOTTOM_EVENT)
      scrollSummaryLines(store, +SCROLL_STEP);
    return;
  }
  if (state.glassesView === "history_list") {
    if (eventType === OsEventTypeList.DOUBLE_CLICK_EVENT) handleHistoryBack(store);
    return;
  }

  // "Start meeting" flow: double-tap steps back one level toward the
  // menu (same gesture as the history surface). Only intercept the
  // double-tap — single-tap keeps its per-screen meaning (advance the
  // describe flow below; list-row select arrives separately as a
  // listEvent), so we fall through rather than swallowing it.
  if (
    eventType === OsEventTypeList.DOUBLE_CLICK_EVENT &&
    (state.glassesView === "describe_idle" ||
      state.glassesView === "listening" ||
      state.glassesView === "describe_confirm" ||
      state.glassesView === "select_audio_source")
  ) {
    handlePreMeetingBack(store);
    return;
  }

  switch (eventType) {
    case OsEventTypeList.CLICK_EVENT: {
      // Describe-idle: single tap on the body kicks off capture.
      if (state.glassesView === "describe_idle") {
        const next = computeNextGlassesView(state.glassesView, { kind: "begin_describing" }, {});
        store.update({ glassesView: next });
        return;
      }
      // Listening (Describing…): single tap commits the transcript
      // and advances to confirm. Promote whatever's still in the
      // interim slot to the final transcript in the same update —
      // for short captures most of the text is still interim at the
      // moment of commit, and the confirm screen reads only
      // listeningTranscript. VAD silence runs the same promote
      // inside `listening.finish()`.
      if (state.glassesView === "listening") {
        const next = computeNextGlassesView(state.glassesView, { kind: "commit_listening" }, {});
        store.update({
          glassesView: next,
          listeningTranscript: state.listeningTranscript + state.listeningInterim,
          listeningInterim: "",
        });
        return;
      }
      // Quick-asks waiting / answer sub-state: single-tap on the
      // full-screen text container returns to the list. The waiting
      // case is the user's "cancel" path (v1 doesn't kill the LLM
      // task server-side; the response will still appear in chat
      // history). The answer case is "I read it, take me back".
      if (state.glassesView === "active_list" && state.glassesCurrentMode === QUICK_ASKS_MODE) {
        if (state.quickAskWaiting || state.quickAskAnswerText !== null) {
          store.update({
            quickAskWaiting: false,
            quickAskAnswerText: null,
            quickAskDispatchAt: null,
          });
        }
        return;
      }
      // Stop-meeting sentinel (client-only mode at the end of the cycle).
      // Ending a meeting is destructive, so it takes two taps: the first
      // arms a confirmation prompt (no intent sent, meeting keeps
      // running); the second fires `stop_meeting` and resets the mode so
      // the next meeting opens on whatever real mode the server picks. A
      // double-tap (mode cycle) cancels by moving off the sentinel.
      if (state.glassesView === "active_list" && state.glassesCurrentMode === GLASSES_STOP_MODE) {
        if (!state.glassesStopArmed) {
          store.update({ glassesStopArmed: true });
          return;
        }
        send({ type: "stop_meeting" });
        const first = glassesModeIds(state)[0] ?? "transcript";
        store.update({ glassesCurrentMode: first, glassesStopArmed: false });
        return;
      }
      // Active meeting, any real mode → single-tap marks a moment.
      if (state.glassesView === "active_list" && state.meetingState === "active") {
        send({ type: "mark_moment", t: markMomentT(state) });
        // Bump the flash counter so the renderer confirms the capture
        // on the glasses (the send itself is fire-and-forget).
        store.update({ momentMarkedSeq: state.momentMarkedSeq + 1 });
      }
      return;
    }
    case OsEventTypeList.DOUBLE_CLICK_EVENT: {
      // During an active meeting on glasses, double-tap cycles
      // through the server-provided modes plus a trailing Stop
      // sentinel. Real modes go via `set_mode` (server is the
      // source of truth); the stop sentinel is a glasses-only
      // override and never round-trips through the server.
      if (state.glassesView === "active_list" && state.meetingState === "active") {
        const target = nextMode(state);
        // Leaving quick_asks via mode-cycle: clear the sub-state so
        // the next entry shows the list (rather than a stale
        // spinner / answer from a previous pick).
        const leavingQuickAsks =
          state.glassesCurrentMode === QUICK_ASKS_MODE && target !== QUICK_ASKS_MODE;
        if (target === GLASSES_STOP_MODE) {
          // Entering the stop sentinel starts disarmed so a stray tap
          // can't end the meeting without the explicit arm step.
          store.update({
            glassesCurrentMode: GLASSES_STOP_MODE,
            glassesStopArmed: false,
            ...(leavingQuickAsks
              ? { quickAskWaiting: false, quickAskAnswerText: null, quickAskDispatchAt: null }
              : {}),
          });
        } else {
          // Glasses mode is a per-surface concern — the DOM has its
          // own `currentMode` driven by mode-tabs. Don't touch it
          // here, and don't send set_mode (nobody listens; the
          // server's current_mode is vestigial after the
          // decoupling).
          store.update({
            glassesCurrentMode: target,
            // Moving onto a real mode also cancels any primed stop
            // confirmation left on the sentinel.
            glassesStopArmed: false,
            // Entering a (scrollable) mode starts at the tail — snap to
            // the latest rather than inheriting a stale scroll offset.
            glassesActiveListLineOffset: 0,
            ...(leavingQuickAsks
              ? { quickAskWaiting: false, quickAskAnswerText: null, quickAskDispatchAt: null }
              : {}),
          });
        }
        return;
      }
      return;
    }
    case OsEventTypeList.SCROLL_TOP_EVENT:
      // Scroll up = toward older content, for the scrollable live modes
      // (summary/highlights). Bottom-anchored offset, so older = +.
      // In every other mode/view scrollActiveList early-returns, so
      // transcript/chat stay tail-only and history lives on the phone.
      scrollActiveList(store, +SCROLL_STEP);
      return;
    case OsEventTypeList.SCROLL_BOTTOM_EVENT:
      // Scroll down = back toward the newest rows (offset → 0 = tail).
      scrollActiveList(store, -SCROLL_STEP);
      return;
  }
}
