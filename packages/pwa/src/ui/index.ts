import type { Store } from "../store";
import type { Intent } from "../types";
import type { AuthBundle } from "../auth";
import { mountTopBar } from "./top-bar";
import { mountComposeAttachments, mountComposeDescription } from "./compose-region";
import { mountComposeAudioSource } from "./compose-audio-source";
import { mountComposeStart } from "./compose-start";
import { mountComposeCard, mountComposeTitle } from "./compose-card";
import { mountMidMeetingSensitivity, mountSensitivityPicker } from "./sensitivity-picker";
import { mountHeaderStrip } from "./header-strip";
import { mountKvEditor } from "./kv-editor";
import { mountModeTabs } from "./mode-tabs";
import { mountCtaRegion, type CtaActions } from "./cta-region";
import { mountItemsMirror } from "./items-mirror";
import { mountChatInput } from "./chat-input";
import { mountSettingsModal } from "./settings-modal";
import { mountMeetingsModal } from "./meetings-modal";
import { mountArtifactsModal } from "./artifacts-modal";
import { mountQuickAsksModal } from "./quick-asks-modal";
import { mountToasts } from "./toast";
import { mountAudioCaptureToast } from "./audio-capture-toast";
import { mountErrorOverlay } from "./error-overlay";

export interface UiContext {
  store: Store;
  send: (intent: Intent) => void;
  actions: CtaActions;
  bridge: {
    setLocalStorage(k: string, v: string): Promise<boolean>;
    getLocalStorage(k: string): Promise<string>;
    /// Optional — present on the real EvenHub bridge, absent on the
    /// narrow KV bridge used in tests. Settings uses it to surface the
    /// connected glasses' serial in the About card.
    getDeviceInfo?: () => Promise<{ sn?: string | null; model?: string | null } | null>;
  };
  reconnect: () => void;
  auth: AuthBundle;
}

export function mountUI(root: HTMLElement, ctx: UiContext): void {
  // Always-visible top status row + settings gear.
  mountTopBar(root, ctx.store, () => ctx.store.update({ settingsModalOpen: true }));

  // === IDLE COMPOSE SURFACE ===
  // Card-based form mirroring the mobile app's `app/(tabs)/index.tsx`:
  // a title block + one card per section (Description / Tags / Audio
  // source / Assist sensitivity / Attachments) + the Start button.
  // Each card sub-mount appends into a content slot the card helper
  // returns; the card itself owns the title + subtitle + coral
  // underline + chrome. Cards self-hide on `meetingState !== "idle"`
  // via the subscriber on the card element; the active-meeting
  // surface (mode tabs, items, etc.) renders below.
  const compose = document.createElement("div");
  compose.className = "compose";
  root.appendChild(compose);

  // NEW MEETING heading + DESCRIBE · TAG · CAPTURE eyebrow. Self-hides
  // outside idle via its own subscriber.
  mountComposeTitle(compose, ctx.store);

  // Description + Tags card — multiline textarea with embedded mic
  // toggle followed by the metadata chip editor. Mobile groups
  // these two into one card (description sets up agent context;
  // tags refine it — they're related), and the PWA mirrors that
  // shape. The shared compose-card head shows DESCRIPTION as the
  // primary title; the inline TAGS sub-label introduces the chip
  // strip below.
  const descCard = mountComposeCard(
    compose,
    "Description",
    "The agent uses this to interpret the transcript.",
  );
  mountComposeDescription(descCard.content, ctx.store, ctx.actions);
  const tagsSub = document.createElement("div");
  tagsSub.className = "compose-subsection";
  const tagsLabel = document.createElement("p");
  tagsLabel.className = "compose-subsection-label";
  tagsLabel.textContent = "Tags";
  tagsSub.appendChild(tagsLabel);
  descCard.content.appendChild(tagsSub);
  mountKvEditor(tagsSub, ctx.store, ctx.send);

  // Audio source card — picks the registered device whose stream
  // feeds the next meeting. Idle only.
  const sourceCard = mountComposeCard(compose, "Audio source");
  mountComposeAudioSource(sourceCard.content, ctx.store);

  // Assist sensitivity card — three-segment picker. Local state on
  // compose; the value rides on the next `start_meeting` intent.
  // Mid-meeting toggle lives in `mountMidMeetingSensitivity` near
  // the mode tabs (renders during active assist view only).
  const sensCard = mountComposeCard(
    compose,
    "Assist sensitivity",
    "How aggressively the agent surfaces tips during the meeting.",
  );
  mountSensitivityPicker(sensCard.content, ctx.store, {
    onPick: (value) => ctx.store.update({ assistSensitivity: value }),
  });

  // Attachments card — meetings + artifacts as two sub-rows
  // (MEETINGS / ARTIFACTS) so the user sees both attach affordances
  // in one place. Replaces the two flat dashed pills the compose
  // screen used to show inline. Idle only.
  const attachCard = mountComposeCard(
    compose,
    "Attachments",
    "Past meetings and artifacts to link to this one.",
  );
  mountComposeAttachments(attachCard.content, ctx.store, ctx.auth);

  // Start button — full-width black CTA. Self-hides outside idle.
  mountComposeStart(compose, ctx.store, ctx.actions);

  // Hide the entire compose stack on `meetingState !== "idle"`.
  // The previous design kept the Tags card visible during active
  // meetings (so the user could still edit metadata chips), but
  // wrapping it in card chrome and floating it above the active-
  // meeting surface read as a layout regression. Tag editing
  // becomes idle-only for now; a future follow-up can surface a
  // mid-meeting tag affordance somewhere on the active surface
  // (e.g. a small chip strip under the mode tabs).
  //
  // Hiding `.compose` (rather than each card) also frees up the
  // entire flex slot, so the active-meeting surface elements
  // (header-strip, mode-tabs, items-mirror, …) can take the full
  // viewport without the scrollable compose container claiming
  // `flex: 1`.
  function syncComposeVisibility() {
    compose.style.display = ctx.store.get().meetingState === "idle" ? "" : "none";
  }
  syncComposeVisibility();
  ctx.store.subscribe((s) => s.meetingState, syncComposeVisibility);

  // === ACTIVE-MEETING SURFACE ===
  // Components self-hide outside active/paused. Lives directly on
  // root (not inside `.compose`) so the active layout uses the full
  // viewport without the compose card max-width constraint.
  mountHeaderStrip(root, ctx.store);
  mountModeTabs(root, ctx.store);
  // Mid-meeting sensitivity picker — surfaces only when the user
  // is on the assist tab during an active meeting. Sits between
  // the mode tabs and the items pane so it reads as a contextual
  // control for the current mode rather than global chrome.
  mountMidMeetingSensitivity(root, ctx.store, ctx.actions);
  mountItemsMirror(root, ctx.store, ctx.send);
  mountChatInput(root, ctx.store, ctx.send);

  // Sticky bottom action bar (Pause/Stop in active, listening UI when listening).
  mountCtaRegion(root, ctx.store, ctx.send, ctx.actions, ctx.auth);

  // Overlays.
  mountSettingsModal(root, ctx.store, ctx.bridge, ctx.reconnect, ctx.auth);
  mountMeetingsModal(root, ctx.store, ctx.auth);
  mountArtifactsModal(root, ctx.store, ctx.auth);
  mountQuickAsksModal(root, ctx.store, ctx.send);
  mountToasts(root, ctx.store);
  // Persistent banner when /audio is broken during an active
  // meeting. Pushes into the same toast queue mountToasts renders.
  mountAudioCaptureToast(ctx.store);
  mountErrorOverlay(root, ctx.store);
}
