import { waitForEvenAppBridge } from "@evenrealities/even_hub_sdk";
import { createStore } from "./store";
import { defaultAppState } from "./types";
import { boot } from "./boot";
import { createGlassesRenderer } from "./glasses/render";
import { handleBridgeEvent } from "./input/gesture-router";
import { handleLifecycleEvent } from "./input/lifecycle";
import { ReconnectingSocket } from "./ws";
import { handleServerEvent } from "./ws-handlers";
import { mountUI } from "./ui";
import type { CtaActions } from "./ui/cta-region";

async function start() {
  const bridge = await waitForEvenAppBridge();
  const store = createStore(defaultAppState());
  await boot({
    bridge: bridge as unknown as Parameters<typeof boot>[0]["bridge"],
    store,
    env: import.meta.env,
  });

  function makeSocket() {
    return new ReconnectingSocket({
      url: store.get().settings.serverUrl,
      token: store.get().settings.serverToken,
      onEvent: (event) => handleServerEvent(event, store),
      onStatus: (status) => store.update({ wsStatus: status }),
    });
  }

  let sock = makeSocket();

  const reconnect = () => {
    sock.close();
    sock = makeSocket();
  };

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

  const actions: CtaActions = {
    describeMeeting: () =>
      store.update({ glassesView: "listening", listeningStartedAt: Date.now() }),
    // Soniox + audio wiring lands in Task 18.
    startMeeting: () =>
      sock.send({ type: "start_meeting", metadata: store.get().settings.lastMetadata }),
    pauseMeeting: () => sock.send({ type: "pause" }),
    resumeMeeting: () => sock.send({ type: "resume" }),
    stopMeeting: () => sock.send({ type: "stop_meeting" }),
    commitListening: () =>
      sock.send({
        type: "start_meeting",
        description: store.get().listeningTranscript,
        metadata: store.get().settings.lastMetadata,
      }),
    cancelListening: () =>
      store.update({
        glassesView: "idle",
        listeningTranscript: "",
        listeningInterim: "",
        listeningStartedAt: null,
      }),
  };

  const bridgeForUi = bridge as unknown as {
    setLocalStorage(k: string, v: string): Promise<boolean>;
    getLocalStorage(k: string): Promise<string>;
  };

  const app = document.querySelector<HTMLDivElement>("#app");
  if (app)
    mountUI(app, { store, send: (i) => sock.send(i), actions, bridge: bridgeForUi, reconnect });
}

start().catch((err) => {
  console.error("boot failed", err);
});
