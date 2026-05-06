import type { Store } from "../store";
import type { AuthBundle } from "../auth";
import { SERVER_URL } from "../server-url";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
}

export function mountSettingsModal(
  parent: HTMLElement,
  store: Store,
  _bridge: BridgeLike,
  _onSave: () => void,
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

  // Connection status row. The server URL is baked at build time
  // (see `server-url.ts`); the user shouldn't be configuring it,
  // mirroring the Mac app.
  const statusRow = document.createElement("div");
  statusRow.className = "settings-status-row";
  const statusDot = document.createElement("span");
  statusDot.className = "top-bar-dot";
  const statusLabel = document.createElement("span");
  statusLabel.className = "label-mono";
  statusRow.append(statusDot, statusLabel);
  modal.appendChild(statusRow);

  // Close button — no editable fields, so a plain dismiss is enough.
  const actions = document.createElement("div");
  actions.className = "settings-actions";
  const closeBtn = document.createElement("button");
  closeBtn.className = "btn-primary";
  closeBtn.style.flex = "1";
  closeBtn.textContent = "Close";
  closeBtn.addEventListener("click", () => store.update({ settingsModalOpen: false }));
  actions.append(closeBtn);
  modal.appendChild(actions);

  let wasOpen = false;
  function syncOpenState() {
    const s = store.get();
    const isOpen = s.settingsModalOpen;
    overlay.classList.toggle("open", isOpen);
    if (isOpen && !wasOpen) {
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
      statusLabel.textContent = `CONNECTED · ${SERVER_URL}`;
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
  store.subscribe((s) => (s.auth ? s.auth.sub : ""), syncOpenState);
}
