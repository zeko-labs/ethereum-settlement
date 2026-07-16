import "@testing-library/jest-dom/vitest"

// Node 26 exposes an incomplete global localStorage unless a backing file is
// configured, so use a deterministic in-memory browser implementation.
const values = new Map<string, string>()
const testStorage: Storage = {
  get length() { return values.size },
  clear: () => values.clear(),
  getItem: (key) => values.get(key) ?? null,
  key: (index) => [...values.keys()][index] ?? null,
  removeItem: (key) => void values.delete(key),
  setItem: (key, value) => void values.set(key, value)
}
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: testStorage })
Object.defineProperty(window, "localStorage", { configurable: true, value: testStorage })

Object.defineProperty(HTMLMediaElement.prototype, "play", {
  configurable: true,
  value: async () => undefined
})
