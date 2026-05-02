import { waitForEvenAppBridge } from "@evenrealities/even_hub_sdk";
import { createStore } from "./store";
import { defaultAppState } from "./types";
import { boot } from "./boot";
import { createGlassesRenderer } from "./glasses/render";

async function start() {
  const bridge = await waitForEvenAppBridge();
  const store = createStore(defaultAppState());
  await boot({
    bridge: bridge as unknown as Parameters<typeof boot>[0]["bridge"],
    store,
    env: import.meta.env,
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
