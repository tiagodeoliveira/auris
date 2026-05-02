import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import { computeNextGlassesView } from "../state-machine";
import type { Store } from "../store";
import type { Intent } from "../types";

interface BridgeEvent {
  textEvent?: { eventType?: number | undefined };
  listEvent?: { eventType?: number | undefined };
  sysEvent?: { eventType?: number };
}

type SendIntent = (intent: Intent) => void;

export function handleBridgeEvent(event: BridgeEvent, store: Store, send: SendIntent): void {
  const inputEvent = event.textEvent ?? event.listEvent;
  if (inputEvent) {
    handleInput(normalizeEventType(inputEvent.eventType), store, send);
  }
  // Sys events are not handled here — lifecycle.ts handles them.
}

function normalizeEventType(t: number | undefined): number {
  return t === undefined ? OsEventTypeList.CLICK_EVENT : t;
}

function handleInput(eventType: number, store: Store, send: SendIntent): void {
  const state = store.get();

  switch (eventType) {
    case OsEventTypeList.CLICK_EVENT: {
      const next = computeNextGlassesView(state.glassesView, { kind: "ring_click" }, {});
      const patch: Parameters<Store["update"]>[0] = { glassesView: next };
      if (state.glassesView === "active_list" && next === "active_detail") {
        const item = state.items[state.highlightIndex];
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
        const next = Math.min(state.items.length - 1, state.highlightIndex + 1);
        store.update({ highlightIndex: next });
      }
      return;
    }
  }
}
