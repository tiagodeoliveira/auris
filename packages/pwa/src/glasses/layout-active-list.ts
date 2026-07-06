import {
  TextContainerProperty,
  RebuildPageContainer,
  TextContainerUpgrade,
} from "@evenrealities/even_hub_sdk";
import type { AppState } from "../types";
import { activeGlassesItems } from "../types";
import {
  ACTIVE_LIST_VISIBLE_LINES,
  formatActiveListBody,
  formatActiveListWindow,
} from "./format-active-list";
import { formatActiveChatBody } from "./format-active-chat";

const HEADER_ID = 1;
const BODY_ID = 2;
const HEADER_NAME = "header";
const BODY_NAME = "body";

/// Tiny top-right "audio is flowing" indicator. Mirrors the activity
/// glyph the native Converse app shows while it's listening + STT is
/// producing tokens. Sits on top of the header strip's empty
/// right-edge zone — declaration order is z-order in the SDK, so
/// declaring it after the header puts it on top.
export const ACTIVITY_ID = 3;
export const ACTIVITY_NAME = "act";

/// Transient "moment captured" marker, sitting just left of the
/// activity indicator. Flashed for `MOMENT_FLASH_MS` after a moment is
/// marked (glasses single-tap or phone CTA), then cleared back to a
/// blank. Only the standard active-list layout carries it — moments
/// are never marked from the Quick Asks surface.
export const MOMENT_ID = 4;
export const MOMENT_NAME = "mmt";
/// ASCII-safe (same firmware-font constraint as `ACTIVITY_FRAMES`).
const MOMENT_FLASH = "+1";
/// Single space (not empty) so the upgrade actually overwrites the
/// firmware buffer when clearing — same trick the activity idle uses.
const MOMENT_IDLE = " ";
export const MOMENT_FLASH_MS = 1500;

/// Three-frame animation, ASCII-only (firmware font has no Unicode
/// drawing glyphs reliably — we hit this with the `⌁` issue earlier).
/// Looks like sound waves emanating outward.
const ACTIVITY_FRAMES = ["·", "··", "···"];
export const ACTIVITY_FRAME_INTERVAL_MS = 400;

export function activityFrame(index: number): string {
  return ACTIVITY_FRAMES[index % ACTIVITY_FRAMES.length] ?? ACTIVITY_FRAMES[0];
}

/// Blank-but-not-empty when the server reports no audio is flowing.
/// A literal empty string can fail to overwrite the firmware buffer
/// (we hit this in the stop-sentinel work) — single space forces a
/// clear.
const ACTIVITY_IDLE = " ";

/// Shown in place of the animated frames when THIS client's audio
/// WebSocket has stalled during an active meeting. The indicator is
/// the only status surface on the Quick Asks layout (no header), so
/// it has to carry the alarm itself — a stopped, distinct ASCII glyph
/// the wearer can catch and act on before content is lost.
const ACTIVITY_WARNING = "!!";

/// True when THIS client's `/audio` WebSocket is not actively
/// streaming during a live meeting — i.e. capture has stalled and
/// audio may be getting dropped. `idle` is deliberately excluded: a
/// meeting with no source bound to this client (another device is
/// capturing, or it's a silent meeting) is a valid state, not a
/// fault. Single source of truth for both the header's text banner
/// and the activity indicator's warning glyph.
export function audioStalled(state: AppState): boolean {
  if (state.meetingState !== "active") return false;
  const kind = state.audioCaptureState.kind;
  return kind === "connecting" || kind === "reconnecting" || kind === "failed";
}

/// Resting (non-animated) content for the activity indicator. The
/// renderer animates the frames while audio is genuinely flowing;
/// otherwise it pushes this — a warning glyph when capture has
/// stalled mid-meeting, an animation frame as the seed when flowing,
/// or a blank when nothing is being captured.
export function activityIndicatorRestingContent(state: AppState): string {
  if (audioStalled(state)) return ACTIVITY_WARNING;
  return state.status.listening ? activityFrame(0) : ACTIVITY_IDLE;
}

/// Client-only sentinel slotted at the end of the mode-cycle on
/// glasses. When `state.currentMode === GLASSES_STOP_MODE`, the
/// active-list body shows a stop-confirmation rather than the
/// current mode's items. Never sent to the server (the server has
/// no concept of this mode); cleared the moment the user picks a
/// real mode or stops the meeting.
export const GLASSES_STOP_MODE = "__stop__";

