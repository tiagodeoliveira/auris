import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  build: {
    target: "es2022",
    sourcemap: true,
  },
  define: {
    "import.meta.env.VITE_PROTOCOL_VERSION": JSON.stringify(1),
  },
  server: {
    port: 5173,
    strictPort: true,
  },
});
