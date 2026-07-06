import type {
  AssistSensitivity,
  Device,
  Event as ServerEvent,
  Intent,
  Item,
  ModeOption,
  PriorContextSummary,
  Status as ServerStatus,
  MeetingState,
} from "./contract";
import type { MeetingSummary } from "./meetings-api";

// Re-export so other PWA modules can `import type { AssistSensitivity } from "./types"`
// instead of reaching into the wire-contract module.
export type { AssistSensitivity };

/// The screen currently shown on the glasses display.
///   - `idle`            entry menu (`> Start meeting / list meetings`)
///   - `describe_idle`   "Describe meeting / Tap to start description!"
///   - `listening`       active description capture (renders as "Describing…")
///   - `describe_confirm` post-capture confirm with transcript preview
///   - `select_audio_source` source picker after the user commits the description
///   - `active_list`     live meeting (transcript / modes / stop sentinel)
///
/// Tag extraction is async and server-driven (kicked by start_meeting
/// whenever description is non-empty). The PWA doesn't wait — tags
/// arrive via `MetadataChanged` whenever the LLM returns and update
/// the active-meeting view in place.
export type GlassesView =
  | "idle"
  | "describe_idle"
  | "listening"
  | "describe_confirm"
  | "select_audio_source"
  | "active_list"
  | "history_list"
  | "history_summary";
export type WsStatus = "connecting" | "open" | "reconnecting" | "closed" | "error";

export interface Settings {
  /** Legacy shared-secret token. Kept on the type for backward
   * compatibility during the OAuth migration; nothing on the wire
   * uses it anymore. The Auth0 access token is fetched live via
   * `auth.getAccessToken()` and never persisted to settings. */
  serverToken: string;
  lastMetadata: Record<string, string>;
  /** Per-mode visibility on the glasses cycle, keyed by `ModeOption.id`.
   * `undefined` (or a missing entry) means "show" — the explicit
   * absence-is-enabled default keeps existing installs unchanged
   * after the field landed. Only `false` hides the mode. Persisted to
   * localStorage via the bridge KV. PWA-only setting; doesn't affect
   * other surfaces. */
  glassesModes: Record<string, boolean>;
}

/** Identity surface for the logged-in user. `null` while we're still
 * resolving the Auth0 session at boot, or when nobody's signed in. */
export interface AuthIdentity {
  email: string | null;
  name: string | null;
  picture: string | null;
  /** Auth0 `sub` (e.g. `google-oauth2|123…`). Stable across logins. */
  sub: string;
}

export interface Toast {
  id: string;
  text: string;
  level: "info" | "warn" | "error";
  /** ms-epoch when the toast auto-dismisses, or `null` for a
   * persistent toast that must be cleared explicitly by whoever
   * pushed it. Used by the audio-capture banner — a 4-second
   * dismissal is too quiet for "your meeting isn't recording." */
  expiresAt: number | null;
}

/// State machine for the glasses → server audio pipeline. Single
/// source of truth for "are my microphone frames actually reaching
/// the server right now?" Driven by `GlassesAudioSource`, consumed
/// by the top-bar pill and the persistent-banner toast.
///
/// Why this exists separately from `wsStatus`: `wsStatus` reflects
/// the *control* WebSocket, which auto-reconnects through a
/// different code path. The /audio WS has its own lifecycle, and
/// the server's view of "audio source bound" survives a WS reset.
/// Without this dedicated field, the UI silently kept showing
/// "Connected" while no audio frames were flowing.
export type AudioCaptureState =
  | { kind: "idle" }
  | { kind: "connecting" }
  | { kind: "streaming"; since: number }
  | { kind: "reconnecting"; attempt: number; since: number }
  | { kind: "failed"; reason: string };

export interface ErrorOverlay {
  title: string;
  message: string;
  dismissable: boolean;
}

/// Resolved summary for the glasses history popup. `title` comes from
/// `pickDetailTitle`, `body` from `formatHistorySummaryBody` (the full
/// summary — the layer paginates it across screen-sized pages).
export interface HistorySummary {
  title: string;
  body: string;
}

