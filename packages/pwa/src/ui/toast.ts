import type { Store } from "../store";

export function mountToasts(parent: HTMLElement, store: Store): void {
  const container = document.createElement("div");
  container.style.cssText =
    "position:fixed;top:48px;left:16px;right:16px;display:flex;flex-direction:column;gap:8px;z-index:100;pointer-events:none;";
  parent.appendChild(container);

  function render() {
    const now = Date.now();
    // `expiresAt: null` means persistent — only the toast's owner
    // (e.g. the audio-capture banner) clears it. Time-based toasts
    // self-prune when they expire.
    const toasts = store.get().toasts.filter((t) => t.expiresAt === null || t.expiresAt > now);
    if (toasts.length !== store.get().toasts.length) {
      store.update({ toasts });
    }
    container.innerHTML = "";
    for (const t of toasts) {
      const el = document.createElement("div");
      el.style.cssText = `background:var(--bg-elev);border-left:4px solid var(--${t.level === "error" ? "error" : t.level === "warn" ? "warn" : "accent"});padding:12px 14px;border-radius:6px;font-size:14px;pointer-events:auto;`;
      el.textContent = t.text;
      container.appendChild(el);
    }
  }

  render();
  store.subscribe((s) => s.toasts, render);
  setInterval(render, 1000); // auto-prune expired toasts
}
