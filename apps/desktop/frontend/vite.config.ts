/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Node globals are available when Vite loads this file; @types/node is not installed.
declare const process: { env: Record<string, string | undefined> };

// https://vitejs.dev/config/
export default defineConfig(({ command, mode }) => {
  // The mock backend is for browser development, screenshots and unit tests
  // only. Refuse to produce a production bundle that could ever select it.
  if (command === "build" && mode === "production" && process.env.VITE_MOCK === "1") {
    throw new Error("VITE_MOCK=1 must not be set for a production build.");
  }
  return {
    server: {
      host: "127.0.0.1",
      port: 9245,
      strictPort: true,
    },
    plugins: [react(), tailwindcss()],
    test: {
      environment: "jsdom",
      globals: true,
      setupFiles: ["./src/test/setup.ts"],
      css: false,
    },
  };
});
