//! Shared modal chrome — small round icon buttons for the close
//! and reload affordances that every modal in the PWA needs. Lives
//! here so the four modal files (meetings, artifacts, quick-asks,
//! settings) inherit one consistent look + position instead of
//! growing their own outlined-pill / bottom-link / no-button
//! treatments.
//!
//! Buttons are constructed via createElementNS so the SVG glyphs
//! are real SVG trees rather than innerHTML strings (matches the
//! pattern used by top-bar.ts).

const SVG_NS = "http://www.w3.org/2000/svg";

/// Round ✕ button. Place near the modal's top-right corner; CSS
/// (`.modal-close-btn`) handles the visual chrome and hover state.
export function makeCloseButton(onClose: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "modal-close-btn";
  b.setAttribute("aria-label", "Close");
  b.title = "Close";
  b.appendChild(makeCloseIcon());
  b.addEventListener("click", onClose);
  return b;
}

/// Round refresh button. Place to the left of the close button on
/// modals that show server-side data the user can pull fresh
/// (meetings, artifacts). Quick-asks + settings don't need it.
export function makeReloadButton(onReload: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "modal-reload-btn";
  b.setAttribute("aria-label", "Reload");
  b.title = "Reload";
  b.appendChild(makeReloadIcon());
  b.addEventListener("click", onReload);
  return b;
}

function makeCloseIcon(): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 20 20");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("aria-hidden", "true");
  for (const [x1, y1, x2, y2] of [
    [6, 6, 14, 14],
    [14, 6, 6, 14],
  ] as const) {
    const line = document.createElementNS(SVG_NS, "line");
    line.setAttribute("x1", String(x1));
    line.setAttribute("y1", String(y1));
    line.setAttribute("x2", String(x2));
    line.setAttribute("y2", String(y2));
    svg.appendChild(line);
  }
  return svg;
}

function makeReloadIcon(): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 20 20");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.6");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  // Curved arrow tracing ~270° of a circle with an arrowhead.
  const arc = document.createElementNS(SVG_NS, "path");
  arc.setAttribute("d", "M15.5 10 A5.5 5.5 0 1 1 13.6 5.9");
  svg.appendChild(arc);
  const head = document.createElementNS(SVG_NS, "polyline");
  head.setAttribute("points", "13.8 3.2 13.8 6.1 10.9 6.1");
  svg.appendChild(head);
  return svg;
}
