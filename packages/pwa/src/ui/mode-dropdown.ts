import type { Store } from "../store";
import type { Intent } from "../types";

export function mountModeDropdown(
  parent: HTMLElement,
  store: Store,
  send: (i: Intent) => void,
): void {
  const wrap = document.createElement("div");
  wrap.style.cssText =
    "display:flex;align-items:center;gap:8px;padding:12px 16px;border-bottom:1px solid #25252a;";

  const label = document.createElement("label");
  label.textContent = "Mode:";
  label.style.color = "var(--fg-dim)";
  label.style.fontSize = "14px";

  const select = document.createElement("select");
  select.style.cssText =
    "background:var(--bg-elev);color:var(--fg);border:1px solid #25252a;padding:6px 10px;border-radius:6px;flex:1;";

  const tag = document.createElement("span");
  tag.style.color = "var(--fg-dim)";
  tag.style.fontSize = "12px";

  wrap.append(label, select, tag);
  parent.appendChild(wrap);

  function render() {
    const s = store.get();
    select.innerHTML = "";
    for (const mode of s.availableModes) {
      const opt = document.createElement("option");
      opt.value = mode.id;
      opt.textContent = mode.label;
      if (mode.id === s.currentMode) opt.selected = true;
      select.appendChild(opt);
    }
    tag.textContent = s.displayTag ?? "";
  }

  select.addEventListener("change", () => {
    const next = select.value;
    store.update({ currentMode: next });
    send({ type: "set_mode", mode: next });
  });

  render();
  store.subscribe((s) => s.availableModes, render);
  store.subscribe((s) => s.currentMode, render);
  store.subscribe((s) => s.displayTag, render);
}
