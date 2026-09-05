import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Config de Vite para Tauri: puerto fijo 1420 (tauri.conf.json apunta ahí),
// y evitamos que Vite se meta con archivos que cambia Rust al compilar.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
