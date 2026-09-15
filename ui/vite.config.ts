import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

// The Rust binary serves the built SPA from /assets/ (rust-embed on ui/dist).
// Dev: `npm run dev` proxies API/SSE calls to a `cargo run` instance.
export default defineConfig({
  plugins: [preact()],
  base: "/assets/",
  build: {
    outDir: "dist",
    // files land directly in dist/ so the served URL is /assets/index-*.js
    assetsDir: "",
    target: "es2022",
    sourcemap: false,
  },
  server: {
    port: 5173,
    proxy: {
      "/api": { target: "http://127.0.0.1:9920", ws: false },
      "/logs/export": { target: "http://127.0.0.1:9920" },
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
