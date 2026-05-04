import type { Store } from "../store";
import { saveSetting } from "../storage";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
}

export function mountSettingsModal(
  parent: HTMLElement,
  store: Store,
  bridge: BridgeLike,
  onSave: () => void,
): void {
  const overlay = document.createElement("div");
  overlay.className = "settings-overlay";
  parent.appendChild(overlay);

  const modal = document.createElement("div");
  modal.className = "settings-modal";
  overlay.appendChild(modal);

  // Title
  const title = document.createElement("h2");
  title.className = "settings-title";
  title.textContent = "Settings";
  modal.appendChild(title);

  // Connection status row
  const statusRow = document.createElement("div");
  statusRow.className = "settings-status-row";
  const statusDot = document.createElement("span");
  statusDot.className = "top-bar-dot";
  const statusLabel = document.createElement("span");
  statusLabel.className = "label-mono";
  statusRow.append(statusDot, statusLabel);
  modal.appendChild(statusRow);

  // Form fields
  const urlInput = field("Server URL", "ws://localhost:7331", "text");
  const tokenInput = field("Server token", "", "password");
  const sonioxInput = field("Soniox API key", "", "password");
  modal.append(urlInput.wrap, tokenInput.wrap, sonioxInput.wrap);

  // Actions
  const actions = document.createElement("div");
  actions.className = "settings-actions";
  const cancelBtn = document.createElement("button");
  cancelBtn.className = "btn-ghost";
  cancelBtn.textContent = "Close";
  cancelBtn.addEventListener("click", () => store.update({ settingsModalOpen: false }));
  const saveBtn = document.createElement("button");
  saveBtn.className = "btn-primary";
  saveBtn.style.flex = "1";
  saveBtn.textContent = "Save & Reconnect";
  saveBtn.addEventListener("click", async () => {
    const settings = {
      serverUrl: urlInput.input.value,
      serverToken: tokenInput.input.value,
      sonioxKey: sonioxInput.input.value,
      lastMetadata: store.get().settings.lastMetadata,
    };
    await Promise.all([
      saveSetting(bridge, "serverUrl", settings.serverUrl),
      saveSetting(bridge, "serverToken", settings.serverToken),
      saveSetting(bridge, "sonioxKey", settings.sonioxKey),
    ]);
    store.update({ settings, settingsModalOpen: false });
    onSave();
  });
  actions.append(cancelBtn, saveBtn);
  modal.appendChild(actions);

  function syncFromStore() {
    const s = store.get();
    overlay.classList.toggle("open", s.settingsModalOpen);
    if (s.settingsModalOpen) {
      urlInput.input.value = s.settings.serverUrl;
      tokenInput.input.value = s.settings.serverToken;
      sonioxInput.input.value = s.settings.sonioxKey;
    }
    // Update status row
    const ws = s.wsStatus;
    statusDot.dataset.state =
      ws === "open" ? "ok" : ws === "connecting" || ws === "reconnecting" ? "pending" : "off";
    if (ws === "open") {
      statusLabel.textContent = `CONNECTED · ${s.settings.serverUrl || "ws://localhost:7331"}`;
    } else if (ws === "connecting") {
      statusLabel.textContent = "CONNECTING…";
    } else if (ws === "reconnecting") {
      statusLabel.textContent = "RECONNECTING…";
    } else {
      statusLabel.textContent = "DISCONNECTED";
    }
  }
  syncFromStore();
  store.subscribe((s) => s.settingsModalOpen, syncFromStore);
  store.subscribe((s) => s.wsStatus, syncFromStore);
  store.subscribe((s) => s.settings.serverUrl, syncFromStore);
}

function field(labelText: string, placeholder: string, type: string) {
  const wrap = document.createElement("label");
  wrap.className = "settings-field";
  const lab = document.createElement("span");
  lab.className = "label-mono";
  lab.textContent = labelText;
  const input = document.createElement("input");
  input.type = type;
  input.placeholder = placeholder;
  wrap.append(lab, input);
  return { wrap, input };
}
