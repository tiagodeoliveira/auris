import type { Store } from "../store";
import type { GlassesView } from "../types";
import { buildEntryRebuild } from "./layout-entry";
import {
  ACTIVITY_FRAME_INTERVAL_MS,
  GLASSES_STOP_MODE,
  MOMENT_FLASH_MS,
  activityFrame,
  activityIndicatorRestingContent,
  audioStalled,
  buildActiveListLayout,
  buildActivityUpgrade,
  buildBodyUpgrade,
  buildHeaderUpgrade,
  buildMomentUpgrade,
} from "./layout-active-list";
import {
  buildDescribeConfirmLayout,
  buildDescribeIdleLayout,
  buildListeningBodyUpgrade,
  buildListeningLayout,
} from "./layout-describe";
import { buildSelectAudioSourceLayout } from "./layout-select-audio-source";
import {
  QUICK_ASKS_SPINNER_FRAME_INTERVAL_MS,
  buildQuickAsksAnswerUpgrade,
  buildQuickAsksLayout,
  buildQuickAsksSpinnerUpgrade,
  quickAsksSubstate,
} from "./layout-quick-asks";
import { buildAssistPopupLayout } from "./layout-assist-popup";
import { buildHistoryListLayout } from "./layout-history-list";
import { buildHistorySummaryLayout } from "./layout-history-summary";
import type { Item } from "../types";

const QUICK_ASKS_MODE = "quick_asks";

interface BridgeLike {
  rebuildPageContainer(container: unknown): Promise<boolean>;
  textContainerUpgrade(container: unknown): Promise<boolean>;
}

export interface GlassesRenderer {
  applyView(view: GlassesView): Promise<void>;
}

