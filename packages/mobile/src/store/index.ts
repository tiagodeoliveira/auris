// App-wide store. Mirrors the PWA's `defaultAppState` shape — same
// slice names so cross-client code review is symmetrical — but uses
// Zustand instead of the PWA's hand-rolled subscribe-based store.
//
// The store owns the WebSocket lifecycle: it constructs the
// ReconnectingSocket on `connect()`, dispatches inbound events into
// state, and tears down on `disconnect()`. UI components read state
// via `useAppStore(selector)` and dispatch intents via methods on
// the store.

import { create } from "zustand";
import { serverUrl } from "../config";
import * as auth0 from "../auth/auth0";
import type { Identity } from "../auth/auth0";
import type {
  Device,
  Event as ServerEvent,
  Intent,
  Item,
  MeetingState,
  ModeOption,
  PriorContextSummary,
  Status,
} from "../wire/contract";
import { MeetingsApi } from "../wire/meetings-api";
import { ReconnectingSocket, type WsStatus } from "../wire/ws";

interface AppState {
  // ───── Auth ─────────────────────────────────────────────────────
  identity: Identity | null;
  /// `true` after the first `bootstrap()` call resolves. UI gates
  /// rendering on this so we don't flash the login screen for users
  /// who already have a refresh token persisted.
  authBootstrapped: boolean;

  // ───── Connection ───────────────────────────────────────────────
  wsStatus: WsStatus | "idle";

  // ───── Meeting (per PWA's defaultAppState) ──────────────────────
  meetingState: MeetingState;
  currentMeetingId: string | null;
  availableModes: ModeOption[];
  currentMode: string;
  displayTag: string | null;
  itemsByMode: Record<string, Item[]>;
  liveTranscriptInterim: string;
  metadata: Record<string, string>;
  attachedArtifactIds: string[];
  /// Past-meeting IDs staged for attach during compose. Drained
  /// once the meeting transitions to `active` and we have a
  /// `currentMeetingId` to attach against.
  pendingAttachedMeetings: string[];
  /// Past-meeting IDs attached to the active meeting (server-
  /// authoritative via `attached_meetings_changed`). Cleared on
  /// idle. The mid-meeting picker pre-checks rows against this.
  attachedMeetingIds: string[];
  /// Setter the compose surface calls before sending `start_meeting`.
  setPendingAttachedMeetings: (ids: string[]) => void;
  status: Status;
  priorContext: PriorContextSummary | null;
  devices: Device[];
  audioSourceDeviceId: string | null;

  // ───── Imperative actions ──────────────────────────────────────
  /// Bootstrap auth from secure storage. Call once at app launch;
  /// idempotent thereafter.
  bootstrap: () => Promise<void>;
  signIn: () => Promise<void>;
  signOut: () => Promise<void>;
  /// Open the WS connection using the cached token. No-op if
  /// already connected.
  connect: () => void;
  /// Tear down the WS without clearing auth state.
  disconnect: () => void;
  /// Send an Intent over the WS. Buffers if not yet connected.
  send: (intent: Intent) => void;
}

let socket: ReconnectingSocket | null = null;

export const useAppStore = create<AppState>((set, get) => ({
  identity: null,
  authBootstrapped: false,

  wsStatus: "idle",

  meetingState: "idle",
  currentMeetingId: null,
  availableModes: [],
  currentMode: "transcript",
  displayTag: null,
  itemsByMode: {},
  liveTranscriptInterim: "",
  metadata: {},
  attachedArtifactIds: [],
  pendingAttachedMeetings: [],
  attachedMeetingIds: [],
  setPendingAttachedMeetings: (ids: string[]) => set({ pendingAttachedMeetings: ids }),
  status: { listening: false, paused: false },
  priorContext: null,
  devices: [],
  audioSourceDeviceId: null,

  bootstrap: async () => {
    auth0.subscribe((id) => set({ identity: id }));
    const id = await auth0.bootstrap();
    set({ identity: id, authBootstrapped: true });
  },

  signIn: async () => {
    const id = await auth0.signIn();
    set({ identity: id });
  },

  signOut: async () => {
    await auth0.signOut();
    get().disconnect();
  },

  connect: () => {
    if (socket) return;
    socket = new ReconnectingSocket({
      url: serverUrl,
      tokenProvider: () => auth0.getAccessToken(),
      onStatus: (status) => set({ wsStatus: status }),
      onEvent: (event) => handleEvent(event, set, get),
    });
  },

  disconnect: () => {
    if (!socket) return;
    socket.close();
    socket = null;
    set({ wsStatus: "idle" });
  },

  send: (intent) => {
    if (!socket) {
      console.warn("[store] send called without an active socket", intent);
      return;
    }
    socket.send(intent);
  },
}));

