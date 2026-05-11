//! Conservative markdown rendering for agent prose.
//!
//! Used by chat-mode answer bubbles. Renders a *subset* of markdown
//! (bold, italic, code spans, inline links, line breaks / paragraphs)
//! and strips everything else via a strict DOMPurify allowlist —
//! headers, lists, blockquotes, raw HTML are all turned back into
//! plain text. This keeps the visual surface consistent with the
//! Mac (SwiftUI AttributedString) and mobile (react-native-markdown-
//! display) renderers, which intentionally lack rich-block support.
//!
//! Defense-in-depth: marked emits HTML, DOMPurify enforces the
//! allowlist. Even if the agent's text contains a `<script>` tag,
//! DOMPurify strips it before it reaches the DOM.

import { marked, type MarkedOptions } from "marked";
import DOMPurify from "dompurify";

/// Tags we render. Anything else marked emits gets stripped by
/// DOMPurify (its content is preserved as text).
const ALLOWED_TAGS = ["strong", "em", "code", "a", "br", "p"] as const;
const ALLOWED_ATTR = ["href", "title"] as const;

/// Force links to open in a new tab + add rel=noopener noreferrer so
/// the new context can't reach back into the PWA via window.opener.
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A") {
    const el = node as HTMLAnchorElement;
    el.setAttribute("target", "_blank");
    el.setAttribute("rel", "noopener noreferrer");
  }
});

const MARKED_OPTIONS: MarkedOptions = {
  // GitHub-flavored line breaks: a single `\n` inside a paragraph
  // becomes `<br>`. The agent often emits these instead of full
  // paragraph breaks; without this they vanish.
  breaks: true,
  // Strict-ish parsing — disables some quirky lenient behaviors.
  gfm: true,
};

export function renderChatMarkdown(source: string): string {
  // `marked.parse` returns a string in sync mode (default).
  const rawHtml = marked.parse(source, MARKED_OPTIONS) as string;
  return DOMPurify.sanitize(rawHtml, {
    ALLOWED_TAGS: [...ALLOWED_TAGS],
    ALLOWED_ATTR: [...ALLOWED_ATTR],
    // KEEP_CONTENT default = true: stripped tags lose their wrapper
    // but their inner text survives, so a `# Header` becomes the
    // plain text "Header" rather than disappearing.
  });
}
