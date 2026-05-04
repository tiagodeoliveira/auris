import type { ServerEvent } from "./types";
import type { Store } from "./store";
import { computeNextGlassesView } from "./state-machine";
import { applyItemsUpdate } from "./glasses/apply-items-update";

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
      const nextGlassesView =
        event.meeting_state === "idle"
          ? "idle"
          : event.meeting_state === "active"
            ? "active_list"
            : "active_list"; // paused -> stay in active_list
      // Discard listening state on snapshot per spec §5.1.3.
      const wasListening = store.get().glassesView === "listening";
      const snapshotMeetingStartedAt =
        (event.meeting_state === "active" || event.meeting_state === "paused") &&
        !store.get().meetingStartedAt
          ? Date.now()
          : store.get().meetingStartedAt;
      store.update({
        protocolVersionMatched: true,
        meetingState: event.meeting_state,
        meetingStartedAt: snapshotMeetingStartedAt,
        availableModes: event.available_modes,
        currentMode: event.mode,
        displayTag: event.display_tag ?? null,
        metadata: event.metadata,
        itemsByMode: { ...store.get().itemsByMode, [event.mode]: event.items },
        status: event.status,
        glassesView: nextGlassesView,
        highlightIndex: 0,
        viewportStart: 0,
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
      if (event.meeting_state === "active" && !meetingStartedAt) {
        meetingStartedAt = Date.now();
      } else if (event.meeting_state === "idle") {
        meetingStartedAt = null;
      }
      store.update({ meetingState: event.meeting_state, glassesView: next, meetingStartedAt });
      return;
    }
    case "available_modes_changed":
      store.update({ availableModes: event.available_modes });
      return;
    case "mode_changed":
      store.update({
        currentMode: event.mode,
        displayTag: event.display_tag ?? null,
        itemsByMode: { ...store.get().itemsByMode, [event.mode]: event.items },
        highlightIndex: 0,
        viewportStart: 0,
      });
      return;
    case "display_tag_changed":
      store.update({ displayTag: event.tag ?? null });
      return;
    case "metadata_changed":
      store.update({ metadata: event.metadata });
      return;
    case "items_update": {
      const modeOpt = store.get().availableModes.find((m) => m.id === event.mode);
      if (!modeOpt) return;
      const current = store.get().itemsByMode[event.mode] ?? [];
      const next = applyItemsUpdate(current, event.items, modeOpt);
      store.update({ itemsByMode: { ...store.get().itemsByMode, [event.mode]: next } });
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
