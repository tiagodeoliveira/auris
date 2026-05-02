import type {
  Event as ServerEvent,
  Intent,
  Item,
  ModeOption,
  Status as ServerStatus,
  MeetingState,
} from "./contract";

export type GlassesView = "idle" | "listening" | "active_list" | "active_detail";
export type WsStatus = "connecting" | "open" | "reconnecting" | "closed" | "error";

export interface Settings {
  serverUrl: string;
  serverToken: string;
  sonioxKey: string;
  lastMetadata: Record<string, string>;
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
  availableModes: ModeOption[];
  currentMode: string;
  displayTag: string | null;
  metadata: Record<string, string>;
  items: Item[];
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
  toasts: Toast[];
  errorOverlay: ErrorOverlay | null;
}

// Re-exported for convenience.
export type { ServerEvent, Intent, Item, ModeOption, ServerStatus, MeetingState };
