import type { ServerEvent } from "./types";
import type { Store } from "./store";
import { computeNextGlassesView } from "./state-machine";
import { applyItemsUpdate } from "./glasses/apply-items-update";
import { firstEnabledGlassesMode } from "./input/gesture-router";

const PROTOCOL_VERSION = 1;

let toastId = 0;
const TOAST_TTL_MS = 4000;

function pushToast(store: Store, text: string, level: "info" | "warn" | "error" = "warn") {
  const toast = {
    id: `t${++toastId}`,
    text,
    level,
    expiresAt: Date.now() + TOAST_TTL_MS,
  };
  store.update({ toasts: [...store.get().toasts, toast] });
}

export function handleServerEvent(event: ServerEvent, store: Store): void {
  store.update({ wsLastEventAt: Date.now() });

  switch (event.type) {
    case "snapshot": {
      if (event.protocol_version !== PROTOCOL_VERSION) {
        store.update({
          protocolVersionMatched: false,
          errorOverlay: {
            title: "Incompatible server",
            message: `PWA expects protocol version ${PROTOCOL_VERSION}, server is ${event.protocol_version}. Update one side.`,
            dismissable: false,
          },
        });
        return;
      }
      const nextGlassesView = event.meeting_state === "idle" ? "idle" : "active_list";
      // Discard listening state on snapshot per spec §5.1.3.
      const wasListening = store.get().glassesView === "listening";
      const snapshotMeetingStartedAt =
        event.meeting_state === "active" && !store.get().meetingStartedAt
          ? Date.now()
          : store.get().meetingStartedAt;
      // `currentMode` is per-surface UI state — we do NOT inherit
      // it from the server's snapshot. Each surface (browser PWA,
      // glasses, Mac, mobile) tracks which mode IT is currently
      // viewing locally, independent of the others. The store's
      // default (`"transcript"`) holds for fresh clients; mid-
      // session reconnects keep whatever mode the user was on
      // because the store survives the WS blip.
      //
      // We still capture the snapshot's items under their canonical
      // mode key (so itemsByMode["chat"] etc. is canonically
      // populated for any future local switch), just not the
      // currentMode pointer.
      store.update({
        protocolVersionMatched: true,
        meetingState: event.meeting_state,
        meetingStartedAt: snapshotMeetingStartedAt,
        currentMeetingId: event.meeting_id ?? null,
        availableModes: event.available_modes,
        displayTag: event.display_tag ?? null,
        metadata: event.metadata,
        itemsByMode: { ...store.get().itemsByMode, [event.mode]: event.items },
        status: event.status,
        priorContext: event.prior_context ?? null,
        availableDevices: event.devices ?? [],
        audioSourceDeviceId: event.audio_source_device_id ?? null,
        attachedMeetingIds: event.attached_meeting_ids ?? [],
        // Snapshot carries the active meeting's sensitivity (or the
        // default if idle). Trust it over local state so a reconnect
        // mid-meeting reflects whatever was set on another surface.
        assistSensitivity: event.assist_sensitivity ?? "moderate",
        glassesView: nextGlassesView,
        listeningTranscript: wasListening ? "" : store.get().listeningTranscript,
        listeningInterim: wasListening ? "" : store.get().listeningInterim,
        listeningStartedAt: wasListening ? null : store.get().listeningStartedAt,
        toasts: [],
      });
      return;
    }
    case "meeting_state_changed": {
      const next = computeNextGlassesView(
        store.get().glassesView,
        { kind: "meeting_state_changed", state: event.meeting_state },
        {},
      );
      let meetingStartedAt = store.get().meetingStartedAt;
      const update: Partial<Parameters<typeof store.update>[0]> = {
        meetingState: event.meeting_state,
        glassesView: next,
        currentMeetingId: event.meeting_id ?? null,
      };
      if (event.meeting_state === "active" && !meetingStartedAt) {
        meetingStartedAt = Date.now();
      } else if (event.meeting_state === "idle") {
        // Mirror the server's handle_stop_meeting cleanup locally —
        // clear meeting-specific state so the next compose surface
        // starts fresh. The server's stop_meeting preserves
        // items_per_mode["quick_asks"] (it's the user's library, not
        // meeting content), so do the same locally — wiping it
        // here means the Quick Asks modal renders empty between
        // meetings until the next snapshot or CRUD broadcast
        // repopulates it. Keep the library; clear everything else.
        meetingStartedAt = null;
        update.metadata = {};
        // Reset both view-state pointers on meeting end so the next
        // compose opens fresh on transcript on both DOM and glasses.
        // `currentMode` (DOM) ignores glasses opt-outs — it's a
        // different surface. `glassesCurrentMode` honors the user's
        // per-mode opt-out so the next meeting doesn't land back on
        // a hidden surface.
        update.currentMode = "transcript";
        update.glassesCurrentMode = firstEnabledGlassesMode(
          store.get().availableModes,
          store.get().settings.glassesModes,
        );
        // Reset the live summary/highlights scroll so the next meeting
        // opens at the tail rather than a stale offset from this one.
        update.glassesActiveListLineOffset = 0;
        const preservedQuickAsks = store.get().itemsByMode["quick_asks"] ?? [];
        update.itemsByMode = { quick_asks: preservedQuickAsks };
        update.liveTranscriptInterim = "";
        update.composeDescription = "";
        update.listeningTranscript = "";
        update.listeningInterim = "";
        update.priorContext = null;
        // Quick-asks state is per-meeting (the sub-state's
        // chat answer is from THIS meeting); the library itself
        // persists via server-side items_per_mode["quick_asks"].
        update.quickAskWaiting = false;
        update.quickAskAnswerText = null;
        update.quickAskDispatchAt = null;
        // Disarm any primed glasses stop confirmation so the next
        // meeting doesn't open with the stop sentinel pre-armed.
        update.glassesStopArmed = false;
        // Assist popup queue is per-meeting; reset the ledger so the
        // next meeting's first assist event pops cleanly.
        update.assistShown = null;
        update.assistShownIds = [];
        // Drop any unsent compose-time staging + clear the active
        // meeting's attached set. The next compose starts fresh.
        update.pendingArtifactAttachments = [];
        update.attachedArtifactIds = [];
        update.pendingAttachedMeetings = [];
        update.attachedMeetingIds = [];
        // Sensitivity is per-meeting; reset to default so the next
        // compose screen opens on Moderate (matching a fresh client).
        update.assistSensitivity = "moderate";
      }
      update.meetingStartedAt = meetingStartedAt;
      store.update(update);
      // Compose-time staged attachments are drained by a subscriber
      // in `main.ts` (which has the AuthBundle handle). The handler
      // here only updates state; the side-effect plumbing lives at
      // the seam where we can reach Auth0.
      return;
    }
    case "assist_sensitivity_changed":
      // Server broadcasts this on `set_assist_sensitivity` mid-meeting
      // AND on `start_meeting` so cross-device clients pick up the
      // value without a fresh snapshot. Idempotent — no-op when the
      // value matches what we already have.
      if (store.get().assistSensitivity !== event.value) {
        store.update({ assistSensitivity: event.value });
      }
      return;
    case "mode_changed":
      // PWA + glasses don't follow the server's view broadcast — each
      // surface picks its own mode locally (tab click in the browser,
      // double-tap cycle on the glasses). The mode_changed event
      // still carries items for the mode that was switched into, so
      // we keep that part: items_per_mode buckets stay in sync for
      // any future local switch. We intentionally drop the
      // `currentMode` + `displayTag` fields because following them
      // would teleport our view when another surface (Mac, mobile)
      // changes its mode.
      store.update({
        itemsByMode: { ...store.get().itemsByMode, [event.mode]: event.items },
      });
      return;
    case "display_tag_changed":
      // Same rationale as mode_changed: display_tag is view-state,
      // and PWA/glasses pick their own. The Mac/mobile broadcast for
      // their own use; we ignore it here.
      return;
    case "metadata_changed":
      store.update({ metadata: event.metadata });
      return;
    case "prior_context_changed": {
      // The server emits `summary: 0/0/0/0` to mean "cleared" (e.g.,
      // after stop). Normalize to null so UI conditionals can stay simple.
      const s = event.summary;
      const empty =
        s.preferences === 0 && s.facts === 0 && s.episodes === 0 && s.project_memories === 0;
      store.update({ priorContext: empty ? null : s });
      return;
    }
    case "devices_changed":
      store.update({ availableDevices: event.devices });
      return;
    case "device_registered":
      // Sent only to the registering client. We are now registered
      // as an `audio_capture` device backed by the glasses mic; latch
      // `ownDeviceId` so the audio-source reactor can recognize when
      // the server binds the meeting source to us. The merge into
      // `availableDevices` covers the case where the server's
      // broadcast hasn't landed yet on this connection.
      store.update({
        ownDeviceId: event.device.id,
        availableDevices: [
          ...store.get().availableDevices.filter((d) => d.id !== event.device.id),
          event.device,
        ],
      });
      return;
    case "audio_source_device_changed":
      store.update({ audioSourceDeviceId: event.device_id ?? null });
      return;
    case "artifacts_changed":
      // Server's authoritative attached-set for the active meeting.
      // Overwrite the local mirror — clients pre-check rows in the
      // attach picker against this. Stays in sync without polling
      // even when the OTHER client (Mac vs PWA) issued the attach.
      store.update({ attachedArtifactIds: event.artifact_ids });
      return;
    case "attached_meetings_changed":
      store.update({ attachedMeetingIds: event.meeting_ids });
      return;
    case "item_updated": {
      // One-row in-place update — used today by the expand_item
      // flow to land the agent's expansion into the matching
      // item's `detail`. Replace by id; if the id isn't present in
      // current items (rare race), drop silently.
      const cur = store.get().itemsByMode[event.mode] ?? [];
      const idx = cur.findIndex((it) => it.id === event.item.id);
      if (idx === -1) return;
      const next = [...cur];
      next[idx] = event.item;
      store.update({ itemsByMode: { ...store.get().itemsByMode, [event.mode]: next } });
      return;
    }
    case "items_update": {
      const modeOpt = store.get().availableModes.find((m) => m.id === event.mode);
      if (!modeOpt) return;
      let current = store.get().itemsByMode[event.mode] ?? [];
      // Chat-mode special case: drop optimistic-echo placeholders
      // ("chat-q-pending-…" / "chat-a-pending-…") before appending
      // the server's real Q+A pair. Without this, the pending
      // bubbles linger alongside the real ones after every send.
      if (event.mode === "chat") {
        current = current.filter(
          (it) => !it.id.startsWith("chat-q-pending-") && !it.id.startsWith("chat-a-pending-"),
        );
      }
      const next = applyItemsUpdate(current, event.items, modeOpt);
      // Snap the live summary/highlights scroll back to the tail when the
      // mode currently shown on glasses is the one that just changed — the
      // wearer chose "always show the latest" on an update. Updates to a
      // mode they aren't viewing leave their scroll position untouched.
      const snapLatest =
        event.mode === store.get().glassesCurrentMode ? { glassesActiveListLineOffset: 0 } : {};
      store.update({
        itemsByMode: { ...store.get().itemsByMode, [event.mode]: next },
        ...snapLatest,
      });
      return;
    }
    case "transcript_interim":
      store.update({ liveTranscriptInterim: event.text });
      return;
    case "status":
      store.update({ status: event.status });
      return;
    case "error":
      pushToast(store, `${event.code}: ${event.message}`, "warn");
      return;
  }
}