// `CHARS_PER_LINE` is used by the chat formatter (which pre-wraps
// each turn itself). The plain active list is sized by visible-line
// budget instead — see `ACTIVE_LIST_VISIBLE_LINES`.
export const CHARS_PER_LINE = 80;

export function buildActiveListLayout(state: AppState) {
  const header = new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 32,
    borderWidth: 0,
    paddingLength: 4,
    containerID: HEADER_ID,
    containerName: HEADER_NAME,
    content: buildHeader(state),
    isEventCapture: 0,
  });

  const body = new TextContainerProperty({
    xPosition: 0,
    yPosition: 32,
    width: 576,
    height: 256,
    borderWidth: 0,
    paddingLength: 4,
    containerID: BODY_ID,
    containerName: BODY_NAME,
    content: buildBody(state),
    isEventCapture: 1,
  });

  // Activity indicator (top-right, ~36px wide). Overlays the empty
  // right-edge of the header zone. Initial content is whatever the
  // current `status.listening` says; the renderer animates frames
  // via `textContainerUpgrade` while listening stays true.
  return new RebuildPageContainer({
    containerTotalNum: 4,
    textObject: [header, body, activityIndicator(state), momentIndicator()],
  });
}

/// The "+1" moment marker container. Starts blank on every rebuild
/// (the flash is transient); the renderer pushes the marker via
/// `buildMomentUpgrade` on a moment-marked edge and clears it on a
/// timer. Positioned just left of the activity indicator (x=536) with
/// a small gap so the two never overlap.
export function momentIndicator(): TextContainerProperty {
  return new TextContainerProperty({
    xPosition: 492,
    yPosition: 4,
    width: 40,
    height: 24,
    borderWidth: 0,
    paddingLength: 0,
    containerID: MOMENT_ID,
    containerName: MOMENT_NAME,
    content: MOMENT_IDLE,
    isEventCapture: 0,
  });
}

