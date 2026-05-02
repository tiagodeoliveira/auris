import { waitForEvenAppBridge } from "@evenrealities/even_hub_sdk";
import { createStore } from "./store";
import { defaultAppState } from "./types";
import { boot } from "./boot";
import { createGlassesRenderer } from "./glasses/render";
import { handleBridgeEvent } from "./input/gesture-router";
import { handleLifecycleEvent } from "./input/lifecycle";
import { ReconnectingSocket } from "./ws";

async function start() {
  const bridge = await waitForEvenAppBridge();
  const store = createStore(defaultAppState());
  await boot({
    bridge: bridge as unknown as Parameters<typeof boot>[0]["bridge"],
    store,
    env: import.meta.env,
  });

  const sock = new ReconnectingSocket({
    url: store.get().settings.serverUrl,
    token: store.get().settings.serverToken,
    onEvent: (_event) => {
      /* event handlers in Task 13 */
    },
    onStatus: (status) => store.update({ wsStatus: status }),
  });

  bridge.onEvenHubEvent((e: unknown) => {
    const event = e as Record<string, unknown>;
    handleBridgeEvent(event as Parameters<typeof handleBridgeEvent>[0], store, (intent) =>
      sock.send(intent),
    );
    handleLifecycleEvent(
      event as Parameters<typeof handleLifecycleEvent>[0],
      store,
      bridge as unknown as Parameters<typeof handleLifecycleEvent>[2],
    );
  });

  createGlassesRenderer(bridge as unknown as Parameters<typeof createGlassesRenderer>[0], store);

  // Mount placeholder UI (proper UI lands in Tasks 14-17).
  const app = document.querySelector<HTMLDivElement>("#app");
  if (app) {
    app.textContent = "Meeting Companion (booted)";
  }
}

start().catch((err) => {
  console.error("boot failed", err);
});
