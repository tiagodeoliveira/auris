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
import { meetingMenu } from "./meeting-menu";

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
    menuObject: meetingMenu(state),
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

function buildBody(state: AppState): string {
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
