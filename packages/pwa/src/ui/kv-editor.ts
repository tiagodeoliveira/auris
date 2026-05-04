import type { Store } from "../store";
import type { Intent } from "../types";

const DEBOUNCE_MS = 500;

export function mountKvEditor(parent: HTMLElement, store: Store, send: (i: Intent) => void): void {
  const wrap = document.createElement("div");
  wrap.className = "kv-editor";
  parent.appendChild(wrap);

  const list = document.createElement("div");
  wrap.appendChild(list);

  const addRow = document.createElement("div");
  addRow.className = "kv-add";
  const newKey = document.createElement("input");
  newKey.placeholder = "key";
  const newVal = document.createElement("input");
  newVal.placeholder = "value";
  const addBtn = document.createElement("button");
  addBtn.textContent = "add";
  addBtn.addEventListener("click", () => {
    const k = newKey.value.trim();
    const v = newVal.value.trim();
    if (!k) return;
    send({ type: "set_metadata", key: k, value: v || null });
    newKey.value = "";
    newVal.value = "";
  });
  addRow.append(newKey, newVal, addBtn);
  wrap.appendChild(addRow);

  const editingKeys = new Set<string>();
  const debounceTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  function render() {
    const s = store.get();
    list.innerHTML = "";
    for (const [k, v] of Object.entries(s.metadata)) {
      if (editingKeys.has(k)) continue; // preserve in-flight edit
      const row = document.createElement("div");
      row.className = "kv-row";
      const keySpan = document.createElement("span");
      keySpan.className = "label-mono";
      keySpan.textContent = k;
      const valInput = document.createElement("input");
      valInput.value = v;
      valInput.addEventListener("focus", () => editingKeys.add(k));
      valInput.addEventListener("blur", () => editingKeys.delete(k));
      valInput.addEventListener("input", () => {
        if (debounceTimers[k]) clearTimeout(debounceTimers[k]);
        debounceTimers[k] = setTimeout(() => {
          send({ type: "set_metadata", key: k, value: valInput.value || null });
        }, DEBOUNCE_MS);
      });
      const del = document.createElement("button");
      del.className = "kv-delete";
      del.textContent = "✕";
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
