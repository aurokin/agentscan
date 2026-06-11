import { defineConfig } from "vitest/config";

// The Effect service tests are pure logic (a mock TauriIpc layer, no DOM/Tauri),
// so a node environment is enough. The two .test.tsx mount smoke tests opt into
// jsdom per-file via // @vitest-environment jsdom. Kept separate from
// vite.config.ts so the Tauri build config stays focused on the app bundle.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
