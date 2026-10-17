import babel from "@rolldown/plugin-babel";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  // Relative asset paths: the same build serves from any directory (GitHub Pages, a CDN, or pandod).
  base: "./",
  plugins: [react(), babel({ presets: [reactCompilerPreset()] })],
  worker: { format: "es" },
  build: { target: "es2023", sourcemap: true },
  test: { environment: "jsdom", include: ["src/**/*.test.{ts,tsx}"] },
});
