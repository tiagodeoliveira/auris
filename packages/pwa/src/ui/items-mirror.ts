import type { Store } from "../store";
import { activeItems } from "../types";

export function mountItemsMirror(parent: HTMLElement, store: Store): void {
  const wrap = document.createElement("div");
  wrap.style.cssText = "padding:12px 16px;border-top:1px solid #25252a;flex:1;overflow-y:auto;";
  const title = document.createElement("h3");
  title.textContent = "Items so far";
  title.style.cssText = "margin:0 0 8px;font-size:14px;color:var(--fg-dim);font-weight:500;";
  const list = document.createElement("div");
  list.style.cssText = "font-family:ui-monospace, monospace;font-size:13px;line-height:1.6;";

  wrap.append(title, list);
  parent.appendChild(wrap);

  function render() {
    const s = store.get();
    list.innerHTML = "";
    activeItems(s).forEach((item, idx) => {
      const row = document.createElement("div");
      const cursor = idx === s.highlightIndex ? "▶ " : "  ";
      row.textContent = cursor + item.text;
      list.appendChild(row);
    });
  }

  render();
  store.subscribe((s) => s.itemsByMode[s.currentMode], render);
  store.subscribe((s) => s.highlightIndex, render);
}
