import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

// Read package.json at config-resolve time so the version string is
// available as `import.meta.env.VITE_APP_VERSION` everywhere in the
// bundle. Lets UI surfaces (settings modal brand lockup, future
// "About" surfaces) display the real build version instead of a
// hardcoded literal that drifts with each release.
const pkg = JSON.parse(
  readFileSync(fileURLToPath(new URL("./package.json", import.meta.url)), "utf-8"),
) as { version: string };

export default defineConfig({
  base: "./",
  build: {
    target: "es2022",
    sourcemap: true,
  },
  define: {
    "import.meta.env.VITE_PROTOCOL_VERSION": JSON.stringify(1),
    "import.meta.env.VITE_APP_VERSION": JSON.stringify(pkg.version),
  },
  server: {
    // Bind to all network interfaces (0.0.0.0) so the dev server is
    // reachable from devices on the local network — specifically the
    // EvenHub QR install flow, where the glasses fetch the bundle
    // from the laptop's LAN IP via the EvenHub companion app. Without
    // this Vite binds to 127.0.0.1 only and the QR URL would return
    // "connection refused" from any non-laptop client.
    host: true,
    port: 5173,
    strictPort: true,
  },
});