/// Server-event reducer. Each case maps a wire event to a partial
/// state update. Unknown events are ignored (forward-compat with
/// future server additions).
function handleEvent(
  event: ServerEvent,
  set: (partial: Partial<AppState>) => void,
  get: () => AppState,
): void {
  switch (event.type) {
    case "snapshot": {
      const itemsByMode: Record<string, Item[]> = {};
      itemsByMode[event.mode] = event.items;
      set({
        meetingState: event.meeting_state,
        currentMeetingId: event.meeting_id ?? null,
        availableModes: event.available_modes,
        currentMode: event.mode,
        displayTag: event.display_tag ?? null,
        metadata: event.metadata,
        itemsByMode,
        status: event.status,
        priorContext: event.prior_context ?? null,
        devices: event.devices,
        audioSourceDeviceId: event.audio_source_device_id ?? null,
        attachedMeetingIds: event.attached_meeting_ids ?? [],
      });
      return;
    }
    case "meeting_state_changed": {
      const partial: Partial<AppState> = {
        meetingState: event.meeting_state,
        currentMeetingId: event.meeting_id ?? null,
      };
      if (event.meeting_state === "idle") {
        partial.pendingAttachedMeetings = [];
        partial.attachedMeetingIds = [];
      }
      set(partial);
      // Drain compose-time staged meeting attachments. Best-effort
      // POSTs — server is idempotent so a re-fire is safe. We
      // atomically clear the queue here (rather than inside the
      // async) so transient state churn doesn't double-attach.
      if (
        event.meeting_state === "active" &&
        event.meeting_id &&
        get().pendingAttachedMeetings.length > 0
      ) {
        const ids = get().pendingAttachedMeetings;
        const parentId = event.meeting_id;
        set({ pendingAttachedMeetings: [] });
        void (async () => {
          const api = MeetingsApi.from(serverUrl, () => auth0.getAccessToken());
          if (!api) return;
          for (const mid of ids) {
            try {
              await api.attach(parentId, mid);
              set({
                attachedMeetingIds: [...get().attachedMeetingIds.filter((x) => x !== mid), mid],
              });
            } catch (e) {
              console.warn(`[store] attach meeting ${mid} failed:`, e);
            }
          }
        })();
      }
      return;
    }
    case "attached_meetings_changed":
      set({ attachedMeetingIds: event.meeting_ids });
      return;
    case "mode_changed":
      set({
        currentMode: event.mode,
        displayTag: event.display_tag ?? null,
        itemsByMode: { ...get().itemsByMode, [event.mode]: event.items },
      });
      return;
    case "items_update":
      set({ itemsByMode: { ...get().itemsByMode, [event.mode]: event.items } });
      return;
    case "item_updated": {
      const existing = get().itemsByMode[event.mode] ?? [];
      const idx = existing.findIndex((it) => it.id === event.item.id);
      if (idx === -1) return;
      const next = existing.slice();
      next[idx] = event.item;
      set({ itemsByMode: { ...get().itemsByMode, [event.mode]: next } });
      return;
    }
    case "metadata_changed":
      set({ metadata: event.metadata });
      return;
    case "transcript_interim":
      set({ liveTranscriptInterim: event.text });
      return;
    case "status":
      set({ status: event.status });
      return;
    case "prior_context_changed":
      set({ priorContext: event.summary });
      return;
    case "devices_changed":
      set({ devices: event.devices });
      return;
    case "audio_source_device_changed":
      set({ audioSourceDeviceId: event.device_id ?? null });
      return;
    case "display_tag_changed":
      set({ displayTag: event.tag ?? null });
      return;
    case "error":
      console.warn(`[store] server error: ${event.code} — ${event.message}`);
      return;
    // device_registered, artifacts_changed: not needed for Phase 1
    // demo state. Wire when their consumers land in later phases.
    default:
      return;
  }
}
