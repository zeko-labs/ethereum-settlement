import react from "@vitejs/plugin-react"
import { defineConfig } from "vitest/config"

export default defineConfig({
  plugins: [react()],
  server: { host: "127.0.0.1", port: 5174 },
  preview: { host: "127.0.0.1", port: 4174 },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts",
    exclude: ["tests/**", "live-e2e/**", "node_modules/**", "dist/**"],
    css: true
  }
})
