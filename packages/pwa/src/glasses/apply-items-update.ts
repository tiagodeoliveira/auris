import type { Item, ModeOption } from "../types";

export function applyItemsUpdate(current: Item[], incoming: Item[], mode: ModeOption): Item[] {
  if (mode.update_strategy === "replace") {
    return incoming.slice();
  }
  // append: upsert by id
  const next = current.slice();
  for (const item of incoming) {
    const idx = next.findIndex((i) => i.id === item.id);
    if (idx >= 0) next[idx] = item;
    else next.push(item);
  }
  return next;
}
