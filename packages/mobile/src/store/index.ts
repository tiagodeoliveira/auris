// App-wide store. Mirrors the PWA's `defaultAppState` shape — same
// slice names so cross-client code review is symmetrical — but uses
// Zustand instead of the PWA's hand-rolled subscribe-based store.
//
// The store owns the WebSocket lifecycle: it constructs the
// ReconnectingSocket on `connect()`, dispatches inbound events into
// state, and tears down on `disconnect()`. UI components read state
// via `useAppStore(selector)` and dispatch intents via methods on
// the store.

import AsyncStorage from "@react-native-async-storage/async-storage";
import { create } from "zustand";
import { serverUrl } from "../config";
import * as auth0 from "../auth/auth0";
import type { Identity } from "../auth/auth0";
import type {
  AssistSensitivity,
  Device,
  Event as ServerEvent,
  Intent,
  Item,
  MeetingState,
  ModeOption,
  PriorContextSummary,
  Status,
} from "../wire/contract";
import { applyItemsUpdate } from "../wire/apply-items-update";
import { ArtifactsApi } from "../wire/artifacts-api";
import { MeetingsApi } from "../wire/meetings-api";
import { ReconnectingSocket, type WsStatus } from "../wire/ws";

/// Persistence key for the user's appearance preference. Lives under
/// `auris.*` to keep all app-owned keys namespaced (mirrors the auth
/// key convention in `auth0.ts`).
const THEME_OVERRIDE_KEY = "auris.themeOverride";

export type ThemeOverride = "system" | "light" | "dark";

function isThemeOverride(v: string | null): v is ThemeOverride {
  return v === "system" || v === "light" || v === "dark";
}

interface AppState {
  // ───── Auth ─────────────────────────────────────────────────────
  identity: Identity | null;
  /// `true` after the first `bootstrap()` call resolves. UI gates
  /// rendering on this so we don't flash the login screen for users
  /// who already have a refresh token persisted.
  authBootstrapped: boolean;

  // ───── Appearance ───────────────────────────────────────────────
  /// User's appearance preference. `"system"` defers to OS; `"light"`
  /// or `"dark"` force a fixed scheme regardless of OS setting.
  /// Persisted to AsyncStorage so the choice survives cold starts.
  /// `useTheme()` reads this slice and short-circuits before falling
  /// back to `useColorScheme()`.
  themeOverride: ThemeOverride;
  /// Setter wired to the Settings → Appearance picker. Writes through
  /// to AsyncStorage in the background; the in-memory update is
  /// synchronous so the UI flips on the next render.
  setThemeOverride: (mode: ThemeOverride) => void;

  // ───── Connection ───────────────────────────────────────────────
  wsStatus: WsStatus | "idle";

  // ───── Meeting (per PWA's defaultAppState) ──────────────────────
  meetingState: MeetingState;
  currentMeetingId: string | null;
  /// Wall-clock ms when THIS client observed the meeting go active
  /// (meeting_state_changed → active). Used to compute
  /// `mark_moment.t` — the ms offset from meeting start. `null`
  /// when idle, or when this client joined an already-running
  /// meeting via snapshot; in that case mark_moment sends the
  /// t==0 sentinel and the server computes the offset from its
  /// own meeting clock (mirrors the PWA's meetingStartedAt slice,
  /// minus the snapshot-time Date.now() approximation — faking a
  /// start time would be strictly worse than the server's answer).
  meetingStartedAt: number | null;
  availableModes: ModeOption[];
  currentMode: string;
  /// Local setter for the mode-tab picker. `currentMode` is
  /// per-surface UI state — purely local; we don't fire
  /// `set_mode` to the server anymore (the intent is a legacy
  /// no-op now).
  setCurrentMode: (id: string) => void;
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
  /// Per-meeting assist surface sensitivity. Local default `moderate`
  /// (matches server). Snapshot + `assist_sensitivity_changed`
  /// events update this. Carried into the `start_meeting` intent;
  /// `setAssistSensitivity` fires `set_assist_sensitivity` mid-
  /// meeting and updates locally only when idle.
  assistSensitivity: AssistSensitivity;
  setAssistSensitivity: (value: AssistSensitivity) => void;
  /// Artifact IDs staged for attach during compose. Mirrors the
  /// PWA's `pendingArtifactAttachments` slice. Drained once the
  /// meeting transitions to `active` and we have a `currentMeetingId`
  /// to attach against.
  pendingArtifactAttachments: string[];
  setPendingArtifactAttachments: (ids: string[]) => void;
  status: Status;
  /// Last pair code received via `pair_code_minted`. Pair sheet
  /// reads this to populate the display. Cleared by the sheet on
  /// mount before sending a fresh `mint_pair_code` intent so a
  /// stale code from a prior session never flashes on screen.
  pairCode: { code: string; expires_at: string } | null;
  setPairCode: (v: { code: string; expires_at: string } | null) => void;
  /// Monotonic counter that ticks every time the server fires
  /// `paired_devices_changed`. Components subscribe via a selector
  /// and re-fetch `/pair/devices` whenever this increments. Counter
  /// instead of a boolean so two events in a row both fire.
  pairedDevicesSeq: number;
  priorContext: PriorContextSummary | null;
  devices: Device[];
  audioSourceDeviceId: string | null;
  /// Setter the compose surface calls when the user picks an audio
  /// source. Mirrors the PWA's `composeAudioSourceDeviceId` write
  /// path. Distinct from server-driven updates which arrive via
  /// `audio_source_device_changed`.
  setAudioSourceDeviceId: (id: string | null) => void;

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

