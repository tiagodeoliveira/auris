import type { Store } from "../store";

export function mountErrorOverlay(parent: HTMLElement, store: Store): void {
  const overlay = document.createElement("div");
  overlay.style.cssText =
    "display:none;position:fixed;inset:0;background:rgba(0,0,0,0.85);z-index:200;align-items:center;justify-content:center;padding:24px;";
  parent.appendChild(overlay);

  const card = document.createElement("div");
  card.style.cssText =
    "background:var(--bg-elev);border:1px solid var(--error);border-radius:var(--radius);padding:24px;max-width:480px;text-align:center;";
  overlay.appendChild(card);

  const title = document.createElement("h2");
  title.style.cssText = "color:var(--error);margin:0 0 12px;";
  const message = document.createElement("p");
  message.style.cssText = "margin:0 0 16px;color:var(--fg);";
  card.append(title, message);

  function render() {
    const ov = store.get().errorOverlay;
    overlay.style.display = ov ? "flex" : "none";
    if (ov) {
      title.textContent = ov.title;
      message.textContent = ov.message;
    }
  }
  render();
  store.subscribe((s) => s.errorOverlay, render);
}
