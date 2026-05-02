import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import type { Store } from "../store";

interface SysEvent {
  sysEvent?: { eventType?: number };
}

interface BridgeLike {
  audioControl(open: boolean): Promise<boolean>;
}

export function handleLifecycleEvent(event: SysEvent, store: Store, bridge: BridgeLike): void {
  const t = event.sysEvent?.eventType;
  if (t === undefined) return;
  switch (t) {
    case OsEventTypeList.FOREGROUND_EXIT_EVENT:
      store.update({ appForegrounded: false });
      if (store.get().glassesView === "listening") {
        void bridge.audioControl(false);
        store.update({
          glassesView: "idle",
          listeningTranscript: "",
          listeningInterim: "",
          listeningStartedAt: null,
        });
      }
      return;
    case OsEventTypeList.FOREGROUND_ENTER_EVENT:
      store.update({ appForegrounded: true });
      return;
    case OsEventTypeList.ABNORMAL_EXIT_EVENT:
      // Surface as toast (toast machinery lands in Task 17).
      store.update({ bleConnected: false });
      return;
    case OsEventTypeList.SYSTEM_EXIT_EVENT:
      // Tear-down hook for clean exit.
      void bridge.audioControl(false);
      return;
  }
}
