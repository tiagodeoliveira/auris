import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import type { Store } from "../store";

interface SysEvent {
  sysEvent?: { eventType?: number };
}

interface BridgeLike {
  audioControl(open: boolean): Promise<boolean>;
  // Tells the host to tear down the page container we created at
  // startup. `exitMode` 0 = exit immediately (the user already
  // confirmed via the host's "Leave app?" dialog).
  shutDownPageContainer(exitMode: number): Promise<boolean>;
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
      // Host-driven clean exit (long-press → "Leave app?" confirmed).
      // Release the mic, then tell the host to tear down the page
      // container we created at startup — without this the container
      // is leaked on exit. exitMode 0 = exit immediately (the user
      // already confirmed via the host dialog, so don't pop a second).
      void bridge.audioControl(false);
      void bridge.shutDownPageContainer(0);
      return;
  }
}
