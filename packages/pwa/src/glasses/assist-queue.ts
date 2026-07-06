//! Pure derivation for the assist popup queue. Given the full
//! assist-mode items list and the ledger of ids already popped (or
//! currently popping), return the next item that should be shown —
//! or null if the queue is empty.
//!
//! Kept pure so the main-loop subscriber stays a thin wrapper and
//! the queue invariants are unit-testable without spinning up a
//! store / event bus.

import type { Item } from "../types";

/// Returns the first assist item whose id is not in the shown
/// ledger. The items list is server-emitted and append-only (assist
/// mode uses `UpdateStrategy::Append`), so "first" here is also
/// "oldest unseen" — which is the desired FIFO ordering for a
/// notification queue.
export function nextAssistToShow(items: Item[], shownIds: string[]): Item | null {
  const seen = new Set(shownIds);
  for (const item of items) {
    if (!seen.has(item.id)) return item;
  }
  return null;
}
