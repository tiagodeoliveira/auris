//! Registers this PWA with the server as an `audio_capture` device
//! (the glasses mic) so it shows up in the audio-source pickers.
//!
//! Fires on either edge that can complete the preconditions: the WS
//! reaching "open" OR the glasses bridge becoming ready. Deduplicated
//! per connection: one `register_device` per WS open. Every skip is
//! logged with a `[register]` prefix so a missing glasses mic can be
//! diagnosed from the WebView console. There is no client-log path to
//! the server today, so these logs are console-only.
import type { Store } from "./store";
import type { Intent } from "./types";

export interface RegistrationDeps {
  send: (intent: Intent) => void;
  resolveIdentity: () => Promise<[deviceId: string, label: string | null]>;
  log?: (msg: string) => void;
}

export function wireDeviceRegistration(store: Store, deps: RegistrationDeps): () => void {
  const log = deps.log ?? ((m: string) => console.log(m));
  // Increments on every WS open; registration is latched per epoch.
  let epoch = 0;
  let registeredEpoch = -1;
  let inFlightEpoch = -1;

  const attempt = (trigger: string) => {
    const s = store.get();
    if (s.wsStatus !== "open") {
      log(`[register] skipped (${trigger}): ws not open (status=${s.wsStatus})`);
      return;
    }
    if (!s.glassesBridgeReady) {
      log(
        `[register] skipped (${trigger}): glasses bridge not ready ` +
          "(createStartUpPageContainer did not succeed yet)",
      );
      return;
    }
    if (registeredEpoch === epoch || inFlightEpoch === epoch) {
      log(`[register] skipped (${trigger}): already registered on this connection`);
      return;
    }
    const myEpoch = epoch;
    inFlightEpoch = myEpoch;
    void deps
      .resolveIdentity()
      .then(([deviceId, label]) => {
        if (myEpoch !== epoch || store.get().wsStatus !== "open") {
          log(`[register] skipped (${trigger}): connection changed while resolving identity`);
          return;
        }
        const hostname = label ?? "Browser (Glasses)";
        deps.send({
          type: "register_device",
          hostname,
          capabilities: ["audio_capture"],
          device_id: deviceId,
        });
        registeredEpoch = myEpoch;
        log(`[register] sent register_device (${trigger}) hostname="${hostname}"`);
      })
      .catch((e: unknown) => {
        log(`[register] failed (${trigger}): ${String(e)}`);
      })
      .finally(() => {
        if (inFlightEpoch === myEpoch) inFlightEpoch = -1;
      });
  };

  const unsubWs = store.subscribe(
    (s) => s.wsStatus,
    (next) => {
      if (next === "open") epoch += 1;
      if (next === "open") attempt("ws-open");
    },
  );
  const unsubBridge = store.subscribe(
    (s) => s.glassesBridgeReady,
    (ready) => {
      if (ready) attempt("bridge-ready");
    },
  );
  // Initial state may already satisfy both (or explain why not).
  if (store.get().wsStatus === "open") epoch += 1;
  attempt("init");
  return () => {
    unsubWs();
    unsubBridge();
  };
}
