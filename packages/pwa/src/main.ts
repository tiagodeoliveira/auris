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
import { ArtifactsApi } from "./artifacts-api";
import type { CtaActions } from "./ui/cta-region";
import { ListeningSession } from "./listening";
import { initAuth, readAuth0Config, type AuthBundle } from "./auth";
import { mountLoginScreen } from "./ui/login-screen";
import { SERVER_URL } from "./server-url";

async function start() {
  const bridge = await waitForEvenAppBridge();
  const store = createStore(defaultAppState());
  await boot({
    bridge: bridge as unknown as Parameters<typeof boot>[0]["bridge"],
    store,
    env: import.meta.env,
  });

  // Resolve the Auth0 session before constructing the WS or REST
  // clients — both of them need a fresh JWT on every (re)connect or
  // request, fetched through the Auth0 SDK.
  const auth0Config = readAuth0Config(import.meta.env as Record<string, string | undefined>);
  if (!auth0Config) {
    const app = document.querySelector<HTMLDivElement>("#app");
    if (app) {
      app.innerHTML = "";
      const err = document.createElement("div");
      err.className = "auth-misconfigured";
      err.textContent =
        "Auth0 is not configured. Set VITE_AUTH0_DOMAIN, VITE_AUTH0_PWA_CLIENT_ID, and VITE_AUTH0_API_AUDIENCE at build time.";
      app.appendChild(err);
    }
    return;
  }
  const auth = await initAuth(auth0Config);
  store.update({ auth: auth.identity });

  const app = document.querySelector<HTMLDivElement>("#app");
  if (!app) return;

  // Render the login screen if there's no active session, then stop;
  // the rest of the app boots only post-login (after the redirect
  // round-trip we'll re-enter `start()` and skip this branch).
  if (!auth.identity) {
    mountLoginScreen(app, auth);
    return;
  }

  bootAuthenticated(app, store, bridge, auth);
}

/// Everything that needed `auth` resolved. Split out from `start()`
/// so the login branch can short-circuit before constructing the
/// socket / UI / listening session — none of that should exist
/// while we're still pre-auth.
function bootAuthenticated(
  app: HTMLDivElement,
  store: ReturnType<typeof createStore>,
  bridge: Awaited<ReturnType<typeof waitForEvenAppBridge>>,
  auth: AuthBundle,
): void {
  function makeSocket() {
    return new ReconnectingSocket({
      url: SERVER_URL,
      tokenProvider: () => auth.getAccessToken(),
      onEvent: (event) => handleServerEvent(event, store),
      onStatus: (status) => store.update({ wsStatus: status }),
    });
  }

  let sock = makeSocket();

  const reconnect = () => {
    sock.close();
    sock = makeSocket();
  };

  const listening = new ListeningSession(bridge as any, store, {
    getServerUrl: () => SERVER_URL,
    getAccessToken: () => auth.getAccessToken(),
  });

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

  mountUI(app, {
    store,
    send: (i) => sock.send(i),
    actions,
    bridge: bridgeForUi,
    reconnect,
    auth,
  });

  // Drain compose-time staged artifact attachments once the server
  // confirms the meeting is active (we have a meeting id to attach
  // against). Best-effort POSTs; failures log but don't surface.
  // Server-side attach is idempotent so a re-fire is harmless.
  store.subscribe(
    (s) => `${s.meetingState}|${s.currentMeetingId}|${s.pendingArtifactAttachments.length}`,
    () => {
      const s = store.get();
      if (s.meetingState !== "active" || !s.currentMeetingId) return;
      if (s.pendingArtifactAttachments.length === 0) return;
      const ids = s.pendingArtifactAttachments;
      const meetingId = s.currentMeetingId;
      // Atomically clear so re-firing this subscriber doesn't
      // double-attach during transient state churn.
      store.update({ pendingArtifactAttachments: [] });
      void (async () => {
        const api = ArtifactsApi.from(SERVER_URL, () => auth.getAccessToken());
        if (!api) return;
        for (const aid of ids) {
          try {
            await api.attach(meetingId, aid);
            store.update({
              attachedArtifactIds: [
                ...store.get().attachedArtifactIds.filter((x) => x !== aid),
                aid,
              ],
            });
            console.log(`[artifacts] attached ${aid} to meeting ${meetingId}`);
          } catch (e) {
            console.warn(`[artifacts] attach ${aid} failed:`, e);
          }
        }
      })();
    },
  );
}

start().catch((err) => {
  console.error("boot failed", err);
});
