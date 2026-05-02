import type { Store } from "../store";
import type { GlassesView } from "../types";
import { buildIdleRebuild } from "./layout-idle";

interface BridgeLike {
  rebuildPageContainer(container: unknown): Promise<boolean>;
  textContainerUpgrade(container: unknown): Promise<boolean>;
}

export interface GlassesRenderer {
  applyView(view: GlassesView): Promise<void>;
}

export function createGlassesRenderer(bridge: BridgeLike, store: Store): GlassesRenderer {
  let lastView: GlassesView | null = null;

  async function applyView(view: GlassesView): Promise<void> {
    if (view === lastView) return;
    lastView = view;
    switch (view) {
      case "idle": {
        await bridge.rebuildPageContainer(buildIdleRebuild());
        return;
      }
      case "listening":
      case "active_list":
      case "active_detail":
        // Implemented in subsequent tasks (8, 9, 10).
        return;
    }
  }

  // Subscribe to glassesView changes.
  store.subscribe(
    (s) => s.glassesView,
    (next) => {
      void applyView(next);
    },
  );

  return { applyView };
}
