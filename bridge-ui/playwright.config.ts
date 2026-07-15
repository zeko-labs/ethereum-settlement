import { defineConfig, devices } from "@playwright/test"

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  retries: 0,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:4174",
    trace: "retain-on-failure"
  },
  webServer: {
    command: "pnpm preview",
    url: "http://127.0.0.1:4174",
    reuseExistingServer: true
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "mobile-chromium", use: { ...devices["Pixel 7"] } }
  ]
})