  // Default to system; the real persisted value is loaded inside
  // `bootstrap()` and overwrites this initial seed before the first
  // tab renders.
  themeOverride: "system",
  setThemeOverride: (mode) => {
    set({ themeOverride: mode });
    // Fire-and-forget persist. A failed write just means the choice
    // doesn't survive a cold start — not worth surfacing to the UI.
    void AsyncStorage.setItem(THEME_OVERRIDE_KEY, mode).catch((e: unknown) => {
      console.warn("[store] persist themeOverride failed:", e);
    });
  },

  wsStatus: "idle",

  meetingState: "idle",
  currentMeetingId: null,
  meetingStartedAt: null,
  availableModes: [],
  currentMode: "transcript",
  setCurrentMode: (id: string) => set({ currentMode: id }),
  displayTag: null,
  itemsByMode: {},
  liveTranscriptInterim: "",
  metadata: {},
  attachedArtifactIds: [],
  pendingAttachedMeetings: [],
  attachedMeetingIds: [],
  setPendingAttachedMeetings: (ids: string[]) => set({ pendingAttachedMeetings: ids }),
  assistSensitivity: "moderate",
  setAssistSensitivity: (value: AssistSensitivity) => {
    const state = get();
    // Always update locally so the compose-screen picker reflects
    // the choice immediately. While a meeting is active, also fire
    // the intent so the server flips its runtime field + broadcasts
    // to other surfaces.
    set({ assistSensitivity: value });
    if (state.meetingState === "active" && state.send) {
      state.send({ type: "set_assist_sensitivity", value });
    }
  },
  pendingArtifactAttachments: [],
  setPendingArtifactAttachments: (ids: string[]) => set({ pendingArtifactAttachments: ids }),
  status: { listening: false },
  pairCode: null,
  setPairCode: (v) => set({ pairCode: v }),
  pairedDevicesSeq: 0,
  priorContext: null,
  devices: [],
  audioSourceDeviceId: null,
  setAudioSourceDeviceId: (id: string | null) => set({ audioSourceDeviceId: id }),

