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
  overlay.className = "modal-overlay";
  overlay.style.display = "none";
  parent.appendChild(overlay);

  const modal = document.createElement("div");
  modal.className = "modal";
  overlay.appendChild(modal);

  const header = document.createElement("div");
  header.style.cssText =
    "display:flex;justify-content:space-between;align-items:center;margin-bottom:16px;";
  header.innerHTML = `<h2 style="margin:0;font-size:18px;">Settings</h2>`;
  const closeBtn = document.createElement("button");
  closeBtn.textContent = "✕";
  closeBtn.className = "cta secondary";
  closeBtn.style.padding = "4px 8px";
  closeBtn.addEventListener("click", () => store.update({ settingsModalOpen: false }));
  header.appendChild(closeBtn);
  modal.appendChild(header);

  const form = document.createElement("div");
  form.style.cssText = "display:flex;flex-direction:column;gap:12px;";
  const urlInput = field("Server URL", "ws://laptop.local:7331", "text");
  const tokenInput = field("Server token", "", "password");
  const sonioxInput = field("Soniox API key", "", "password");
  form.append(urlInput.wrap, tokenInput.wrap, sonioxInput.wrap);
  modal.appendChild(form);

  const footer = document.createElement("div");
  footer.style.cssText = "display:flex;gap:8px;margin-top:16px;";
  const saveBtn = document.createElement("button");
  saveBtn.className = "cta";
  saveBtn.textContent = "Save & Reconnect";
  saveBtn.style.flex = "1";
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
  footer.appendChild(saveBtn);
  modal.appendChild(footer);

  function syncFromStore() {
    const s = store.get();
    overlay.style.display = s.settingsModalOpen ? "flex" : "none";
    if (s.settingsModalOpen) {
      urlInput.input.value = s.settings.serverUrl;
      tokenInput.input.value = s.settings.serverToken;
      sonioxInput.input.value = s.settings.sonioxKey;
    }
  }
  syncFromStore();
  store.subscribe((s) => s.settingsModalOpen, syncFromStore);
}

function field(labelText: string, placeholder: string, type: string) {
  const wrap = document.createElement("label");
  wrap.style.cssText = "display:flex;flex-direction:column;gap:4px;";
  const lab = document.createElement("span");
  lab.textContent = labelText;
  lab.style.cssText = "font-size:13px;color:var(--fg-dim);";
  const input = document.createElement("input");
  input.type = type;
  input.placeholder = placeholder;
  input.style.cssText =
    "background:var(--bg-elev);color:var(--fg);border:1px solid #25252a;padding:10px;border-radius:6px;font-family:ui-monospace,monospace;";
  wrap.append(lab, input);
  return { wrap, input };
}
