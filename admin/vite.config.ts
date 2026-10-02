import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tanstackRouter from "@tanstack/router-plugin/vite";
import path from "node:path";

/**
 * A short id for the bundle this build produced.
 *
 * Shown in the sidebar so "did my change land" is a thing you read rather
 * than a thing you deduce: a stale service worker, a cached shell or a
 * half-finished deploy all look identical from the outside otherwise.
 */
const BUILD_ID = Date.now().toString(36).slice(-6);

export default defineConfig({
  define: {
    __VYASA_BUILD__: JSON.stringify(BUILD_ID),
  },
  plugins: [
    tanstackRouter(),
    react(),
    {
      name: "disable-rocket-loader",
      generateBundle(_, bundle) {
        for (const [name, chunk] of Object.entries(bundle)) {
          if (name === "index.html" && "source" in chunk) {
            const source = (chunk as { source: string }).source;
            (chunk as { source: string }).source = source.replace(
              '<script type="module"',
              '<script data-cfasync="false" type="module"',
            );
          }
        }
      },
    },
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: process.env.VYASA_API_URL ?? "http://127.0.0.1:3000",
        // Keep the browser's Host header: the API's CSRF check compares
        // Origin against Host, and rewriting Host made every mutation in
        // dev a 403.
        changeOrigin: false,
      },
    },
  },
});
