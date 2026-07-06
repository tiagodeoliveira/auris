import type { Store } from "../store";
import type { Intent } from "../types";

export function mountKvEditor(parent: HTMLElement, store: Store, send: (i: Intent) => void): void {
  const wrap = document.createElement("div");
  wrap.className = "kv-editor";
  parent.appendChild(wrap);

  // Wrapping chip strip — every entry is one compact pill, plus a trailing
  // "+ ADD" pill that turns into an inline-edit chip when clicked.
  const strip = document.createElement("div");
  strip.className = "kv-strip";
  wrap.appendChild(strip);

  const editingKeys = new Set<string>();
  let addOpen = false;

  function autoSize(input: HTMLInputElement, fallback = 4) {
    input.size = Math.max(input.value.length, input.placeholder.length, fallback);
  }

  function buildExistingChip(k: string, v: string): HTMLDivElement {
    const chip = document.createElement("div");
    chip.className = "kv-chip";

    const keySpan = document.createElement("span");
    keySpan.className = "kv-chip-key";
    keySpan.textContent = k;

    const valInput = document.createElement("input");
    valInput.className = "kv-chip-val";
    valInput.value = v;
    autoSize(valInput);

    let committed = v;
    function commit() {
      const next = valInput.value.trim();
      if (next === committed) return;
      committed = next;
      send({ type: "set_metadata", key: k, value: next || null });
    }

    valInput.addEventListener("focus", () => editingKeys.add(k));
    valInput.addEventListener("input", () => autoSize(valInput));
    valInput.addEventListener("blur", () => {
      editingKeys.delete(k);
      commit();
    });
    valInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        valInput.blur(); // triggers commit via blur handler
      } else if (e.key === "Escape") {
        valInput.value = committed;
        autoSize(valInput);
        valInput.blur();
      }
    });

    const del = document.createElement("button");
    del.type = "button";
    del.className = "kv-chip-x";
    del.title = "Remove";
    del.textContent = "×";
    del.addEventListener("mousedown", (e) => e.preventDefault()); // don't steal focus
    del.addEventListener("click", () => {
      send({ type: "set_metadata", key: k, value: null });
    });

    chip.append(keySpan, valInput, del);
    return chip;
  }

  function buildAddToggle(): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "kv-add-toggle";
    btn.textContent = "+ ADD";
    btn.addEventListener("click", () => {
      addOpen = true;
      render();
      strip.querySelector<HTMLInputElement>(".kv-chip-key-input")?.focus();
    });
    return btn;
  }

  function buildAddChip(): HTMLDivElement {
    const chip = document.createElement("div");
    chip.className = "kv-chip kv-chip-new";

    const keyInput = document.createElement("input");
    keyInput.className = "kv-chip-key-input";
    keyInput.placeholder = "key";
    autoSize(keyInput);

    const valInput = document.createElement("input");
    valInput.className = "kv-chip-val";
    valInput.placeholder = "value";
    autoSize(valInput);

    let committed = false;
    function close() {
      addOpen = false;
      render();
    }
    function commit() {
      if (committed) return;
      committed = true;
      const k = keyInput.value.trim();
      const v = valInput.value.trim();
      if (k) send({ type: "set_metadata", key: k, value: v || null });
      close();
    }

    keyInput.addEventListener("input", () => autoSize(keyInput));
    valInput.addEventListener("input", () => autoSize(valInput));

    keyInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        if (keyInput.value.trim()) valInput.focus();
        else commit();
      } else if (e.key === "Escape") {
        committed = true; // suppress blur-commit
        close();
      }
    });
    valInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        commit();
      } else if (e.key === "Escape") {
        committed = true;
        close();
      }
    });

    // Commit when focus leaves the chip entirely (clicking outside, tab away).
    chip.addEventListener("focusout", (e) => {
      const next = e.relatedTarget as Node | null;
      if (next && chip.contains(next)) return;
      commit();
    });

    chip.append(keyInput, valInput);
    return chip;
  }

  function render() {
    const s = store.get();
    strip.innerHTML = "";
    for (const [k, v] of Object.entries(s.metadata)) {
      if (editingKeys.has(k)) continue; // preserve in-flight edit
      strip.appendChild(buildExistingChip(k, v));
    }
    strip.appendChild(addOpen ? buildAddChip() : buildAddToggle());
  }

  render();
  store.subscribe((s) => s.metadata, render);
}
