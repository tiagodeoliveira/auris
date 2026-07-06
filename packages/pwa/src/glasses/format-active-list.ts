import type { Item } from "../types";
import { wrapToLines } from "./wrap";

/// Inner text width (px) of the 576px-wide active body container minus
/// its 4px padding on each side. Wrapping at this width matches what the
/// firmware actually renders, so the scroll window's line model is exact.
const ACTIVE_BODY_INNER_WIDTH = 568;

/// Approximate width of one wrapped row of the firmware font inside
/// the active-list body container. The SDK doesn't expose font
/// metrics so this is an empirical undercount — better to send a
/// little less than overflow.
const WRAP_CHARS_PER_LINE = 56;

/// Approximate number of wrapped rows that visibly fit inside the
/// 256px-tall body container. Anything past this gets clipped off
/// the bottom by the firmware (top-down layout, no bottom-align).
/// Tightened to 8 so the latest items breathe — long meetings were
/// reading as a packed wall of text.
export const ACTIVE_LIST_VISIBLE_LINES = 8;

/// Render the active-meeting list as a bottom-anchored "tail" — we
/// always show the newest content, dropping older items off the top
/// as the budget fills. There is no scroll: the glasses screen is
/// too small to navigate history comfortably, and the phone already
/// has a real scrollable transcript. Items are taken newest-first
/// until the visual-line budget is exhausted, so the latest item is
/// ALWAYS visible.
export function formatActiveListBody(
  items: Item[],
  maxVisualLines: number,
  /// Live interim transcript from Soniox. When non-empty, it pins
  /// to the bottom row and consumes part of the budget.
  interim?: string,
): string {
  const showInterim = !!interim && interim.length > 0;
  const interimLines = showInterim ? estimateLines(interim ?? "") : 0;
  let remaining = Math.max(0, maxVisualLines - interimLines);

  // Walk newest-first, prepending items until we run out of budget.
  // Always include AT LEAST the newest item, even if it alone exceeds
  // the budget — better to clip one huge item than render a blank
  // screen during a meeting.
  const visible: string[] = [];
  for (let i = items.length - 1; i >= 0; i--) {
    const lines = estimateLines(items[i].text);
    if (visible.length > 0 && lines > remaining) break;
    visible.unshift(items[i].text);
    remaining -= lines;
  }
  if (showInterim) visible.push(interim ?? "");

  const content = visible.join("\n");
  // `textContainerUpgrade` with `""` doesn't clear the firmware
  // buffer — the prior render lingers. A single space forces an
  // overwrite so mode-switches into an empty mode actually blank
  // out the previous mode's content.
  return content.length > 0 ? content : " ";
}

/// The current mode's items wrapped to exact display rows, oldest first —
/// the document the scroll window slides across. Used for the scrollable
/// surfaces (summary / highlights), where the wearer can page back through
/// the whole list instead of only seeing the tail.
export function activeListDocLines(items: Item[]): string[] {
  return items.flatMap((it) => wrapToLines(it.text, ACTIVE_BODY_INNER_WIDTH));
}

/// The largest bottom-anchored line offset: 0 shows the newest rows (the
/// tail), the max scrolls all the way back to the oldest. Zero when the
/// whole list already fits in the visible-line budget.
export function activeListMaxOffset(items: Item[], maxVisualLines: number): number {
  return Math.max(0, activeListDocLines(items).length - maxVisualLines);
}

/// Windowed render of the current mode's items for a scrollable surface.
/// `offset` counts display rows UP from the newest (0 = tail, newest at the
/// bottom). The firmware lays out top-down with no bottom-align, so we emit
/// the `maxVisualLines` rows ending `offset` rows above the bottom — keeping
/// the newest row pinned to the bottom at offset 0, exactly like the tail.
export function formatActiveListWindow(
  items: Item[],
  maxVisualLines: number,
  offset: number,
): string {
  const lines = activeListDocLines(items);
  const max = Math.max(0, lines.length - maxVisualLines);
  const clamped = Math.min(Math.max(offset, 0), max);
  const end = lines.length - clamped;
  const start = Math.max(0, end - maxVisualLines);
  const content = lines.slice(start, end).join("\n");
  // Same firmware-clear sentinel as the tail formatter (see below).
  return content.length > 0 ? content : " ";
}

/// Estimate how many wrapped rows a single item will occupy. Counts
/// explicit newlines in the source text, then approximates word-wrap
/// for each. Always at least 1 row, even for empty strings (the
/// firmware reserves a line for blank entries).
function estimateLines(text: string): number {
  if (text.length === 0) return 1;
  let lines = 0;
  for (const segment of text.split("\n")) {
    lines += Math.max(1, Math.ceil(segment.length / WRAP_CHARS_PER_LINE));
  }
  return lines;
}
