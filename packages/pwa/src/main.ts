// Locally-bundled fonts. Importing each weight's CSS shim from
// `@fontsource/*` makes Vite emit the .woff2 files into dist/assets
// and inject @font-face rules pointing at them. Eliminates the
// Google Fonts CDN dependency that the EvenHub webview is finicky
// about (and would need two extra origins whitelisted in app.json).
// Match the Google Fonts URL the prior index.html used:
//   Bebas Neue (400 only), JetBrains Mono (400 + 500),
//   Space Grotesk (400 + 500 + 600).
// Body + display fonts come from the system stack now (-apple-system /
// SF Pro on iOS, Segoe UI on Windows, Roboto on Android) so the PWA
// reads as a first-party page inside the Even Hub WebView without
// shipping a webfont. JetBrains Mono is still bundled because we use
// it for timecodes / IDs where genuine monospace matters.
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";

import { waitForEvenAppBridge } from "@evenrealities/even_hub_sdk";
import { createStore } from "./store";
import { defaultAppState } from "./types";
import { boot } from "./boot";
import { createGlassesRenderer } from "./glasses/render";
import { buildEntryRebuild } from "./glasses/layout-entry";
import { buildUnpairedRebuild } from "./glasses/layout-unpaired";
import { nextAssistToShow } from "./glasses/assist-queue";
import { paintAurisMark } from "./glasses/auris-mark-bitmap";
import { createGestureCoalescer, handleBridgeEvent, markMomentT } from "./input/gesture-router";
import { handleLifecycleEvent } from "./input/lifecycle";
import { ReconnectingSocket } from "./ws";
import { handleServerEvent } from "./ws-handlers";
import { mountUI } from "./ui";
import { ArtifactsApi } from "./artifacts-api";
import { MeetingsApi } from "./meetings-api";
import { pickDetailTitle, formatHistorySummaryBody } from "./meeting-format";
import type { CtaActions } from "./ui/cta-region";
import { ListeningSession } from "./listening";
import { GlassesAudioSource } from "./glasses-audio-source";
import { initAuth, RePairRequired, type AuthBundle } from "./auth";
import { mountPairScreen } from "./ui/pair-screen";
import { SERVER_URL } from "./server-url";
import { getOrCreateDeviceId } from "./storage";
import { resolveDeviceLabel } from "./device-label";

async function start() {
  const bridge = await waitForEvenAppBridge();
  const store = createStore(defaultAppState());

  // Resolve auth state up front (one async read from the host's
  // persistent KV + JWT-claims decode — no network I/O), so boot()
  // can pick the right initial glasses layout. Unpaired users see a
  // pair-prompt; paired users see the "⌁ Ready" idle screen.
  const auth = await initAuth(SERVER_URL, bridge as unknown as Parameters<typeof initAuth>[1]);
  const isPaired = auth.identity !== null;

  await boot({
    bridge: bridge as unknown as Parameters<typeof boot>[0]["bridge"],
    store,
    env: import.meta.env,
    isPaired,
  });
  store.update({ auth: auth.identity });

  const app = document.querySelector<HTMLDivElement>("#app");
  if (!app) return;

  // No paired session? Render the pair screen and stop. On a
  // successful redeem the pair screen calls onPaired, which kicks
  // us into bootAuthenticated.
  if (!auth.identity) {
    mountPairScreen(app, auth, {
      onPaired: () => {
        console.log("[pair] redeem complete, identity:", auth.identity);
        // Flip the glasses display from the unpaired layout to the
        // "⌁ Ready" idle layout. Fire-and-forget — `createGlassesRenderer`
        // (built inside `bootAuthenticated`) only fires on `glassesView`
        // *changes*, and the default view is already "idle", so without
        // this explicit rebuild the glasses would stay on the pair
        // prompt forever.
        void bridge.rebuildPageContainer(buildEntryRebuild());
        // Clear any errorOverlay that boot() left lying around (the
        // dev-mode "Failed to initialize glasses display" warning from
        // createStartUpPageContainer in prototype mode). The pair
        // screen hid it; without this clear, mountUI's first render
        // surfaces it on the freshly-authed surface.
        store.update({ auth: auth.identity, errorOverlay: null });
        try {
          bootAuthenticated(app, store, bridge, auth);
          console.log("[pair] bootAuthenticated returned");
        } catch (e) {
          console.error("[pair] bootAuthenticated threw", e);
          throw e;
        }
      },
    });
    return;
  }

  bootAuthenticated(app, store, bridge, auth);
}

