import babel from "@rolldown/plugin-babel";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { createHash } from "node:crypto";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";

/**
 * A strict Content Security Policy for the built app. Scripts run only from the app's own files
 * and the inline theme script (by hash); connections may go to any server, because people choose
 * their own. Hosts should also send `frame-ancestors 'none'` as a header (meta cannot).
 */
function csp(): Plugin {
  return {
    name: "pando-csp",
    apply: "build",
    transformIndexHtml(html) {
      const hashes = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => `'sha256-${createHash("sha256").update(m[1]!).digest("base64")}'`);
      const policy = [
        "default-src 'none'",
        `script-src 'self' ${hashes.join(" ")}`,
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' data:",
        "font-src 'self'",
        "connect-src *",
        "worker-src 'self'",
        "manifest-src 'self'",
        "base-uri 'none'",
        "form-action 'none'",
        "object-src 'none'",
      ].join("; ");
      return html.replace("<head>", `<head>\n    <meta http-equiv="Content-Security-Policy" content="${policy}" />`);
    },
  };
}

export default defineConfig({
  // Relative asset paths: the same build serves from any directory (GitHub Pages, a CDN, or pandod).
  base: "./",
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), csp()],
  worker: { format: "es" },
  build: { target: "es2023", sourcemap: true },
  test: { environment: "jsdom", include: ["src/**/*.test.{ts,tsx}"] },
});
