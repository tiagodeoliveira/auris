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
import { ListeningSession } from "./listening";

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

  const listening = new ListeningSession(bridge as any, store);

  bridge.onEvenHubEvent((e: unknown) => {
    const event = e as Record<string, unknown> & { audioEvent?: { audioPcm?: Uint8Array } };
    if (event?.audioEvent?.audioPcm && store.get().glassesView === "listening") {
      listening.feedAudio(event.audioEvent.audioPcm);
    }
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
    describeMeeting: () => {
      store.update({ glassesView: "listening" });
      void listening.start();
    },
    extractMetadata: (description: string) => {
      const d = description.trim();
      if (!d) return;
      store.update({ extractingMetadata: true });
      sock.send({ type: "extract_metadata", description: d });
    },
    // Don't send `metadata` — the server preserves whatever it has in state
    // (extracted chips, manual edits) when the intent omits the field.
    startMeeting: (description: string, audioSourceDeviceId: string | null) =>
      sock.send({
        type: "start_meeting",
        description: description || undefined,
        audio_source_device_id: audioSourceDeviceId ?? undefined,
      }),
    markMoment: () => {
      const startedAt = store.get().meetingStartedAt;
      const t = startedAt ? Math.max(0, Date.now() - startedAt) : 0;
      sock.send({ type: "mark_moment", t });
    },
    pauseMeeting: () => sock.send({ type: "pause" }),
    resumeMeeting: () => sock.send({ type: "resume" }),
    stopMeeting: () => sock.send({ type: "stop_meeting" }),
    stopListening: () => void listening.finish(),
    cancelListening: () => void listening.cancel(),
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