export interface AppState {
  settings: Settings;
  wsStatus: WsStatus;
  wsLastEventAt: number | null;
  protocolVersionMatched: boolean;
  meetingState: MeetingState;
  meetingStartedAt: number | null;
  availableModes: ModeOption[];
  /// Mode the DOM (phone-companion-style) view is showing. Driven by
  /// the mode-tabs picker. Independent of `glassesCurrentMode` —
  /// each surface (DOM, glasses) navigates on its own.
  currentMode: string;
  /// Mode the glasses display (EvenHub WebView / simulator's Glasses
  /// Display window) is showing. Driven by the gesture-router's
  /// double-tap cycle. Independent of `currentMode` so that inside
  /// `just pwa-sim` the Browser window and the Glasses Display
  /// window can show different modes despite sharing a single store.
  glassesCurrentMode: string;
  /// Bottom-anchored scroll offset (in display rows) for the live
  /// summary/highlights view: 0 = tail (newest pinned to the bottom),
  /// positive scrolls back toward older items. Only consulted for the
  /// scrollable modes; transcript/chat stay tail-only. Snaps back to 0
  /// whenever the viewed mode's items update, the mode changes, or the
  /// meeting ends ("always show the latest on a change").
  glassesActiveListLineOffset: number;
  /// True once the wearer has tapped the glasses `> Stop` sentinel
  /// once, arming the confirmation prompt. A second tap actually
  /// fires `stop_meeting`; cycling modes (double-tap) or the meeting
  /// ending by any other path disarms it. Client-only — the server
  /// has no concept of this flag.
  glassesStopArmed: boolean;
  /// Monotonic counter bumped each time a moment is marked (glasses
  /// single-tap or phone CTA). Carries no value beyond "it changed" —
  /// the glasses renderer subscribes to it to flash a transient "+1"
  /// marker confirming the capture, then clears it on a timer.
  /// Client-only; the server tracks moments via the meeting detail.
  momentMarkedSeq: number;
  displayTag: string | null;
  metadata: Record<string, string>;
  itemsByMode: Record<string, Item[]>;
  composeDescription: string;
  /// True while the glasses' quick_asks mode is waiting for the
  /// answer to a snippet the user just picked. Locks the view onto
  /// the spinner; a single tap returns to the list (without
  /// cancelling server-side — v1 lets the LLM call finish).
  quickAskWaiting: boolean;
  /// Chat-mode `items` array length at the moment we dispatched the
  /// snippet. Used by the answer detector to ignore older assistant
  /// turns in history and only react to items that landed AFTER our
  /// dispatch. Without this, re-sending the same prompt would lock
  /// onto the previous answer (still sitting at the chat tail until
  /// the new one streams in). `null` outside the waiting window.
  quickAskDispatchAt: number | null;
  /// Latest chat-answer text to render in the quick_asks "answer"
  /// sub-state, or null when nothing's been picked yet (the list
  /// view shows). Cleared on meeting stop.
  quickAskAnswerText: string | null;
  /// Assist-mode item currently popped on the glasses canvas (or
  /// null if no popup is showing). The popup interrupts whatever
  /// view is active; a single click clears it. New assist items
  /// that arrive while a popup is up are queued via
  /// `assistShownIds` and pop in arrival order on subsequent
  /// dismissals.
  assistShown: Item | null;
  /// Ids of assist-mode items already popped (or currently
  /// popping). Used as a dedup ledger so the queue derivation
  /// "first item in itemsByMode.assist whose id is not here" is
  /// idempotent across re-renders and stable across reconnects.
  /// Cleared on meeting stop.
  assistShownIds: string[];
  /// Active meeting's assist sensitivity (or the compose-screen
  /// default when idle). Set from `start_meeting` intent / updated
  /// by `assist_sensitivity_changed` event / read from snapshot
  /// on reconnect. Default `moderate` matches the server.
  assistSensitivity: AssistSensitivity;
  priorContext: PriorContextSummary | null;
  /// All currently-registered devices (Phase 2g UI consumes this).
  availableDevices: Device[];
  /// `device_id` the server assigned to THIS PWA on the most recent
  /// `register_device` round-trip. Null when we haven't registered
  /// (no glasses bridge, or pre-handshake). Used to decide whether
  /// `audio_source_device_changed` means *we* should start streaming.
  ownDeviceId: string | null;
  /// True once `createStartUpPageContainer` succeeded at boot — i.e.
  /// we're running inside a real EvenHub WebView with a working
  /// bridge to glasses (real or simulator). False in plain-browser
  /// prototype mode, where we degrade the UI but can't actually
  /// capture audio. Gates the `audio_capture` registration so the
  /// PWA doesn't show up as a phantom source in the picker.
  glassesBridgeReady: boolean;
  /// Device whose audio is feeding the active meeting; null otherwise.
  audioSourceDeviceId: string | null;
  /// Device the user has picked to feed audio for the *next* meeting.
  /// Distinct from `audioSourceDeviceId` (which reflects what the
  /// server has bound to the *active* meeting). Auto-seeded to the
  /// first online audio-capable device when devices change; user can
  /// override via the compose-region picker. `null` means "no
  /// source" — the server starts a silent meeting.
  composeAudioSourceDeviceId: string | null;
  liveTranscriptInterim: string;
  status: ServerStatus;
  glassesView: GlassesView;
  listeningTranscript: string;
  listeningInterim: string;
  listeningStartedAt: number | null;
  appForegrounded: boolean;
  bleConnected: boolean;
  batteryLevel: number | null;
  wearing: boolean;
  settingsModalOpen: boolean;
  meetingsModalOpen: boolean;
  artifactsModalOpen: boolean;
  quickAsksModalOpen: boolean;
  /// IDs of library artifacts staged for attach during compose.
  /// On meeting start, the WS-event handler drains these to
  /// `attachedArtifactIds` after firing one POST per id.
  pendingArtifactAttachments: string[];
  /// IDs of artifacts attached to the currently-active meeting.
  /// Cleared on idle. The mid-meeting picker reads this to
  /// pre-check rows already in the meeting's set.
  attachedArtifactIds: string[];
  /// IDs of past meetings staged for attach during compose. Same
  /// shape as `pendingArtifactAttachments` — the meeting-start
  /// dispatcher drains these into `attachedMeetingIds` after firing
  /// one POST per id against `/meetings/:id/attached_meetings`.
  pendingAttachedMeetings: string[];
  /// IDs of past meetings attached to the active meeting (from the
  /// server's `AttachedMeetingsChanged` event). Cleared on idle.
  /// The picker reads this to pre-check rows already attached.
  attachedMeetingIds: string[];
  /// Server-assigned id of the active meeting (mirrors Mac's
  /// `currentMeetingId`). `null` when idle. Carried on snapshot
  /// + meeting_state_changed events; used to target attach POSTs.
  currentMeetingId: string | null;
  toasts: Toast[];
  /// State of the glasses → server audio pipeline. Truth-source for
  /// "is the meeting actually recording right now." See AudioCaptureState.
  audioCaptureState: AudioCaptureState;
  errorOverlay: ErrorOverlay | null;
  /// Auth0-resolved identity of the active user. `null` while still
  /// resolving at boot or when signed out — the login screen renders
  /// in that case. Token itself is *never* in the store; it's fetched
  /// on demand from the Auth0 client to avoid leaking it to logs/devtools.
  auth: AuthIdentity | null;
  /// Glasses "List meetings" history surface. The gesture router flips
  /// `glassesView` to history_list/history_summary and sets the loading
  /// flags; the reactor in main.ts fills these in via MeetingsApi.
  /// All cleared back to defaults when the wearer double-taps out.
  glassesHistory: MeetingSummary[]; // newest-first, capped to 20
  glassesHistoryLoading: boolean;
  glassesHistoryError: string | null;
  glassesHistorySelectedId: string | null;
  glassesHistorySummary: HistorySummary | null;
  glassesHistorySummaryLoading: boolean;
  glassesHistorySummaryError: string | null;
  /// Line offset of the summary body's scrolling window — the first body
  /// display row visible beneath the pinned title. Reset to 0 whenever a
  /// summary opens; scroll up/down moves it by `SCROLL_STEP` rows.
  glassesHistorySummaryLineOffset: number;
}

