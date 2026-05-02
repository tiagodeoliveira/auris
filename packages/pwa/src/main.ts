// Boot sequence lands in Task 6 once the bridge wrapper, store, and
// glasses orchestrator exist. Keep this file minimal so `vite dev`
// renders an empty placeholder during early-task development.

const app = document.querySelector<HTMLDivElement>("#app");
if (app) {
  app.textContent = "Meeting Companion (booting…)";
}
