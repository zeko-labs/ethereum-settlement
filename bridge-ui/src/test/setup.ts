import "@testing-library/jest-dom/vitest"

Object.defineProperty(HTMLMediaElement.prototype, "play", {
  configurable: true,
  value: async () => undefined
})