export function buildMomentUpgrade(flash: boolean) {
  const content = flash ? MOMENT_FLASH : MOMENT_IDLE;
  return new TextContainerUpgrade({
    containerID: MOMENT_ID,
    containerName: MOMENT_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

/// The top-right "audio is flowing" indicator container. Shared by
/// the standard active-list layout AND the quick_asks layout so both
/// surfaces show the same recording status with a single source of
/// truth for geometry + initial frame. Container id/name are stable
/// (`ACTIVITY_ID`/`ACTIVITY_NAME`) so `buildActivityUpgrade` animates
/// it regardless of which layout mounted it.
export function activityIndicator(state: AppState): TextContainerProperty {
  return new TextContainerProperty({
    xPosition: 536,
    yPosition: 4,
    width: 36,
    height: 24,
    borderWidth: 0,
    paddingLength: 0,
    containerID: ACTIVITY_ID,
    containerName: ACTIVITY_NAME,
    content: activityIndicatorRestingContent(state),
    isEventCapture: 0,
  });
}

export function buildActivityUpgrade(content: string) {
  return new TextContainerUpgrade({
    containerID: ACTIVITY_ID,
    containerName: ACTIVITY_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

export function buildActivityIdleUpgrade() {
  return buildActivityUpgrade(ACTIVITY_IDLE);
}

export function buildBodyUpgrade(state: AppState) {
  const content = buildBody(state);
  return new TextContainerUpgrade({
    containerID: BODY_ID,
    containerName: BODY_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

export function buildHeaderUpgrade(state: AppState) {
  const content = buildHeader(state);
  return new TextContainerUpgrade({
    containerID: HEADER_ID,
    containerName: HEADER_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

function buildHeader(state: AppState): string {
  // Stop sentinel: blank the header so the view is unobstructed.
  // A single space (not empty string) is required to overwrite the
  // firmware buffer — see format-active-list for the same trick.
  if (state.glassesCurrentMode === GLASSES_STOP_MODE) {
    return " ";
  }
  const mode = state.availableModes.find((m) => m.id === state.glassesCurrentMode);
  const label = mode?.label ?? state.glassesCurrentMode;
  const tag = state.displayTag ? `  ${state.displayTag}` : "";
  // Audio-loss banner: when a meeting is live but mic frames aren't
  // actually reaching the server, prepend a hard-to-miss ASCII marker
  // (firmware font has no reliable Unicode glyphs — see ACTIVITY_FRAMES
  // note). This is the glasses-side counterpart to the phone's top-bar
  // pill + persistent toast; without it the loss is invisible on the
  // display the wearer is actually looking at.
  return `${audioWarningPrefix(state)}> ${label}${tag}`;
}

/// Returns a leading "!! NO AUDIO  " marker when recording has stalled
/// during an active meeting, else "". `idle` is intentionally excluded
/// — a silent meeting (no source bound) is a valid state, not a fault.
function audioWarningPrefix(state: AppState): string {
  if (!audioStalled(state)) return "";
  return state.audioCaptureState.kind === "failed" ? "!! AUDIO LOST  " : "!! NO AUDIO  ";
}

/// Pads "> Stop" with leading newlines + spaces so it lands in the
/// bottom-right corner of the 256px-tall body container. The
/// firmware has no alignment APIs (text is always top-left), so we
/// position with whitespace. Numbers tuned to the visible-line
/// budget — same ~56 chars/line as ACTIVE_LIST_VISIBLE_LINES.
const STOP_TEXT_LINE_PADDING = 7;
const STOP_TEXT_LEFT_PADDING = 50;
/// Column the stop affordance right-aligns to (matches the resting
/// "> Stop" at LEFT_PADDING + label width). Reused to right-align the
/// armed confirm prompt so both states sit in the same corner.
const STOP_TEXT_RIGHT_COL = STOP_TEXT_LEFT_PADDING + "> Stop".length;

/// Bottom-right stop affordance. Disarmed: the minimal "> Stop".
/// Armed (after one tap): a confirm prompt — a second single-tap
/// ends the meeting, a double-tap (mode cycle) cancels.
function stopBody(armed: boolean): string {
  if (!armed) {
    return "\n".repeat(STOP_TEXT_LINE_PADDING) + " ".repeat(STOP_TEXT_LEFT_PADDING) + "> Stop";
  }
  const line1 = "> Tap again to end";
  const line2 = "double-tap to cancel";
  const rightAlign = (s: string) => " ".repeat(Math.max(0, STOP_TEXT_RIGHT_COL - s.length)) + s;
  return "\n".repeat(STOP_TEXT_LINE_PADDING - 1) + rightAlign(line1) + "\n" + rightAlign(line2);
}

function buildBody(state: AppState): string {
  // Glasses-only stop view — minimal "> Stop" in the bottom-right
  // corner for an unobstructed view. First single-tap arms a confirm
  // prompt; a second tap fires `stop_meeting` (gesture router);
  // double-tap cycles back to a real mode (cancel).
  if (state.glassesCurrentMode === GLASSES_STOP_MODE) {
    return stopBody(state.glassesStopArmed);
  }
  // Chat is a flowing thread, not a bullet list — single-line
  // truncation drops most of an assistant answer. The chat
  // formatter wraps each turn across multiple lines and pins the
  // latest exchange to the bottom of the visible window.
  if (state.glassesCurrentMode === "chat") {
    return formatActiveChatBody(
      activeGlassesItems(state),
      CHARS_PER_LINE,
      ACTIVE_LIST_VISIBLE_LINES,
    );
  }
  // Summary and highlights are digestible LLM-derived lists the wearer
  // may want to read back through, so they get a scrollable window
  // instead of the tail. The bottom-anchored offset keeps the newest
  // pinned at offset 0 (matching the tail) and pages back as it grows.
  // Transcript stays tail-only — it's live speech, newest-at-bottom is
  // the right model and it has the phone for history.
  if (state.glassesCurrentMode === "summary" || state.glassesCurrentMode === "highlights") {
    return formatActiveListWindow(
      activeGlassesItems(state),
      ACTIVE_LIST_VISIBLE_LINES,
      state.glassesActiveListLineOffset,
    );
  }
  // Interim text only makes sense in the live-transcript view —
  // other modes (highlights, actions, summary) render LLM-derived
  // content that isn't tied to the in-flight STT segment.
  const interim =
    state.glassesCurrentMode === "transcript" ? state.liveTranscriptInterim : undefined;
  return formatActiveListBody(activeGlassesItems(state), ACTIVE_LIST_VISIBLE_LINES, interim);
}
