import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vitest/config";

// Port 5173 matches devUrl in src-tauri/tauri.conf.json.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: { port: 5173, strictPort: true },
  clearScreen: false,
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
