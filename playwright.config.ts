import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  use: {
    channel: "chrome",
    baseURL: "http://127.0.0.1:1431",
    viewport: { width: 1180, height: 800 },
    trace: "retain-on-failure",
  },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1 --port 1431",
    url: "http://127.0.0.1:1431",
    reuseExistingServer: !process.env.CI,
  },
});
