import { describe, expect, it, vi } from "vitest"
import {
  announceMinaProvider,
  discoverMetaMaskProvider,
  MinaSnapProvider
} from "./index"

describe("Auro-compatible Mina Provider", () => {
  it("installs the Snap and maps Auro methods to wallet_invokeSnap", async () => {
    const request = vi.fn(async ({ method }: { method: string }) => {
      if (method === "wallet_getSnaps") return {}
      if (method === "wallet_requestSnaps") return {}
      if (method === "wallet_invokeSnap") {
        return ["B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA"]
      }
      throw new Error(`Unexpected ${method}`)
    })
    const provider = new MinaSnapProvider({ request }, {
      snapId: "local:http://127.0.0.1:8080"
    })

    await expect(provider.requestAccounts()).resolves.toEqual([
      "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA"
    ])
    expect(request).toHaveBeenNthCalledWith(2, {
      method: "wallet_requestSnaps",
      params: {
        "local:http://127.0.0.1:8080": {}
      }
    })
    expect(request).toHaveBeenLastCalledWith({
      method: "wallet_invokeSnap",
      params: {
        snapId: "local:http://127.0.0.1:8080",
        request: { method: "mina_requestAccounts" }
      }
    })
  })

  it("requests an exact audited npm Snap version", async () => {
    const request = vi.fn(async ({ method }: { method: string }) => {
      if (method === "wallet_getSnaps") return {}
      if (method === "wallet_requestSnaps") return {}
      if (method === "wallet_invokeSnap") return []
      throw new Error(`Unexpected ${method}`)
    })
    const provider = new MinaSnapProvider({ request })

    await provider.getAccounts()

    expect(request).toHaveBeenNthCalledWith(2, {
      method: "wallet_requestSnaps",
      params: {
        "npm:@zeko-labs/mina-snap": { version: "0.1.0" }
      }
    })
  })

  it("preserves the bridge's Auro onlySign request and response", async () => {
    const request = vi.fn(async ({ method }: { method: string }) => {
      if (method === "wallet_getSnaps") {
        return { "npm:@zeko-labs/mina-snap": { id: "npm:@zeko-labs/mina-snap" } }
      }
      return { signedData: "{\"zkappCommand\":{}}" }
    })
    const provider = new MinaSnapProvider({ request })

    await expect(provider.sendTransaction({
      onlySign: true,
      transaction: "{\"feePayer\":{}}"
    })).resolves.toEqual({ signedData: "{\"zkappCommand\":{}}" })
    expect(request).toHaveBeenLastCalledWith({
      method: "wallet_invokeSnap",
      params: {
        snapId: "npm:@zeko-labs/mina-snap",
        request: {
          method: "mina_sendTransaction",
          params: { onlySign: true, transaction: "{\"feePayer\":{}}" }
        }
      }
    })
  })

  it("maps MetaMask rejection errors to Auro's provider codes", async () => {
    const request = vi.fn(async ({ method }: { method: string }) => {
      if (method === "wallet_getSnaps") {
        return { "npm:@zeko-labs/mina-snap": { id: "npm:@zeko-labs/mina-snap" } }
      }
      throw Object.assign(new Error("User rejected the request"), { code: 4001 })
    })
    const provider = new MinaSnapProvider({ request })

    await expect(provider.signMessage({ message: "no" })).rejects.toMatchObject({
      code: 1002,
      message: "User rejected the request"
    })
  })

  it("maps standard Snap authorization and parameter errors to Auro codes", async () => {
    let failure = Object.assign(new Error("Connect the Mina account before signing"), {
      code: 4100
    })
    const request = vi.fn(async ({ method }: { method: string }) => {
      if (method === "wallet_getSnaps") {
        return { "npm:@zeko-labs/mina-snap": { id: "npm:@zeko-labs/mina-snap" } }
      }
      throw failure
    })
    const provider = new MinaSnapProvider({ request })

    await expect(provider.signMessage({ message: "no" })).rejects.toMatchObject({ code: 1001 })
    failure = Object.assign(new Error("Invalid parameters"), { code: -32602 })
    await expect(provider.signMessage({ message: "no" })).rejects.toMatchObject({ code: 20003 })
  })

  it("selects MetaMask through EIP-6963 when another wallet owns window.ethereum", () => {
    const target = new EventTarget() as EventTarget & { ethereum?: unknown }
    const metamask = { request: vi.fn() }
    target.ethereum = { request: vi.fn(), name: "another wallet" }
    target.addEventListener("eip6963:requestProvider", () => {
      target.dispatchEvent(new CustomEvent("eip6963:announceProvider", {
        detail: {
          info: { rdns: "io.metamask", name: "MetaMask", uuid: "metamask", icon: "data:image/svg+xml,<svg/>" },
          provider: metamask
        }
      }))
    })

    expect(discoverMetaMaskProvider(target)).toBe(metamask)
  })

  it("selects MetaMask Flask through EIP-6963 for local Snap development", () => {
    const target = new EventTarget() as EventTarget & { ethereum?: unknown }
    const flask = { request: vi.fn() }
    target.ethereum = { request: vi.fn(), name: "another wallet" }
    target.addEventListener("eip6963:requestProvider", () => {
      target.dispatchEvent(new CustomEvent("eip6963:announceProvider", {
        detail: {
          info: { rdns: "io.metamask.flask" },
          provider: flask
        }
      }))
    })

    expect(discoverMetaMaskProvider(target)).toBe(flask)
  })

  it("does not treat an unidentified injected wallet as MetaMask", () => {
    const target = new EventTarget() as EventTarget & { ethereum?: unknown }
    target.ethereum = { request: vi.fn(), name: "another wallet" }

    expect(discoverMetaMaskProvider(target)).toBeUndefined()
  })

  it("announces a frozen Mina provider without impersonating Auro", () => {
    const target = new EventTarget()
    const provider = new MinaSnapProvider({ request: vi.fn() })
    let announcement: CustomEvent | undefined
    target.addEventListener("mina:announceProvider", (event) => {
      announcement = event as CustomEvent
    })

    const cleanup = announceMinaProvider(target, provider)
    target.dispatchEvent(new Event("mina:requestProvider"))

    expect(announcement?.detail).toMatchObject({
      info: { slug: "zeko-mina-snap", name: "Zeko Mina Snap" },
      provider: { isAuro: false, isMinaSnap: true }
    })
    expect(Object.isFrozen(announcement?.detail)).toBe(true)
    cleanup()
  })

  it("rehydrates nullifier field elements as bigints like Auro", async () => {
    const request = vi.fn(async ({ method }: { method: string }) => {
      if (method === "wallet_getSnaps") {
        return { "npm:@zeko-labs/mina-snap": { id: "npm:@zeko-labs/mina-snap" } }
      }
      return {
        publicKey: { x: "1", y: "2" },
        public: { nullifier: { x: "3", y: "4" }, s: "5" },
        private: {
          c: "6",
          g_r: { x: "7", y: "8" },
          h_m_pk_r: { x: "9", y: "10" }
        }
      }
    })
    const provider = new MinaSnapProvider({ request })

    await expect(provider.createNullifier({ message: [1] })).resolves.toMatchObject({
      publicKey: { x: 1n, y: 2n },
      public: { s: 5n }
    })
  })
})