// Re-exported for convenience.
export type { ServerEvent, Intent, Item, ModeOption, ServerStatus, MeetingState };

/// Items the DOM body should render — keyed off the DOM's mode.
export function activeItems(s: AppState): Item[] {
  return s.itemsByMode[s.currentMode] ?? [];
}

/// Items the glasses body should render — keyed off the glasses'
/// own mode. Separate from `activeItems` because DOM and glasses
/// can be on different modes; they share `itemsByMode` but each
/// reads its own slice.
export function activeGlassesItems(s: AppState): Item[] {
  return s.itemsByMode[s.glassesCurrentMode] ?? [];
}

export function defaultAppState(): AppState {
  return {
    settings: { serverToken: "", lastMetadata: {}, glassesModes: {} },
    wsStatus: "closed",
    wsLastEventAt: null,
    protocolVersionMatched: false,
    meetingState: "idle",
    meetingStartedAt: null,
    availableModes: [],
    currentMode: "transcript",
    glassesCurrentMode: "transcript",
    glassesActiveListLineOffset: 0,
    glassesStopArmed: false,
    momentMarkedSeq: 0,
    displayTag: null,
    metadata: {},
    itemsByMode: {},
    composeDescription: "",
    quickAskWaiting: false,
    quickAskDispatchAt: null,
    quickAskAnswerText: null,
    assistShown: null,
    assistShownIds: [],
    assistSensitivity: "moderate",
    priorContext: null,
    availableDevices: [],
    ownDeviceId: null,
    glassesBridgeReady: false,
    audioSourceDeviceId: null,
    composeAudioSourceDeviceId: null,
    liveTranscriptInterim: "",
    status: { listening: false },
    glassesView: "idle",
    listeningTranscript: "",
    listeningInterim: "",
    listeningStartedAt: null,
    appForegrounded: true,
    bleConnected: false,
    batteryLevel: null,
    wearing: false,
    settingsModalOpen: false,
    meetingsModalOpen: false,
    artifactsModalOpen: false,
    quickAsksModalOpen: false,
    pendingArtifactAttachments: [],
    attachedArtifactIds: [],
    pendingAttachedMeetings: [],
    attachedMeetingIds: [],
    currentMeetingId: null,
    toasts: [],
    audioCaptureState: { kind: "idle" },
    errorOverlay: null,
    auth: null,
    glassesHistory: [],
    glassesHistoryLoading: false,
    glassesHistoryError: null,
    glassesHistorySelectedId: null,
    glassesHistorySummary: null,
    glassesHistorySummaryLoading: false,
    glassesHistorySummaryError: null,
    glassesHistorySummaryLineOffset: 0,
  };
}