export function createGlassesRenderer(rawBridge: BridgeLike, store: Store): GlassesRenderer {
  let lastView: GlassesView | null = null;
  let lastMode: string | null = null;
  // True while an assist popup is on screen. Gates every normal
  // render path through the wrapped `bridge` below — none of the
  // mode/view subscribers should touch the canvas while the popup
  // is up, since their target containers have been torn down by
  // the popup rebuild. The popup's own enter/exit rebuilds use
  // `rawBridge` directly to bypass the gate.
  let popupActive = false;
  const bridge: BridgeLike = {
    async rebuildPageContainer(c) {
      if (popupActive) return false;
      return rawBridge.rebuildPageContainer(c);
    },
    async textContainerUpgrade(c) {
      if (popupActive) return false;
      return rawBridge.textContainerUpgrade(c);
    },
  };
  // Quick-asks spinner animation timer — set when entering the
  // waiting sub-state, cleared on leave. Renderer-local because
  // it's pure presentation.
  let qaSpinnerInterval: ReturnType<typeof setInterval> | null = null;
  let qaSpinnerFrame = 0;
  // Activity-indicator animation timer — set while status.listening
  // is true and we're rendering active_list (non-quick_asks) mode.
  // Cleared on listening-stops or view-leaves.
  let activityInterval: ReturnType<typeof setInterval> | null = null;
  let activityTick = 0;
  // Moment "+1" flash timer — set when a moment is marked, cleared
  // when it expires or we leave the active-list surface.
  let momentFlashTimer: ReturnType<typeof setTimeout> | null = null;

  function clearMomentFlash(): void {
    if (momentFlashTimer !== null) {
      clearTimeout(momentFlashTimer);
      momentFlashTimer = null;
    }
  }

  function startActivityAnim(): void {
    if (activityInterval !== null) return;
    activityTick = 0;
    void bridge.textContainerUpgrade(buildActivityUpgrade(activityFrame(activityTick)));
    activityInterval = setInterval(() => {
      activityTick += 1;
      void bridge.textContainerUpgrade(buildActivityUpgrade(activityFrame(activityTick)));
    }, ACTIVITY_FRAME_INTERVAL_MS);
  }

  function stopActivityAnim(): void {
    if (activityInterval !== null) {
      clearInterval(activityInterval);
      activityInterval = null;
    }
  }
  function startQaSpinner(): void {
    qaSpinnerFrame = 0;
    qaSpinnerInterval = setInterval(() => {
      qaSpinnerFrame += 1;
      void bridge.textContainerUpgrade(buildQuickAsksSpinnerUpgrade(qaSpinnerFrame));
    }, QUICK_ASKS_SPINNER_FRAME_INTERVAL_MS);
  }
  function stopQaSpinner(): void {
    if (qaSpinnerInterval !== null) {
      clearInterval(qaSpinnerInterval);
      qaSpinnerInterval = null;
    }
  }

  function isQuickAsksActive(): boolean {
    return lastView === "active_list" && lastMode === QUICK_ASKS_MODE;
  }

  async function applyView(view: GlassesView): Promise<void> {
    if (view === lastView) return;
    const prev = lastView;
    lastView = view;
    if (prev === "active_list" && lastMode === QUICK_ASKS_MODE) stopQaSpinner();
    // Activity indicator only lives on the active-list (non-quick_asks)
    // layout. Stop the animation whenever we leave that surface, and
    // (re)start it on entry if the server says we're listening.
    if (prev === "active_list") {
      stopActivityAnim();
      // The "+1" container is torn down on leave; drop any pending
      // clear so it doesn't fire an upgrade at a gone container.
      clearMomentFlash();
    }

    switch (view) {
      case "idle":
        await bridge.rebuildPageContainer(buildEntryRebuild());
        return;
      case "describe_idle":
        await bridge.rebuildPageContainer(buildDescribeIdleLayout());
        return;
      case "listening":
        await bridge.rebuildPageContainer(buildListeningLayout(store.get()));
        return;
      case "describe_confirm":
        await bridge.rebuildPageContainer(buildDescribeConfirmLayout(store.get()));
        return;
      case "select_audio_source":
        await bridge.rebuildPageContainer(buildSelectAudioSourceLayout(store.get()));
        return;
      case "history_list":
        await bridge.rebuildPageContainer(buildHistoryListLayout(store.get()));
        return;
      case "history_summary":
        await bridge.rebuildPageContainer(buildHistorySummaryLayout(store.get()));
        return;
      case "active_list": {
        const s = store.get();
        lastMode = s.glassesCurrentMode;
        if (s.glassesCurrentMode === QUICK_ASKS_MODE) {
          await bridge.rebuildPageContainer(buildQuickAsksLayout(s));
          if (quickAsksSubstate(s) === "waiting") startQaSpinner();
        } else {
          await bridge.rebuildPageContainer(buildActiveListLayout(s));
        }
        // Initial frame is baked into the rebuild via the layout
        // builder (both the standard and quick_asks layouts now carry
        // the activity container); kick the animation if listening.
        if (indicatorShouldAnimate()) startActivityAnim();
        return;
      }
    }
  }

  /// Full rebuild for the quick_asks mode whenever its sub-state
  /// changes (list ↔ waiting ↔ answer). Each sub-state has a
  /// different container topology, so textContainerUpgrade can't
  /// cross between them.
  function rebuildQuickAsks(): void {
    const s = store.get();
    stopQaSpinner();
    // The rebuild recreates the activity container with a fresh
    // initial frame, so drop the old ticker and restart it if audio
    // is still flowing into this surface.
    stopActivityAnim();
    void bridge.rebuildPageContainer(buildQuickAsksLayout(s));
    if (quickAsksSubstate(s) === "waiting") startQaSpinner();
    if (indicatorShouldAnimate()) startActivityAnim();
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
    (s) => s.itemsByMode[s.glassesCurrentMode],
    () => {
      if (lastView !== "active_list") return;
      if (store.get().glassesCurrentMode === QUICK_ASKS_MODE) {
        // List items changed (server pushed an updated library);
        // sub-state stays "list" but the rendered labels change.
        rebuildQuickAsks();
      } else {
        void bridge.textContainerUpgrade(buildBodyUpgrade(store.get()));
      }
    },
  );
  // Live interim transcript — fires on every Soniox interim
  // delta so the user sees words come in incrementally instead of
  // waiting for the segment to commit. Only meaningful in the
  // live-transcript mode; `buildBody` skips the interim slot for
  // other modes regardless, but checking here avoids redundant
  // upgrades from chat/highlights/etc.
  store.subscribe(
    (s) => s.liveTranscriptInterim,
    () => {
      if (lastView === "active_list" && store.get().glassesCurrentMode === "transcript")
        void bridge.textContainerUpgrade(buildBodyUpgrade(store.get()));
    },
  );
  // Live summary/highlights scroll — the wearer moved the window, so
  // re-render the body at the new offset. Only the scrollable modes
  // consult the offset; the guard avoids redundant upgrades elsewhere.
  store.subscribe(
    (s) => s.glassesActiveListLineOffset,
    () => {
      const s = store.get();
      if (
        lastView === "active_list" &&
        (s.glassesCurrentMode === "summary" || s.glassesCurrentMode === "highlights")
      )
        void bridge.textContainerUpgrade(buildBodyUpgrade(s));
    },
  );
  store.subscribe(
    (s) => s.glassesCurrentMode,
    () => {
      if (lastView !== "active_list") return;
      const next = store.get().glassesCurrentMode;
      const crossingQuickAsks = next === QUICK_ASKS_MODE || lastMode === QUICK_ASKS_MODE;
      lastMode = next;
      if (crossingQuickAsks) {
        // The quick-asks layout has a different container topology
        // (list-only or full-screen text) than the active-list
        // layout (header + body + activity indicator). A rebuild
        // is required when crossing the boundary in either
        // direction; textUpgrade can't bridge the topology change.
        if (next === QUICK_ASKS_MODE) {
          // rebuildQuickAsks rebuilds to the quick_asks page (which
          // carries its own activity container) and owns the indicator
          // ticker lifecycle across the swap.
          rebuildQuickAsks();
        } else {
          void bridge.rebuildPageContainer(buildActiveListLayout(store.get()));
          if (indicatorShouldAnimate()) startActivityAnim();
        }
      } else {
        void bridge.textContainerUpgrade(buildHeaderUpgrade(store.get()));
        void bridge.textContainerUpgrade(buildBodyUpgrade(store.get()));
      }
    },
  );
  store.subscribe(
    (s) => s.displayTag,
    () => {
      if (lastView === "active_list")
        void bridge.textContainerUpgrade(buildHeaderUpgrade(store.get()));
    },
  );

  // Audio-capture state drives the header's "NO AUDIO" banner. Re-push
  // the header on every transition so the glasses display reflects a
  // recording stall (or recovery) in real time — the wearer is looking
  // at the glasses, not the phone, so this is the surface that matters.
  store.subscribe(
    (s) => s.audioCaptureState.kind,
    () => {
      if (lastView !== "active_list") return;
      // Header banner ("NO AUDIO"/"AUDIO LOST") exists only on the
      // standard active-list layout; the indicator carries the same
      // health on every layout (incl. Quick Asks, which has no header).
      void bridge.textContainerUpgrade(buildHeaderUpgrade(store.get()));
      refreshIndicator();
    },
  );

  // Stop-confirmation arming. The body swaps between the resting
  // "> Stop" and the armed confirm prompt; re-push it the instant the
  // wearer arms (first tap) or cancels (mode-cycle) so the glasses
  // reflect the new affordance. Body container only exists on the
  // standard active-list layout, which is where GLASSES_STOP_MODE
  // always renders (the stop sentinel is never a quick_asks substate).
  store.subscribe(
    (s) => s.glassesStopArmed,
    () => {
      if (lastView === "active_list" && store.get().glassesCurrentMode === GLASSES_STOP_MODE)
        void bridge.textContainerUpgrade(buildBodyUpgrade(store.get()));
    },
  );

  /// True when the activity indicator should be ticking: we're on the
  /// active-list view (standard OR quick_asks — both layouts now carry
  /// the indicator container), the server reports audio is reaching
  /// it, AND this client's audio WS is not stalled. A stall takes
  /// priority over "listening" so a mid-meeting drop surfaces as a
  /// stopped warning glyph rather than a misleading live animation.
  function indicatorShouldAnimate(): boolean {
    const s = store.get();
    return lastView === "active_list" && s.status.listening && !audioStalled(s);
  }

  /// Reconcile the indicator with the current audio health. Animate
  /// while flowing; otherwise stop the ticker and push the resting
  /// content (a warning glyph when capture has stalled mid-meeting,
  /// else a blank). Called on every signal that can change health.
  function refreshIndicator(): void {
    if (lastView !== "active_list") return;
    if (indicatorShouldAnimate()) {
      startActivityAnim();
    } else {
      stopActivityAnim();
      void bridge.textContainerUpgrade(
        buildActivityUpgrade(activityIndicatorRestingContent(store.get())),
      );
    }
  }

  // Server-reported "audio is flowing" edge. Runs in quick_asks too
  // now that that layout carries the indicator.
  store.subscribe((s) => s.status.listening, refreshIndicator);

  // Moment-marked edge — flash the "+1" marker just left of the
  // activity indicator, then clear it after MOMENT_FLASH_MS. Only the
  // standard active-list layout carries the moment container; quick_asks
  // has no header zone and never marks moments via tap, so skip it
  // there (a stray phone-CTA mark while the wearer is on quick_asks
  // simply shows no flash rather than upgrading a missing container).
  store.subscribe(
    (s) => s.momentMarkedSeq,
    () => {
      if (lastView !== "active_list") return;
      if (store.get().glassesCurrentMode === QUICK_ASKS_MODE) return;
      void bridge.textContainerUpgrade(buildMomentUpgrade(true));
      clearMomentFlash();
      momentFlashTimer = setTimeout(() => {
        momentFlashTimer = null;
        void bridge.textContainerUpgrade(buildMomentUpgrade(false));
      }, MOMENT_FLASH_MS);
    },
  );

  // Quick-asks sub-state — `waiting` and `answer` carry entirely
  // different layouts than the list view, so any change to either
  // backing field triggers a full rebuild while we're showing the
  // mode. Selector composes both so the subscriber fires exactly
  // when the rendered sub-state would change.
  store.subscribe(
    (s) => `${s.quickAskWaiting ? 1 : 0}|${s.quickAskAnswerText === null ? "" : "a"}`,
    () => {
      if (isQuickAsksActive()) rebuildQuickAsks();
    },
  );

  // Streaming answer text — once the sub-state is already `answer`,
  // each chat delta updates `quickAskAnswerText` in place. The
  // sub-state selector above doesn't fire (the rendered phase is
  // unchanged), so we need a separate subscriber that pushes the
  // new text into the existing full-screen container via
  // `textContainerUpgrade` (flicker-free, no topology change).
  // Skip when text is null (we're in `list` or just left `answer`)
  // or when we're not even on the quick_asks surface.
  store.subscribe(
    (s) => s.quickAskAnswerText,
    (text) => {
      if (text === null) return;
      if (!isQuickAsksActive()) return;
      if (quickAsksSubstate(store.get()) !== "answer") return;
      void bridge.textContainerUpgrade(buildQuickAsksAnswerUpgrade(text));
    },
  );

  // Listening transcript subscription — fires on every interim /
  // final transcript delta while we're in the listening view. The
  // describe_confirm screen is static (transcript preview snapshotted
  // at entry) so no subscription needed there.
  store.subscribe(
    (s) => s.listeningTranscript + s.listeningInterim,
    () => {
      if (lastView === "listening")
        void bridge.textContainerUpgrade(buildListeningBodyUpgrade(store.get()));
    },
  );

  // Audio-source list refresh — lists can't be updated in place
  // (firmware constraint), so rebuild the whole page whenever the
  // available-devices set changes WHILE the user is on the
  // source-pick screen. Selector serializes ids so the subscription
  // fires only on real membership changes, not on every store update.
  store.subscribe(
    (s) => s.availableDevices.map((d) => d.id).join("|"),
    () => {
      if (lastView === "select_audio_source")
        void bridge.rebuildPageContainer(buildSelectAudioSourceLayout(store.get()));
    },
  );

  // History list — the row data arrives async (loading → loaded/empty/
  // error) while the view stays `history_list`, so `applyView`'s dedupe
  // won't fire. Rebuild on any change to the backing slice. Lists can't
  // be upgraded in place (firmware), so it's a full rebuild like the
  // audio-source list. Selector serializes the slice so it fires only
  // on real changes.
  store.subscribe(
    (s) =>
      // Capture the fields pickDetailTitle actually renders (title /
      // description), not just the id, so a row whose title resolves
      // in place still repaints. The summary selector below does the
      // same with title+body.
      `${s.glassesHistoryLoading ? 1 : 0}|${s.glassesHistoryError ?? ""}|${s.glassesHistory
        .map((m) => `${m.id}:${m.metadata.title ?? ""}:${m.description ?? ""}`)
        .join(",")}`,
    () => {
      if (lastView === "history_list")
        void bridge.rebuildPageContainer(buildHistoryListLayout(store.get()));
    },
  );

  // History summary — the detail fetch resolves while the view stays
  // `history_summary`; rebuild the popup when it lands (or errors), and
  // when the wearer scrolls the body window (the line offset is part of
  // the selector so change-only subscribe fires on a scroll).
  store.subscribe(
    (s) =>
      `${s.glassesHistorySummaryLoading ? 1 : 0}|${s.glassesHistorySummaryError ?? ""}|${
        s.glassesHistorySummary ? s.glassesHistorySummary.title + s.glassesHistorySummary.body : ""
      }|${s.glassesHistorySummaryLineOffset}`,
    () => {
      if (lastView === "history_summary")
        void bridge.rebuildPageContainer(buildHistorySummaryLayout(store.get()));
    },
  );

  // Assist popup — page-swap over whatever view is active. Enters
  // by stopping animations that talk to soon-to-be-torn-down
  // containers, flipping the `popupActive` gate so the rest of the
  // subscribers stop pushing renders, and rebuilding to the popup
  // layout. Exits by dropping the gate and forcing a re-rebuild of
  // the underlying `glassesView` (clearing `lastView` so the
  // dedupe in `applyView` doesn't short-circuit "returning to the
  // same view" as a no-op). Uses `rawBridge` directly so the
  // popup's own rebuilds bypass the gate.
  store.subscribe(
    (s) => s.assistShown,
    (shown: Item | null) => {
      if (shown !== null) {
        stopQaSpinner();
        stopActivityAnim();
        popupActive = true;
        void rawBridge.rebuildPageContainer(buildAssistPopupLayout(shown));
      } else if (popupActive) {
        popupActive = false;
        const currentView = store.get().glassesView;
        lastView = null;
        void applyView(currentView);
      }
    },
  );

  return { applyView };
}
