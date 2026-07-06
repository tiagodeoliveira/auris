//! Quick Asks editor modal. List the user's library + edit/add/delete.
//! Position is assigned by creation order; no drag-reorder in v1.
//!
//! Read from `itemsByMode["quick_asks"]` (server's items_update keeps
//! it canonical). Writes go via `Intent::UpsertQuickAsk` /
//! `Intent::DeleteQuickAsk`; the server broadcasts back and the list
//! re-renders.

import type { Store } from "../store";
import type { Intent, Item } from "../types";
import { makeCloseButton } from "./modal-chrome";

const QUICK_ASKS_MODE = "quick_asks";

function newId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return `qa-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

interface EditingState {
  id: string;
  label: string;
  text: string;
  position: number;
  isNew: boolean;
}

export function mountQuickAsksModal(
  parent: HTMLElement,
  store: Store,
  send: (intent: Intent) => void,
): void {
  const overlay = document.createElement("div");
  overlay.className = "settings-overlay";
  parent.appendChild(overlay);

  const modal = document.createElement("div");
  modal.className = "settings-modal settings-modal-v2";
  overlay.appendChild(modal);

  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) store.update({ quickAsksModalOpen: false });
  });

  // Header row — title on the left, shared ✕ on the right. The
  // ✕ uses the same `modal-close-btn` chrome the meetings /
  // artifacts / settings modals use so all four read as one
  // family.
  const header = document.createElement("div");
  header.className = "modal-header-row";
  const title = document.createElement("h2");
  title.textContent = "Quick Asks";
  title.style.margin = "0";
  const closeBtn = makeCloseButton(() => store.update({ quickAsksModalOpen: false }));
  header.append(title, closeBtn);
  modal.appendChild(header);

  const subtitle = document.createElement("p");
  subtitle.textContent =
    "Saved prompts you can fire into the meeting chat with one tap — also picked from the glasses by mode.";
  subtitle.className = "settings-card-helper";
  modal.appendChild(subtitle);

  // Editor form (hidden by default, shown when an item is being edited).
  let editing: EditingState | null = null;
  const formCard = document.createElement("section");
  formCard.className = "settings-card";
  formCard.style.display = "none";

  const labelRow = document.createElement("label");
  labelRow.style.display = "block";
  labelRow.style.marginBottom = "8px";
  labelRow.textContent = "Label";
  const labelInput = document.createElement("input");
  labelInput.type = "text";
  labelInput.maxLength = 40;
  labelInput.placeholder = "Short mnemonic (e.g. Action items)";
  labelInput.style.width = "100%";
  labelInput.style.marginTop = "4px";
  labelRow.appendChild(labelInput);

  const textRow = document.createElement("label");
  textRow.style.display = "block";
  textRow.style.marginBottom = "8px";
  textRow.textContent = "Prompt";
  const textArea = document.createElement("textarea");
  textArea.rows = 6;
  textArea.placeholder = "Multiline; markdown OK. This is what gets sent to chat.";
  textArea.style.width = "100%";
  textArea.style.marginTop = "4px";
  textArea.style.fontFamily = "var(--font-mono)";
  textRow.appendChild(textArea);

  const buttonRow = document.createElement("div");
  buttonRow.style.display = "flex";
  buttonRow.style.gap = "8px";
  buttonRow.style.marginTop = "8px";

  const saveBtn = document.createElement("button");
  saveBtn.type = "button";
  saveBtn.textContent = "Save";
  saveBtn.className = "btn-primary";

  const cancelBtn = document.createElement("button");
  cancelBtn.type = "button";
  cancelBtn.textContent = "Cancel";
  cancelBtn.className = "btn-ghost";

  const deleteBtn = document.createElement("button");
  deleteBtn.type = "button";
  deleteBtn.textContent = "Delete";
  deleteBtn.className = "btn-ghost-destructive";
  deleteBtn.style.marginLeft = "auto";

  buttonRow.append(saveBtn, cancelBtn, deleteBtn);
  formCard.append(labelRow, textRow, buttonRow);
  modal.appendChild(formCard);

  const listCard = document.createElement("section");
  listCard.className = "settings-card";
  modal.appendChild(listCard);

  const addBtn = document.createElement("button");
  addBtn.type = "button";
  addBtn.textContent = "+ Add quick ask";
  addBtn.className = "btn-primary";
  addBtn.style.marginBottom = "12px";
  listCard.appendChild(addBtn);

  const list = document.createElement("ul");
  list.style.listStyle = "none";
  list.style.padding = "0";
  list.style.margin = "0";
  listCard.appendChild(list);

  function openEditor(state: EditingState): void {
    editing = state;
    labelInput.value = state.label;
    textArea.value = state.text;
    deleteBtn.style.display = state.isNew ? "none" : "";
    formCard.style.display = "";
    listCard.style.display = "none";
    setTimeout(() => labelInput.focus(), 0);
  }

  function closeEditor(): void {
    editing = null;
    formCard.style.display = "none";
    listCard.style.display = "";
  }

  addBtn.addEventListener("click", () => {
    const items = store.get().itemsByMode[QUICK_ASKS_MODE] ?? [];
    // Position = max + 10 (gaps for cheap future reorders).
    const maxPos = items.reduce((acc, it) => Math.max(acc, Number(it.t) || 0), 0);
    openEditor({
      id: newId(),
      label: "",
      text: "",
      position: maxPos + 10,
      isNew: true,
    });
  });

  saveBtn.addEventListener("click", () => {
    if (!editing) return;
    const label = labelInput.value.trim();
    const text = textArea.value.trim();
    if (!label || !text) return;
    send({
      type: "upsert_quick_ask",
      id: editing.id,
      label,
      text,
      position: editing.position,
    });
    closeEditor();
  });

  cancelBtn.addEventListener("click", closeEditor);

  deleteBtn.addEventListener("click", () => {
    if (!editing || editing.isNew) return;
    send({ type: "delete_quick_ask", id: editing.id });
    closeEditor();
  });

  function renderList(items: Item[]): void {
    while (list.firstChild) list.removeChild(list.firstChild);
    if (items.length === 0) {
      const empty = document.createElement("p");
      empty.className = "settings-card-helper";
      empty.textContent = "No quick asks yet. Add one above.";
      list.appendChild(empty);
      return;
    }
    for (const it of items) {
      const row = document.createElement("li");
      row.style.padding = "10px 0";
      row.style.borderTop = "1px solid var(--color-border)";
      row.style.cursor = "pointer";

      const labelLine = document.createElement("div");
      labelLine.style.fontWeight = "600";
      labelLine.textContent = it.text; // server packs label in text

      const previewLine = document.createElement("div");
      previewLine.style.color = "var(--color-text-secondary)";
      previewLine.style.fontSize = "0.9em";
      previewLine.style.marginTop = "2px";
      const preview = (it.detail ?? "").split("\n")[0]?.slice(0, 80) ?? "";
      previewLine.textContent = preview;

      row.append(labelLine, previewLine);
      row.addEventListener("click", () => {
        openEditor({
          id: it.id,
          label: it.text,
          text: it.detail ?? "",
          position: Number(it.t) || 0,
          isNew: false,
        });
      });
      list.appendChild(row);
    }
  }

  function syncVisibility(): void {
    const s = store.get();
    // Match the settings/meetings/artifacts modal convention: an
    // `.open` class flips the CSS-driven display. Inline
    // `style.display` doesn't work here because the base
    // `.settings-overlay` rule sets `display: none` by default.
    overlay.classList.toggle("open", s.quickAsksModalOpen);
    if (!s.quickAsksModalOpen) {
      closeEditor();
    }
  }
  syncVisibility();
  store.subscribe((s) => s.quickAsksModalOpen, syncVisibility);

  function refresh(): void {
    renderList(store.get().itemsByMode[QUICK_ASKS_MODE] ?? []);
  }
  refresh();
  store.subscribe((s) => s.itemsByMode[QUICK_ASKS_MODE], refresh);
}
