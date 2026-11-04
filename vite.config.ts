import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  // janela principal + janelinha de abertura (splash)
  build: { rollupOptions: { input: { main: "index.html", splash: "splash.html" } } },
});
