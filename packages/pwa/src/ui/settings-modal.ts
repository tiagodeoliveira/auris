import type { Store } from "../store";
import type { AuthBundle } from "../auth";
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
  auth: AuthBundle,
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

  // Account section — replaces the old shared-token field. Shows
  // who's signed in and exposes a logout button. The access token
  // itself never appears in the UI.
  const account = document.createElement("section");
  account.className = "settings-account";
  const accountLabel = document.createElement("div");
  accountLabel.className = "label-mono";
  accountLabel.textContent = "Account";
  const accountWho = document.createElement("div");
  accountWho.className = "settings-account-who";
  const logoutBtn = document.createElement("button");
  logoutBtn.className = "btn-ghost";
  logoutBtn.textContent = "Sign out";
  logoutBtn.addEventListener("click", () => {
    void auth.logout();
  });
  account.append(accountLabel, accountWho, logoutBtn);
  modal.appendChild(account);

  // Connection status row
  const statusRow = document.createElement("div");
  statusRow.className = "settings-status-row";
  const statusDot = document.createElement("span");
  statusDot.className = "top-bar-dot";
  const statusLabel = document.createElement("span");
  statusLabel.className = "label-mono";
  statusRow.append(statusDot, statusLabel);
  modal.appendChild(statusRow);

  // Form fields. Server URL stays (you might point at a hosted
  // server vs localhost). Token is gone — auth is the JWT now.
  // Soniox key still applies (the PWA's dictation flow runs against
  // the user's own Soniox account, not the server).
  const urlInput = field("Server URL", "ws://localhost:7331", "text");
  const sonioxInput = field("Soniox API key", "", "password");
  modal.append(urlInput.wrap, sonioxInput.wrap);

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
      serverToken: store.get().settings.serverToken, // legacy, unused
      sonioxKey: sonioxInput.input.value,
      lastMetadata: store.get().settings.lastMetadata,
    };
    await Promise.all([
      saveSetting(bridge, "serverUrl", settings.serverUrl),
      saveSetting(bridge, "sonioxKey", settings.sonioxKey),
    ]);
    store.update({ settings, settingsModalOpen: false });
    onSave();
  });
  actions.append(cancelBtn, saveBtn);
  modal.appendChild(actions);

  let wasOpen = false;
  function syncOpenState() {
    const s = store.get();
    const isOpen = s.settingsModalOpen;
    overlay.classList.toggle("open", isOpen);
    if (isOpen && !wasOpen) {
      urlInput.input.value = s.settings.serverUrl;
      sonioxInput.input.value = s.settings.sonioxKey;
      const id = s.auth;
      accountWho.textContent = id?.email ?? id?.name ?? id?.sub ?? "(not signed in)";
    }
    wasOpen = isOpen;
  }
  function syncStatusRow() {
    const s = store.get();
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
  syncOpenState();
  syncStatusRow();
  store.subscribe((s) => s.settingsModalOpen, syncOpenState);
  store.subscribe((s) => s.wsStatus, syncStatusRow);
  store.subscribe((s) => s.settings.serverUrl, syncStatusRow);
  store.subscribe((s) => (s.auth ? s.auth.sub : ""), syncOpenState);
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
