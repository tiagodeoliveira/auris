import type { Item } from "../types";

export function formatActiveListBody(
  items: Item[],
  highlightIndex: number,
  viewportStart: number,
  linesPerScreen: number,
  charsPerLine: number,
): string {
  const visible = items.slice(viewportStart, viewportStart + linesPerScreen);
  return visible
    .map((item, offset) => {
      const idx = viewportStart + offset;
      const cursor = idx === highlightIndex ? "▶ " : "  ";
      const maxText = Math.max(0, charsPerLine - 2);
      const text = truncate(item.text, maxText);
      return cursor + text;
    })
    .join("\n");
}

export function truncate(s: string, max: number): string {
  if (s.length <= max) return s;
  if (max <= 1) return s.slice(0, max);
  return s.slice(0, max - 1) + "…";
}
