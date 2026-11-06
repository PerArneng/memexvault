import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // No data: URLs, which the server's CSP refuses: small fonts stay files like the rest.
  build: { assetsInlineLimit: 0 },
  server: { host: "127.0.0.1", proxy: { "/api": process.env.MEMEX_API_URL ?? "http://127.0.0.1:8080" } },
  preview: { host: "127.0.0.1" },
});
