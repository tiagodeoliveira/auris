import { describe, expect, test } from "vitest";
import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import { handleLifecycleEvent } from "./lifecycle";
import { createStore } from "../store";
import { defaultAppState } from "../types";
import { createMockBridge } from "../__test__/mock-bridge";

describe("lifecycle", () => {
  test("SYSTEM_EXIT_EVENT shuts down the page container and mutes audio", () => {
    // The host drives exit (long-press → "Leave app?"); confirming it
    // fires SYSTEM_EXIT_EVENT. We must release the mic AND tell the host
    // to tear down the startup page container — exitMode 0 (immediate,
    // the user already confirmed via the host dialog). Mirrors ERGram.
    const store = createStore(defaultAppState());
    const bridge = createMockBridge();

    handleLifecycleEvent(
      { sysEvent: { eventType: OsEventTypeList.SYSTEM_EXIT_EVENT } },
      store,
      bridge,
    );

    expect(bridge.audioControl).toHaveBeenCalledWith(false);
    expect(bridge.shutDownPageContainer).toHaveBeenCalledWith(0);
  });

  test("non-exit lifecycle events never shut down the page container", () => {
    // Foreground enter/exit are routine — they must not tear down the
    // page container, only SYSTEM_EXIT does.
    const store = createStore(defaultAppState());
    const bridge = createMockBridge();

    handleLifecycleEvent(
      { sysEvent: { eventType: OsEventTypeList.FOREGROUND_ENTER_EVENT } },
      store,
      bridge,
    );
    handleLifecycleEvent(
      { sysEvent: { eventType: OsEventTypeList.FOREGROUND_EXIT_EVENT } },
      store,
      bridge,
    );

    expect(bridge.shutDownPageContainer).not.toHaveBeenCalled();
  });
});
