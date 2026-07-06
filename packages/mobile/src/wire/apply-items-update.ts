// Merge an incoming `items_update` payload into the current list for
// a mode, respecting that mode's declared `update_strategy`. Mirrors
// `packages/pwa/src/glasses/apply-items-update.ts` byte-for-byte so
// cross-client behavior stays in lockstep — diverging here is how
// chat history fell out of sync between Mac/PWA and mobile before
// (mobile was always replacing, which clobbered Append-mode chat
// threads on every new Q+A pair).
//
// `replace` (highlights, summary): server sends the full list every
// time; client overwrites.
// `append` (chat, transcript, actions, open_questions): server sends
// only the delta; client upserts by id (new items push, existing ids
// update in-place so an item can be edited).
import type { Item, ModeOption } from "./contract";

export function applyItemsUpdate(current: Item[], incoming: Item[], mode: ModeOption): Item[] {
  if (mode.update_strategy === "replace") {
    return incoming.slice();
  }
  const next = current.slice();
  for (const item of incoming) {
    const idx = next.findIndex((i) => i.id === item.id);
    if (idx >= 0) next[idx] = item;
    else next.push(item);
  }
  return next;
}
