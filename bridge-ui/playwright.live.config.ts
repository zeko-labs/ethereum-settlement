import { defineConfig, devices } from "@playwright/test"

declare const process: { env: Record<string, string | undefined> }
const artifactRoot = process.env.BRIDGE_E2E_ARTIFACT_DIR

export default defineConfig({
  testDir: "./live-e2e",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 45 * 60 * 1_000,
  expect: { timeout: 2 * 60 * 1_000 },
  reporter: [["list"], ["html", {
    outputFolder: artifactRoot ? `${artifactRoot}/report` : "test-results/live-report",
    open: "never"
  }]],
  outputDir: artifactRoot ? `${artifactRoot}/playwright` : "test-results/live-artifacts",
  use: {
    baseURL: process.env.BRIDGE_E2E_BRIDGE_UI_URL ?? "http://127.0.0.1:4174",
    trace: {
      mode: "retain-on-failure",
      screenshots: false,
      snapshots: true,
      sources: true
    },
    screenshot: "only-on-failure",
    video: "retain-on-failure"
  },
  projects: [{ name: "live-chromium", use: { ...devices["Desktop Chrome"] } }]
})