/// Common landing for "the refresh token died, return to the pair
/// screen with a banner". Hand-rolled because both the boot path
/// and the WS reconnect path need it.
function showRePairScreen(
  app: HTMLDivElement,
  store: ReturnType<typeof createStore>,
  bridge: Awaited<ReturnType<typeof waitForEvenAppBridge>>,
  auth: AuthBundle,
  reason: string,
): void {
  void auth.logout().then(async () => {
    store.update({ auth: null });
    // Flip the glasses display back to the unpaired splash. Without
    // this rebuild the glasses keep showing whatever was last
    // rendered (e.g., the entry menu), which contradicts the
    // browser-side pair prompt and confuses the user. Image
    // containers are placeholders after rebuild, so the logo has to
    // be re-uploaded too.
    try {
      await bridge.rebuildPageContainer(buildUnpairedRebuild());
      await paintAurisMark(bridge);
    } catch (e) {
      console.warn("[re-pair] failed to flip glasses to unpaired:", e);
    }
    mountPairScreen(app, auth, {
      bannerText: reason,
      onPaired: () => {
        store.update({ auth: auth.identity });
        bootAuthenticated(app, store, bridge, auth);
      },
    });
  });
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
  // Clear any pre-auth DOM (pair-screen, banners) so the live UI
  // mounts on a clean root. `mountUI` only appends — without this
  // clear, a successful redeem leaves the pair-screen lingering
  // underneath the authenticated UI, with the Pair button frozen
  // on "Pairing…" because the submit handler resolved but doesn't
  // reset state after `opts.onPaired()`.
  while (app.firstChild) app.removeChild(app.firstChild);

  // Single shared token provider with re-pair routing. Any consumer
  // that hits a RePairRequired (refresh token rejected) tears down
  // the live UI and shows the pair screen with a banner. Without
  // this wrap, the failure would bubble as a generic "connection
  // failed" toast and leave the user stranded on a dead session.
  let rePaired = false;
  const tokenProvider = async (): Promise<string> => {
    try {
      return await auth.getAccessToken();
    } catch (e) {
      if (e instanceof RePairRequired && !rePaired) {
        rePaired = true;
        showRePairScreen(
          app,
          store,
          bridge,
          auth,
          "Your session expired. Pair this device again to continue.",
        );
      }
      throw e;
    }
  };

  function makeSocket() {
    return new ReconnectingSocket({
      url: SERVER_URL,
      tokenProvider,
      onEvent: (event) => handleServerEvent(event, store),
      onStatus: (status) => store.update({ wsStatus: status }),
    });
  }

  let sock = makeSocket();

  const reconnect = () => {
    sock.close();
    sock = makeSocket();
  };

  // Resolve the stable device id once per session. Memoizing the
  // promise (rather than awaiting getOrCreateDeviceId on every
  // register) guarantees concurrent first-time calls share a single
  // generation instead of racing to mint two ids.
  let deviceIdPromise: Promise<string> | null = null;
  const ensureDeviceId = (): Promise<string> => (deviceIdPromise ??= getOrCreateDeviceId(bridge));

  // Resolve the human-readable label ("<serial> (G2)") once per
  // session from the glasses' own serial. Memoized like the device id.
  let deviceLabelPromise: Promise<string | null> | null = null;
  const ensureDeviceLabel = (): Promise<string | null> =>
    (deviceLabelPromise ??= resolveDeviceLabel(bridge));

  const listening = new ListeningSession(bridge as any, store, {
    getServerUrl: () => SERVER_URL,
    getAccessToken: tokenProvider,
  });
  // Stream the glasses mic to the server's /audio endpoint while
  // the meeting's audio source is bound to us. The reactor below
  // (subscribed to ownDeviceId / audioSourceDeviceId) starts and
  // stops this; main.ts just routes incoming `audioEvent` frames.
  const glassesAudio = new GlassesAudioSource(bridge as any, store, {
    getServerUrl: () => SERVER_URL,
    getAccessToken: tokenProvider,
  });

  // One coalescer for the session — collapses the duplicate gesture
  // deliveries real glasses emit for a single temple-tap (text + sys),
  // which were advancing the mode cycle twice. See createGestureCoalescer.
  const gestureCoalescer = createGestureCoalescer();

  bridge.onEvenHubEvent((e: unknown) => {
    const event = e as Record<string, unknown> & { audioEvent?: { audioPcm?: Uint8Array } };
    if (event?.audioEvent?.audioPcm) {
      // Two consumers, mutually exclusive at the state-machine
      // level: ListeningSession owns the mic during the describe-
      // meeting flow; GlassesAudioSource owns it during an active
      // meeting bound to us. The state machine keeps `listening`
      // and `active_list` from overlapping, so checking the view
      // (instead of arbitrating with a lock) is sufficient.
      if (store.get().glassesView === "listening") {
        listening.feedAudio(event.audioEvent.audioPcm);
      } else if (glassesAudio.isStreaming) {
        glassesAudio.feed(event.audioEvent.audioPcm);
      }
    }
    handleBridgeEvent(
      event as Parameters<typeof handleBridgeEvent>[0],
      store,
      (intent) => sock.send(intent),
      gestureCoalescer,
    );
    handleLifecycleEvent(
      event as Parameters<typeof handleLifecycleEvent>[0],
      store,
      bridge as unknown as Parameters<typeof handleLifecycleEvent>[2],
    );
  });

  // Auto-register the PWA as an `audio_capture` device on every
  // successful (re)connect. The server returns `device_registered`
  // with our assigned id, which `ws-handlers.ts` latches into
  // `ownDeviceId`. From then on, picking "Browser (Glasses)" in the
  // audio-source list binds the meeting to us and the reactor below
  // takes over.
  //
  // Why gate on the bridge: a plain browser tab without the EvenHub
  // WebView has no path to actually capture mic audio, so claiming
  // the capability would pollute the picker on every paired client.
  store.subscribe(
    (s) => s.wsStatus,
    (next) => {
      if (next !== "open") return;
      // Don't claim `audio_capture` if we don't actually have a
      // working glasses bridge — prototype mode (plain browser tab
      // outside the EvenHub WebView) would otherwise show up as a
      // phantom source on every paired client.
      if (!store.get().glassesBridgeReady) return;
      // Resolve the stable id at send-time (idempotent + memoized) so
      // the server reuses our identity across reconnects and keeps the
      // audio-source binding alive. Without it, a network switch
      // (wifi→5G) re-registers as a brand-new device and silently
      // stops recording.
      void (async () => {
        const [deviceId, label] = await Promise.all([ensureDeviceId(), ensureDeviceLabel()]);
        sock.send({
          type: "register_device",
          // Label by the glasses' own serial ("<serial> (G2)") so the
          // audio-source picker can tell multiple pairs apart; fall
          // back to the generic name when no serial is available
          // (prototype mode / glasses not connected).
          hostname: label ?? "Browser (Glasses)",
          capabilities: ["audio_capture"],
          device_id: deviceId,
        });
      })();
    },
  );

  // Audio-source reactor. When the server tells us we're the bound
  // source, start the /audio stream; when the binding moves away
  // (different device picked, meeting ended), stop. Both edges fire
  // through the same subscription so reconnect / mid-meeting source
  // changes converge cleanly.
  store.subscribe(
    (s) => s.audioSourceDeviceId === s.ownDeviceId && s.ownDeviceId !== null,
    (bound) => {
      if (bound) {
        void glassesAudio.start();
      } else {
        void glassesAudio.stop();
      }
    },
  );

  createGlassesRenderer(bridge as unknown as Parameters<typeof createGlassesRenderer>[0], store);

  // Quick-asks answer detector. While the glasses' quick_asks mode
  // is waiting (the user just picked a snippet), watch the chat-mode
  // items for a non-pending assistant turn that landed AFTER our
  // dispatch — that's the response. Pull its text into
  // `quickAskAnswerText` so the renderer flips to the answer
  // sub-state, and KEEP updating that text on every subsequent
  // chat update until the bubble stops streaming.
  //
  // The `quickAskDispatchAt` index (set in gesture-router on snippet
  // pick) is the chat tail length AT dispatch — items at or after
  // it are "could be ours"; earlier items are stale history. Without
  // this gate, re-sending the same prompt would lock onto the
  // previous answer until the new one finished streaming.
  //
  // Streaming behavior: chat-mode bubbles upsert in place by id, so
  // the same `find` re-locates the same growing bubble on each
  // delta. We hold onto `quickAskDispatchAt` until the server emits
  // the terminal update (`meta.streaming` absent or false) so the
  // detector keeps copying the latest accumulated text into the
  // glasses-facing answer field. Clearing only on terminal also
  // keeps cancellation paths (gesture-router) authoritative — they
  // null out dispatchAt explicitly when the user taps to return.
  store.subscribe(
    (s) => s.itemsByMode["chat"],
    () => {
      const s = store.get();
      if (s.quickAskDispatchAt === null) return;
      const chat = s.itemsByMode["chat"] ?? [];
      const newAssistant = chat
        .slice(s.quickAskDispatchAt)
        .find((it) => (it.meta as { role?: string } | undefined)?.role === "assistant");
      if (!newAssistant || newAssistant.text.trim().length === 0) return;
      const isTerminal =
        (newAssistant.meta as { streaming?: boolean } | undefined)?.streaming !== true;
      store.update({
        quickAskWaiting: false,
        quickAskAnswerText: newAssistant.text,
        ...(isTerminal ? { quickAskDispatchAt: null } : {}),
      });
    },
  );

  // Assist popup detector. The glasses surface assist items as a
  // page-swap popup that interrupts whichever view is currently
  // showing. Only one popup is on screen at a time; new assist
  // items that arrive while a popup is up are implicitly queued
  // by the `assistShownIds` ledger and pop on subsequent dismissals.
  //
  // Selector key encodes the three signals that should retrigger
  // the queue check: assist items grew, the seen ledger advanced
  // (after we put something on screen), OR the popup just cleared
  // (gesture-router set assistShown=null on dismiss). Missing the
  // last one means a dismissed popup never gets replaced by the
  // next queued item — there's no other event that fires the
  // detector after a dismiss.
  store.subscribe(
    (s) =>
      `${s.itemsByMode["assist"]?.length ?? 0}|${s.assistShownIds.length}|${s.assistShown === null ? "n" : "y"}`,
    () => {
      const s = store.get();
      if (s.assistShown !== null) return;
      const next = nextAssistToShow(s.itemsByMode["assist"] ?? [], s.assistShownIds);
      if (!next) return;
      store.update({
        assistShown: next,
        assistShownIds: [...s.assistShownIds, next.id],
      });
    },
  );

  // Bridge between the gesture router's view transitions and the
  // listening session. The gesture router doesn't know about audio
  // capture — it just dispatches `glassesView: "listening"` when the
  // user clicks the idle CTA on glasses, or "idle" when they cancel.
  // We watch the view and start/stop the mic accordingly. The phone
  // CTA still calls `actions.describeMeeting()` directly (which also
  // sets the view + starts listening) — that path is fine because
  // the subscription doesn't re-fire on a `listening → listening`
  // no-op.
  store.subscribe(
    (s) => s.glassesView,
    (next, prev) => {
      if (next === "listening" && prev !== "listening") {
        void listening.start();
      } else if (prev === "listening" && next !== "listening") {
        // Glasses-side double-tap (or any other path that flips
        // glassesView away from "listening") tears down the mic +
        // STT. `finish()` is internally idempotent — calling it
        // after `listening.finish()` already ran via the phone
        // CTA / VAD commit is a safe no-op (cleanup ?-chains
        // through null fields).
        void listening.finish();
      }
    },
  );

  const actions: CtaActions = {
    describeMeeting: () => {
      store.update({ glassesView: "listening" });
      void listening.start();
    },
    // Don't send `metadata` — the server preserves whatever it has in state
    // (extracted chips, manual edits) when the intent omits the field.
    startMeeting: (description: string, audioSourceDeviceId: string | null) =>
      sock.send({
        type: "start_meeting",
        description: description || undefined,
        audio_source_device_id: audioSourceDeviceId ?? undefined,
        // Carry the compose-screen sensitivity into the new meeting.
        // The server defaults to Moderate when omitted; sending
        // explicitly avoids surprises if a user picked Aggressive
        // or Minimal before tapping Start.
        assist_sensitivity: store.get().assistSensitivity,
      }),
    setAssistSensitivity: (value) => sock.send({ type: "set_assist_sensitivity", value }),
    markMoment: () => {
      sock.send({ type: "mark_moment", t: markMomentT(store.get()) });
      // Flash the glasses "+1" marker too — the wearer should see the
      // capture confirmed regardless of which surface triggered it.
      store.update({ momentMarkedSeq: store.get().momentMarkedSeq + 1 });
    },
    stopMeeting: () => sock.send({ type: "stop_meeting" }),
    stopListening: () => void listening.finish(),
    cancelListening: () => void listening.cancel(),
  };

  const bridgeForUi = bridge as unknown as {
    setLocalStorage(k: string, v: string): Promise<boolean>;
    getLocalStorage(k: string): Promise<string>;
    getDeviceInfo?: () => Promise<{ sn?: string | null; model?: string | null } | null>;
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
        const api = ArtifactsApi.from(SERVER_URL, tokenProvider);
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

  // Drain compose-time staged meeting attachments. Same shape as the
  // artifact drainer above; server-side attach is idempotent so a
  // re-fire is harmless.
  store.subscribe(
    (s) => `${s.meetingState}|${s.currentMeetingId}|${s.pendingAttachedMeetings.length}`,
    () => {
      const s = store.get();
      if (s.meetingState !== "active" || !s.currentMeetingId) return;
      if (s.pendingAttachedMeetings.length === 0) return;
      const ids = s.pendingAttachedMeetings;
      const parentId = s.currentMeetingId;
      store.update({ pendingAttachedMeetings: [] });
      void (async () => {
        const api = MeetingsApi.from(SERVER_URL, tokenProvider);
        if (!api) return;
        for (const mid of ids) {
          try {
            await api.attach(parentId, mid);
            store.update({
              attachedMeetingIds: [...store.get().attachedMeetingIds.filter((x) => x !== mid), mid],
            });
            console.log(`[meetings] attached ${mid} to meeting ${parentId}`);
          } catch (e) {
            console.warn(`[meetings] attach ${mid} failed:`, e);
          }
        }
      })();
    },
  );

  // Glasses history reactor. The gesture router flips `glassesView` to
  // history_list / history_summary and sets the loading flags; this
  // reactor does the async fetch and writes results back. Keyed on
  // (view | selectedId) so both edges fire through one subscription.
  // Stale-write guarded: if the wearer double-taps out mid-fetch, the
  // late result is dropped. Mirrors the fire-and-forget shape of the
  // attach drainers above. Cap matches the spec (recent 20, newest-first
  // as the server returns them).
  const HISTORY_LIST_CAP = 20;
  store.subscribe(
    (s) => `${s.glassesView}|${s.glassesHistorySelectedId ?? ""}`,
    () => {
      const s = store.get();
      if (s.glassesView === "history_list" && s.glassesHistoryLoading) {
        void (async () => {
          const api = MeetingsApi.from(SERVER_URL, tokenProvider);
          if (!api) {
            if (store.get().glassesView === "history_list")
              store.update({
                glassesHistoryLoading: false,
                glassesHistoryError: "No server configured — open Settings.",
              });
            return;
          }
          try {
            const all = await api.list();
            if (store.get().glassesView !== "history_list") return; // left mid-fetch
            store.update({
              glassesHistory: all.slice(0, HISTORY_LIST_CAP),
              glassesHistoryLoading: false,
              glassesHistoryError: null,
            });
          } catch (e) {
            if (store.get().glassesView !== "history_list") return;
            store.update({
              glassesHistoryLoading: false,
              glassesHistoryError: e instanceof Error ? e.message : "Couldn't load meetings.",
            });
          }
        })();
        return;
      }
      if (
        s.glassesView === "history_summary" &&
        s.glassesHistorySelectedId &&
        s.glassesHistorySummaryLoading
      ) {
        const id = s.glassesHistorySelectedId;
        void (async () => {
          const api = MeetingsApi.from(SERVER_URL, tokenProvider);
          if (!api) {
            if (store.get().glassesHistorySelectedId === id)
              store.update({
                glassesHistorySummaryLoading: false,
                glassesHistorySummaryError: "No server configured — open Settings.",
              });
            return;
          }
          try {
            const detail = await api.detail(id);
            if (
              store.get().glassesView !== "history_summary" ||
              store.get().glassesHistorySelectedId !== id
            )
              return; // left or switched meetings mid-fetch
            store.update({
              glassesHistorySummary: {
                title: pickDetailTitle(detail),
                body: formatHistorySummaryBody(detail),
              },
              glassesHistorySummaryLoading: false,
              glassesHistorySummaryError: null,
            });
          } catch (e) {
            if (
              store.get().glassesView !== "history_summary" ||
              store.get().glassesHistorySelectedId !== id
            )
              return;
            store.update({
              glassesHistorySummaryLoading: false,
              glassesHistorySummaryError: e instanceof Error ? e.message : "Couldn't load summary.",
            });
          }
        })();
      }
    },
  );
}

start().catch((err) => {
  console.error("boot failed", err);
  if (import.meta.env.DEV) {
    const app = document.querySelector<HTMLDivElement>("#app");
    if (app) {
      const box = document.createElement("pre");
      box.style.cssText =
        "color:#f88;background:#220;font:11px/1.4 monospace;padding:8px;margin:8px;white-space:pre-wrap;border:1px solid #844";
      box.textContent = `BOOT FAILED\n\n${err?.stack ?? String(err)}`;
      app.appendChild(box);
    }
  }
});
