import { defineConfig, devices } from "@playwright/test";

const ci = !!process.env.CI;

export default defineConfig({
  testDir: "e2e",
  globalSetup: "./e2e/setup.ts",
  fullyParallel: false,
  retries: ci ? 1 : 0,
  reporter: ci ? [["github"], ["list"]] : "list",
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "retain-on-failure",
    // Locally use the installed Chrome; CI installs Playwright's Chromium.
    ...(ci ? devices["Desktop Chrome"] : { ...devices["Desktop Chrome"], channel: "chrome" }),
  },
  webServer: {
    command: "pnpm exec vite preview --port 4173 --strictPort --host 127.0.0.1",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: !ci,
  },
});
