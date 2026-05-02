import type { Store } from "../store";
import type { Intent } from "../types";

const DEBOUNCE_MS = 500;

export function mountKvEditor(parent: HTMLElement, store: Store, send: (i: Intent) => void): void {
  const wrap = document.createElement("div");
  wrap.style.cssText = "padding:12px 16px;border-bottom:1px solid #25252a;";
  const title = document.createElement("h3");
  title.textContent = "Metadata";
  title.style.cssText = "margin:0 0 8px;font-size:14px;color:var(--fg-dim);font-weight:500;";
  const list = document.createElement("div");
  const addRow = document.createElement("div");
  addRow.style.cssText = "display:flex;gap:8px;margin-top:8px;";
  const newKey = document.createElement("input");
  newKey.placeholder = "key";
  newKey.style.cssText =
    "flex:1;background:var(--bg-elev);color:var(--fg);border:1px solid #25252a;padding:6px 8px;border-radius:6px;";
  const newVal = document.createElement("input");
  newVal.placeholder = "value";
  newVal.style.cssText = newKey.style.cssText;
  const addBtn = document.createElement("button");
  addBtn.textContent = "add";
  addBtn.className = "cta secondary";
  addBtn.style.padding = "6px 12px";
  addBtn.style.fontSize = "14px";
  addBtn.addEventListener("click", () => {
    const k = newKey.value.trim();
    const v = newVal.value.trim();
    if (!k) return;
    send({ type: "set_metadata", key: k, value: v || null });
    newKey.value = "";
    newVal.value = "";
  });
  addRow.append(newKey, newVal, addBtn);

  wrap.append(title, list, addRow);
  parent.appendChild(wrap);

  const editingKeys = new Set<string>();
  const debounceTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  function render() {
    const s = store.get();
    list.innerHTML = "";
    for (const [k, v] of Object.entries(s.metadata)) {
      if (editingKeys.has(k)) continue; // preserve in-flight edit
      const row = document.createElement("div");
      row.style.cssText = "display:flex;gap:8px;align-items:center;margin-bottom:4px;";
      const keySpan = document.createElement("span");
      keySpan.textContent = k;
      keySpan.style.cssText = "min-width:80px;color:var(--fg-dim);font-size:13px;";
      const valInput = document.createElement("input");
      valInput.value = v;
      valInput.style.cssText = newKey.style.cssText;
      valInput.addEventListener("focus", () => editingKeys.add(k));
      valInput.addEventListener("blur", () => editingKeys.delete(k));
      valInput.addEventListener("input", () => {
        if (debounceTimers[k]) clearTimeout(debounceTimers[k]);
        debounceTimers[k] = setTimeout(() => {
          send({ type: "set_metadata", key: k, value: valInput.value || null });
        }, DEBOUNCE_MS);
      });
      const del = document.createElement("button");
      del.textContent = "✕";
      del.className = "cta secondary";
      del.style.padding = "4px 8px";
      del.style.fontSize = "12px";
      del.addEventListener("click", () => {
        send({ type: "set_metadata", key: k, value: null });
      });
      row.append(keySpan, valInput, del);
      list.appendChild(row);
    }
  }

  render();
  store.subscribe((s) => s.metadata, render);
}
