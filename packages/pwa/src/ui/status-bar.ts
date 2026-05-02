import type { Store } from "../store";

export function mountStatusBar(parent: HTMLElement, store: Store, onSettings: () => void): void {
  const el = document.createElement("div");
  el.className = "status-bar";

  const wsDot = dot();
  const bleDot = dot();
  const pill = document.createElement("span");
  pill.className = "pill";
  const gear = document.createElement("button");
  gear.className = "gear";
  gear.textContent = "⚙";
  gear.addEventListener("click", onSettings);

  el.append(wsDot, label("WS"), bleDot, label("BLE"), pill, gear);
  parent.appendChild(el);

  function render() {
    const s = store.get();
    wsDot.className = "dot " + dotColor(s.wsStatus);
    bleDot.className = "dot " + (s.bleConnected ? "green" : "");
    pill.textContent = `State: ${s.meetingState}`;
  }

  render();
  store.subscribe((s) => s.wsStatus, render);
  store.subscribe((s) => s.bleConnected, render);
  store.subscribe((s) => s.meetingState, render);
}

function dot() {
  const d = document.createElement("span");
  d.className = "dot";
  return d;
}
function label(text: string) {
  const s = document.createElement("span");
  s.textContent = text;
  s.style.color = "var(--fg-dim)";
  s.style.fontSize = "12px";
  return s;
}
function dotColor(status: string): string {
  if (status === "open") return "green";
  if (status === "reconnecting" || status === "connecting") return "yellow";
  if (status === "error" || status === "closed") return "red";
  return "";
}
