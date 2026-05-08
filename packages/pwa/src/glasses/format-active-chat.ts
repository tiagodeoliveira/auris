//! Glasses-specific renderer for chat mode. Unlike `formatActiveListBody`
//! (which truncates each item to a single 60-char line), chat is a
//! flowing thread where the assistant's reply often runs several
//! lines. The list-style truncation drops 90% of a typical answer.
//!
//! Strategy:
//!   - Word-wrap each item across as many lines as needed.
//!   - Prefix user turns with `▶ ` (so the eye lands on the question);
//!     assistant turns with two spaces (no cursor).
//!   - Strip light markdown noise (`**bold**` markers) — glasses
//!     don't render bold, so the asterisks become visual clutter.
//!   - Pin to the bottom of the buffer: only the last
//!     `linesPerScreen` lines render. The latest exchange stays
//!     visible; older history scrolls off the top until the user
//!     uses scroll-up gestures to walk back (future work).

import type { Item } from "../types";

export function formatActiveChatBody(
  items: Item[],
  charsPerLine: number,
  linesPerScreen: number,
): string {
  if (items.length === 0) return "";
  const lines: string[] = [];
  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    const role = (item.meta as Record<string, unknown> | null | undefined)?.role ?? "assistant";
    const isUser = role === "user";
    const prefix = isUser ? "▶ " : "  ";
    const cleaned = stripMarkdownNoise(item.text);
    const wrapped = wrapText(cleaned, Math.max(1, charsPerLine - prefix.length));
    for (let w = 0; w < wrapped.length; w++) {
      lines.push((w === 0 ? prefix : "  ") + wrapped[w]);
    }
    // Blank separator between turns; skip after the last item.
    if (i < items.length - 1) lines.push("");
  }
  // Bottom-pin: the latest exchange should always be visible.
  return lines.slice(-linesPerScreen).join("\n");
}

/// Word-wrap `text` to lines of at most `width` chars, splitting on
/// whitespace. Words longer than `width` are hard-split (rare —
/// happens for URLs / unbroken tokens).
export function wrapText(text: string, width: number): string[] {
  if (width <= 0) return [text];
  // Split text into paragraphs first so explicit newlines are
  // preserved (markdown bullet lists, multi-paragraph answers).
  const paragraphs = text.split("\n");
  const out: string[] = [];
  for (const p of paragraphs) {
    if (p.length === 0) {
      out.push("");
      continue;
    }
    const words = p.split(/\s+/).filter((w) => w.length > 0);
    let current = "";
    for (const w of words) {
      if (w.length > width) {
        // Flush current, then hard-split the long word.
        if (current.length > 0) {
          out.push(current);
          current = "";
        }
        for (let i = 0; i < w.length; i += width) {
          const chunk = w.slice(i, i + width);
          if (chunk.length === width) {
            out.push(chunk);
          } else {
            current = chunk;
          }
        }
        continue;
      }
      if (current.length === 0) {
        current = w;
      } else if (current.length + 1 + w.length <= width) {
        current += " " + w;
      } else {
        out.push(current);
        current = w;
      }
    }
    if (current.length > 0) out.push(current);
  }
  return out;
}

/// Drop common markdown markers that the glasses can't render. Bold
/// `**text**` shows as literal asterisks; we strip them. Other
/// markers (italic `*text*`, headings `#`, code spans `` ` ``) are
/// rare in chat answers and left alone for now.
function stripMarkdownNoise(s: string): string {
  return s.replace(/\*\*/g, "");
}
