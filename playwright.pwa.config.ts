import { defineConfig, devices } from "@playwright/test";

const port = Number(process.env.PWA_PORT ?? 4173);
const baseURL = `http://127.0.0.1:${port}`;
const channel = process.env.PLAYWRIGHT_CHANNEL;

export default defineConfig({
  testDir: "./e2e-pwa",
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [
    ["list"],
    ["html", { outputFolder: "playwright-report-pwa", open: "never" }],
  ],
  outputDir: "test-results-pwa",
  use: {
    baseURL,
    ...(channel ? { channel } : {}),
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  // The PWA spec starts and stops scripts/serve-pages.mjs itself, so it can
  // take the server down before the offline reload.
});
