import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import { computeNextGlassesView } from "../state-machine";
import type { Store } from "../store";
import type { Intent } from "../types";
import { activeItems } from "../types";

interface BridgeEvent {
  textEvent?: { eventType?: number | undefined };
  listEvent?: { eventType?: number | undefined };
  sysEvent?: { eventType?: number; eventSource?: number };
}

type SendIntent = (intent: Intent) => void;

export function handleBridgeEvent(event: BridgeEvent, store: Store, send: SendIntent): void {
  const inputEvent = event.textEvent ?? event.listEvent ?? sysEventAsClick(event.sysEvent);
  if (inputEvent) {
    handleInput(normalizeEventType(inputEvent.eventType), store, send);
  }
  // Other sysEvents (FOREGROUND_ENTER/EXIT, ABNORMAL/SYSTEM_EXIT,
  // IMU_DATA_REPORT) are lifecycle.ts's job — see sysEventAsClick.
}

function normalizeEventType(t: number | undefined): number {
  return t === undefined ? OsEventTypeList.CLICK_EVENT : t;
}

/// The simulator (and likely real glasses for some configs) delivers
/// the primary temple-tap as a `sysEvent` — *not* as a textEvent on
/// the focused container — with `eventSource: 1` (TOUCH_EVENT_FROM_
/// GLASSES_R). proto3 JSON omits scalar zeros, so a sysEvent with no
/// `eventType` field is implicitly CLICK_EVENT (0); double-click
/// arrives serialized as `eventType: 3`.
///
/// Returns a synthetic input-event shape only for click variants; all
/// other sysEvent kinds are lifecycle's responsibility and should not
/// be routed here (otherwise lifecycle FOREGROUND_EXIT etc. would
/// become a phantom click).
function sysEventAsClick(
  sys: { eventType?: number; eventSource?: number } | undefined,
): { eventType?: number } | undefined {
  if (!sys) return undefined;
  const t = sys.eventType ?? OsEventTypeList.CLICK_EVENT;
  if (t === OsEventTypeList.CLICK_EVENT || t === OsEventTypeList.DOUBLE_CLICK_EVENT) {
    return { eventType: t };
  }
  return undefined;
}

function handleInput(eventType: number, store: Store, send: SendIntent): void {
  const state = store.get();

  switch (eventType) {
    case OsEventTypeList.CLICK_EVENT: {
      // Chat mode doesn't use the list→detail transition: chat
      // items are already the full content, and the chat-mode
      // body wraps them across multiple lines. Click in chat is
      // a no-op for now (could later be repurposed, e.g. for a
      // voice-input dictation toggle).
      if (state.glassesView === "active_list" && state.currentMode === "chat") {
        return;
      }
      const next = computeNextGlassesView(state.glassesView, { kind: "ring_click" }, {});
      const patch: Parameters<Store["update"]>[0] = { glassesView: next };
      if (state.glassesView === "active_list" && next === "active_detail") {
        const item = activeItems(state)[state.highlightIndex];
        if (item) {
          patch.detailItemId = item.id;
          if (!item.detail) {
            send({ type: "expand_item", item_id: item.id });
          }
        }
      }
      if (state.glassesView === "active_detail" && next === "active_list") {
        patch.detailItemId = null;
      }
      store.update(patch);
      return;
    }
    case OsEventTypeList.DOUBLE_CLICK_EVENT: {
      if (state.meetingState === "active") {
        // Phase 0: meeting start timestamp not tracked client-side; use 0.
        send({ type: "mark_moment", t: 0 });
      }
      return;
    }
    case OsEventTypeList.SCROLL_TOP_EVENT: {
      if (state.glassesView === "active_list") {
        const next = Math.max(0, state.highlightIndex - 1);
        store.update({ highlightIndex: next, viewportStart: Math.min(state.viewportStart, next) });
      }
      return;
    }
    case OsEventTypeList.SCROLL_BOTTOM_EVENT: {
      if (state.glassesView === "active_list") {
        const next = Math.min(activeItems(state).length - 1, state.highlightIndex + 1);
        store.update({ highlightIndex: next });
      }
      return;
    }
  }
}
