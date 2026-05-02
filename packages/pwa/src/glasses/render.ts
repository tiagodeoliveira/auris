import type { Store } from "../store";
import type { GlassesView } from "../types";
import { buildIdleRebuild } from "./layout-idle";
import { buildActiveListLayout, buildBodyUpgrade, buildHeaderUpgrade } from "./layout-active-list";
import { buildActiveDetailLayout, buildDetailBodyUpgrade } from "./layout-active-detail";

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
      case "idle":
        await bridge.rebuildPageContainer(buildIdleRebuild());
        return;
      case "active_list":
        await bridge.rebuildPageContainer(buildActiveListLayout(store.get()));
        return;
      case "listening":
        return; // Task 10
      case "active_detail":
        await bridge.rebuildPageContainer(buildActiveDetailLayout(store.get()));
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

  // active_list body subscriptions — fire only when we're in active_list view.
  store.subscribe(
    (s) => s.items,
    () => {
      if (lastView === "active_list")
        void bridge.textContainerUpgrade(buildBodyUpgrade(store.get()));
    },
  );
  store.subscribe(
    (s) => s.highlightIndex,
    () => {
      if (lastView === "active_list")
        void bridge.textContainerUpgrade(buildBodyUpgrade(store.get()));
    },
  );
  store.subscribe(
    (s) => s.viewportStart,
    () => {
      if (lastView === "active_list")
        void bridge.textContainerUpgrade(buildBodyUpgrade(store.get()));
    },
  );
  store.subscribe(
    (s) => s.currentMode,
    () => {
      if (lastView === "active_list")
        void bridge.textContainerUpgrade(buildHeaderUpgrade(store.get()));
    },
  );
  store.subscribe(
    (s) => s.displayTag,
    () => {
      if (lastView === "active_list")
        void bridge.textContainerUpgrade(buildHeaderUpgrade(store.get()));
    },
  );

  // active_detail body subscription — fires when the detail item or its content changes.
  store.subscribe(
    (s) => (s.detailItemId ? s.items.find((i) => i.id === s.detailItemId) : null),
    () => {
      if (lastView === "active_detail")
        void bridge.textContainerUpgrade(buildDetailBodyUpgrade(store.get()));
    },
  );

  return { applyView };
}
