// Dev server via Bun's fullstack HTTP server (replaces `vite`). Serves the HTML
// entrypoint with React fast-refresh HMR and bundles TSX/CSS/assets on the fly.
// Tauri points `devUrl` at http://localhost:1420 and launches this through the
// `beforeDevCommand` hook, so the port is fixed.
//
// TAURI_DEV_HOST (set by `tauri android/ios dev`) makes the server bind the LAN
// address so a physical device can reach the dev bundle; otherwise localhost.
import { serve } from "bun";
import index from "../index.html";

const host = process.env.TAURI_DEV_HOST;

const server = serve({
  port: 1420,
  hostname: host || "localhost",
  // Serve the SPA for every route; the bundler injects the HMR client.
  routes: { "/*": index },
  development: { hmr: true, console: true },
  // Fixed port is required by Tauri — surface a clear error if it's taken
  // instead of silently falling back to another port.
  error(err) {
    console.error(err);
    return new Response("dev server error", { status: 500 });
  },
});

console.log(`frontend dev server running at ${server.url}`);