  bootstrap: async () => {
    // Hydrate themeOverride from disk *in parallel* with the auth
    // bootstrap so neither blocks the other. Auth gates rendering;
    // the theme value only affects which token set the first paint
    // uses — if the read races past the first render the UI just
    // re-renders with the persisted scheme a frame later.
    const themePromise = AsyncStorage.getItem(THEME_OVERRIDE_KEY)
      .then((v: string | null) => (isThemeOverride(v) ? v : null))
      .catch((e: unknown) => {
        console.warn("[store] read themeOverride failed:", e);
        return null;
      });

    auth0.subscribe((id) => set({ identity: id }));
    const id = await auth0.bootstrap();
    const persistedTheme = await themePromise;
    set({
      identity: id,
      authBootstrapped: true,
      ...(persistedTheme ? { themeOverride: persistedTheme } : null),
    });
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
      // MERGE the snapshot's mode items into the existing
      // itemsByMode rather than replacing the whole bag. The
      // server's snapshot only ships items for ONE mode
      // (current_mode); the other modes' items only arrive via
      // live items_update events. If we replaced wholesale here,
      // every reconnect (network blip, app backgrounded then
      // foregrounded, etc.) would wipe every tab the user wasn't
      // already on — chat history vanishes, summary vanishes,
      // highlights vanish, even though items_update events
      // already delivered them once.
      //
      // This was the root cause of "mobile shows empty tabs even
      // though PWA on the same meeting has everything" — iOS
      // tends to suspend WS connections in background, the
      // resume reconnects and the snapshot stomped everything.
      // Mirrors the PWA's snapshot merge (ws-handlers.ts).
      const itemsByMode: Record<string, Item[]> = {
        ...get().itemsByMode,
        [event.mode]: event.items,
      };
      // `currentMode` is per-surface UI state — we do NOT inherit
      // it from the server's snapshot. Mobile tracks its own mode
      // locally (defaults to `transcript` via the store init);
      // the snapshot just delivers items under their canonical
      // mode keys.
      set({
        meetingState: event.meeting_state,
        currentMeetingId: event.meeting_id ?? null,
        // Keep a known start time across reconnects; clear a stale
        // one when the snapshot says idle. Never fabricate one here —
        // null means "let the server compute mark_moment offsets".
        meetingStartedAt: event.meeting_state === "active" ? get().meetingStartedAt : null,
        availableModes: event.available_modes,
        displayTag: event.display_tag ?? null,
        metadata: event.metadata,
        itemsByMode,
        status: event.status,
        priorContext: event.prior_context ?? null,
        devices: event.devices,
        audioSourceDeviceId: event.audio_source_device_id ?? null,
        attachedMeetingIds: event.attached_meeting_ids ?? [],
        assistSensitivity: event.assist_sensitivity ?? "moderate",
      });
      return;
    }
    case "assist_sensitivity_changed": {
      // Mirror the canonical server value. Idempotent — no-op when
      // it matches what we already have.
      if (get().assistSensitivity !== event.value) {
        set({ assistSensitivity: event.value });
      }
      return;
    }
    case "meeting_state_changed": {
      const partial: Partial<AppState> = {
        meetingState: event.meeting_state,
        currentMeetingId: event.meeting_id ?? null,
      };
      // Stamp the meeting start on the idle→active edge (don't
      // overwrite if a snapshot already preserved one).
      if (event.meeting_state === "active" && !get().meetingStartedAt) {
        partial.meetingStartedAt = Date.now();
      }
      if (event.meeting_state === "idle") {
        partial.pendingAttachedMeetings = [];
        partial.attachedMeetingIds = [];
        partial.pendingArtifactAttachments = [];
        partial.audioSourceDeviceId = null;
        partial.meetingStartedAt = null;
        // Sensitivity is per-meeting; reset on idle so the next
        // compose surface opens on Moderate (matching the server).
        partial.assistSensitivity = "moderate";
        // Now that the snapshot handler MERGES itemsByMode instead
        // of wiping it, we have to explicitly clear meeting-scoped
        // items on idle — otherwise the next meeting starts with
        // stale highlights / chat / summary from the previous one
        // until items_update events overwrite. quick_asks is the
        // user's library (per-user, NOT per-meeting) so it
        // survives. Same shape as PWA's ws-handlers.ts.
        const preservedQuickAsks = get().itemsByMode.quick_asks ?? [];
        partial.itemsByMode = preservedQuickAsks.length ? { quick_asks: preservedQuickAsks } : {};
        partial.liveTranscriptInterim = "";
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
      // Drain compose-time staged artifact attachments. Same pattern
      // as the meeting drain above: atomically clear the queue
      // before kicking the async POSTs so transient state churn
      // can't double-attach. Server is idempotent so a re-fire is
      // safe regardless.
      if (
        event.meeting_state === "active" &&
        event.meeting_id &&
        get().pendingArtifactAttachments.length > 0
      ) {
        const ids = get().pendingArtifactAttachments;
        const meetingId = event.meeting_id;
        set({ pendingArtifactAttachments: [] });
        void (async () => {
          const api = ArtifactsApi.from(serverUrl, () => auth0.getAccessToken());
          if (!api) return;
          for (const aid of ids) {
            try {
              await api.attach(meetingId, aid);
              set({
                attachedArtifactIds: [...get().attachedArtifactIds.filter((x) => x !== aid), aid],
              });
            } catch (e) {
              console.warn(`[store] attach artifact ${aid} failed:`, e);
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
      // Mobile doesn't follow cross-surface view broadcasts —
      // `currentMode` is purely local. Keep the items payload so
      // any future local mode switch sees fresh data.
      set({
        itemsByMode: { ...get().itemsByMode, [event.mode]: event.items },
      });
      return;
    case "items_update": {
      // Honor the mode's declared update_strategy. Append modes (chat,
      // transcript, actions, open_questions) send deltas — we upsert
      // by id so prior turns stay; replace modes (highlights, summary)
      // send the full list — we overwrite. The shared helper mirrors
      // the PWA's logic so all clients converge to the same state.
      const mode = get().availableModes.find((m) => m.id === event.mode);
      if (!mode) {
        set({ itemsByMode: { ...get().itemsByMode, [event.mode]: event.items } });
        return;
      }
      const current = get().itemsByMode[event.mode] ?? [];
      const next = applyItemsUpdate(current, event.items, mode);
      set({ itemsByMode: { ...get().itemsByMode, [event.mode]: next } });
      return;
    }
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
      // Server pushes this whenever auto-extract finishes or a manual
      // edit lands. No spinner to clear — tags just appear.
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
    case "pair_code_minted":
      set({ pairCode: { code: event.code, expires_at: event.expires_at } });
      return;
    case "paired_devices_changed":
      // Tick the seq; subscribers in the pair sheet + Paired Devices
      // settings card watch this and re-fetch /pair/devices.
      set({ pairedDevicesSeq: get().pairedDevicesSeq + 1 });
      return;
    // device_registered, artifacts_changed: not needed for Phase 1
    // demo state. Wire when their consumers land in later phases.
    default:
      return;
  }
}
