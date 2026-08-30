import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "esnext",
    rollupOptions: {
      input: {
        dashboard: resolve(__dirname, "dashboard.html"),
        floating: resolve(__dirname, "floating.html"),
        settings: resolve(__dirname, "settings.html"),
      },
    },
  },
});
