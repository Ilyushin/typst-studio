import { defineConfig } from "vite";

export default defineConfig({
  // Tauri serves the frontend from this port in development.
  server: { port: 5173, strictPort: true },
  build: { target: "es2022" },
});
