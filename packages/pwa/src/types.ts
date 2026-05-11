import type {
  Device,
  Event as ServerEvent,
  Intent,
  Item,
  ModeOption,
  PriorContextSummary,
  Status as ServerStatus,
  MeetingState,
} from "./contract";

export type GlassesView = "idle" | "listening" | "active_list" | "active_detail";
export type WsStatus = "connecting" | "open" | "reconnecting" | "closed" | "error";

export interface Settings {
  /** Legacy shared-secret token. Kept on the type for backward
   * compatibility during the OAuth migration; nothing on the wire
   * uses it anymore. The Auth0 access token is fetched live via
   * `auth.getAccessToken()` and never persisted to settings. */
  serverToken: string;
  lastMetadata: Record<string, string>;
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
  expiresAt: number;
}

export interface ErrorOverlay {
  title: string;
  message: string;
  dismissable: boolean;
}

export interface AppState {
  settings: Settings;
  wsStatus: WsStatus;
  wsLastEventAt: number | null;
  protocolVersionMatched: boolean;
  meetingState: MeetingState;
  meetingStartedAt: number | null;
  availableModes: ModeOption[];
  currentMode: string;
  displayTag: string | null;
  metadata: Record<string, string>;
  itemsByMode: Record<string, Item[]>;
  composeDescription: string;
  extractingMetadata: boolean;
  priorContext: PriorContextSummary | null;
  /// All currently-registered devices (Phase 2g UI consumes this).
  availableDevices: Device[];
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
  highlightIndex: number;
  viewportStart: number;
  detailItemId: string | null;
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
  errorOverlay: ErrorOverlay | null;
  /// Auth0-resolved identity of the active user. `null` while still
  /// resolving at boot or when signed out — the login screen renders
  /// in that case. Token itself is *never* in the store; it's fetched
  /// on demand from the Auth0 client to avoid leaking it to logs/devtools.
  auth: AuthIdentity | null;
}

// Re-exported for convenience.
export type { ServerEvent, Intent, Item, ModeOption, ServerStatus, MeetingState };

export function activeItems(s: AppState): Item[] {
  return s.itemsByMode[s.currentMode] ?? [];
}

export function defaultAppState(): AppState {
  return {
    settings: { serverToken: "", lastMetadata: {} },
    wsStatus: "closed",
    wsLastEventAt: null,
    protocolVersionMatched: false,
    meetingState: "idle",
    meetingStartedAt: null,
    availableModes: [],
    currentMode: "transcript",
    displayTag: null,
    metadata: {},
    itemsByMode: {},
    composeDescription: "",
    extractingMetadata: false,
    priorContext: null,
    availableDevices: [],
    audioSourceDeviceId: null,
    composeAudioSourceDeviceId: null,
    liveTranscriptInterim: "",
    status: { listening: false, paused: false },
    glassesView: "idle",
    highlightIndex: 0,
    viewportStart: 0,
    detailItemId: null,
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
    pendingArtifactAttachments: [],
    attachedArtifactIds: [],
    pendingAttachedMeetings: [],
    attachedMeetingIds: [],
    currentMeetingId: null,
    toasts: [],
    errorOverlay: null,
    auth: null,
  };
}
