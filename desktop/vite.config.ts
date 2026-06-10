import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST ?? "127.0.0.1";

export default defineConfig({
  plugins: [react()],
  build: {
    // Tauri loads the bundle from disk, so Vite's web-oriented 500 kB payload
    // warning doesn't apply; keep a higher ceiling as a dependency-bloat canary.
    chunkSizeWarningLimit: 700,
  },
  clearScreen: false,
  server: {
    host,
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
