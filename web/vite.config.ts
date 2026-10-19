import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Port 5173 matches devUrl in src-tauri/tauri.conf.json.
export default defineConfig({
  plugins: [react()],
  server: { port: 5173, strictPort: true },
  clearScreen: false,
});
