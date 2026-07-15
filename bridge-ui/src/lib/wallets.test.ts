import { describe, expect, it, vi } from "vitest"
import type { RuntimeConfig } from "./config"
import {
  ensureAuroPoCNetwork,
  ensureEthereumNetwork,
  formatWalletError,
  listenAuroChanges,
  listenEthereumChanges,
  type AuroProvider,
  type EthereumProvider
} from "./wallets"

const config = {
  minaSigningNetworkId: "testnet",
  sequencerGraphqlUrl: "http://127.0.0.1:1923/graphql",
  auroNetworkName: "Zeko Ethereum PoC"
} as RuntimeConfig

describe("wallet adapters", () => {
  it("adds the custom Auro endpoint and accepts only the testnet signing domain", async () => {
    const provider = {
      addChain: vi.fn(async () => ({ networkID: "testnet" })),
      switchChain: vi.fn(async () => ({ networkID: "testnet" })),
      requestNetwork: vi.fn(async () => ({ networkID: "testnet" }))
    } as unknown as AuroProvider
    await ensureAuroPoCNetwork(provider, config)
    expect(provider.addChain).toHaveBeenCalledWith({
      url: config.sequencerGraphqlUrl,
      name: config.auroNetworkName
    })
    expect(provider.switchChain).toHaveBeenCalledWith({ networkID: "testnet" })
  })

  it("rejects a wallet that reports another Auro signing domain", async () => {
    const provider = {
      addChain: vi.fn(async () => ({ networkID: "testnet" })),
      switchChain: vi.fn(async () => ({ networkID: "testnet" })),
      requestNetwork: vi.fn(async () => ({ networkID: "zeko-testnet" }))
    } as unknown as AuroProvider
    await expect(ensureAuroPoCNetwork(provider, config)).rejects.toThrow(/did not select/)
  })

  it("switches an injected Ethereum wallet to the expected chain", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce("0x1")
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce("0xaa36a7")
    await ensureEthereumNetwork({ request } as unknown as EthereumProvider, 11155111)
    expect(request).toHaveBeenNthCalledWith(2, {
      method: "wallet_switchEthereumChain",
      params: [{ chainId: "0xaa36a7" }]
    })
  })

  it("adds local Anvil when the wallet does not know chain 31337", async () => {
    const request = vi.fn()
      .mockResolvedValueOnce("0x1")
      .mockRejectedValueOnce({ code: 4902 })
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce("0x7a69")
    await ensureEthereumNetwork({ request } as unknown as EthereumProvider, 31_337)
    expect(request).toHaveBeenNthCalledWith(3, {
      method: "wallet_addEthereumChain",
      params: [expect.objectContaining({ chainId: "0x7a69", rpcUrls: ["http://127.0.0.1:8545"] })]
    })
  })

  it("forwards Ethereum account and chain changes and unregisters listeners", () => {
    const listeners = new Map<string, (value: unknown) => void>()
    const provider = {
      on: vi.fn((event: string, listener: (value: unknown) => void) => listeners.set(event, listener)),
      removeListener: vi.fn()
    } as unknown as EthereumProvider
    const onAccounts = vi.fn()
    const onChain = vi.fn()
    const cleanup = listenEthereumChanges(provider, { onAccounts, onChain })
    listeners.get("accountsChanged")?.(["0xabc"])
    listeners.get("chainChanged")?.("0xaa36a7")
    expect(onAccounts).toHaveBeenCalledWith(["0xabc"])
    expect(onChain).toHaveBeenCalledWith(11155111)
    cleanup()
    expect(provider.removeListener).toHaveBeenCalledTimes(2)
  })

  it("forwards Auro changes and invalidates listeners on cleanup", () => {
    const listeners = new Map<string, (value: unknown) => void>()
    const provider = {
      on: vi.fn((event: string, listener: (value: unknown) => void) => listeners.set(event, listener)),
      removeAllListeners: vi.fn()
    } as unknown as AuroProvider
    const onAccounts = vi.fn()
    const onNetwork = vi.fn()
    const cleanup = listenAuroChanges(provider, { onAccounts, onNetwork })
    listeners.get("accountsChanged")?.(["B62-account"])
    listeners.get("chainChanged")?.({ networkID: "testnet" })
    expect(onAccounts).toHaveBeenCalledWith(["B62-account"])
    expect(onNetwork).toHaveBeenCalledWith("testnet")
    cleanup()
    expect(provider.removeAllListeners).toHaveBeenCalledTimes(2)
  })

  it("turns wallet rejections into actionable copy", () => {
    expect(formatWalletError(new Error("User rejected the request"))).toBe("The wallet request was rejected.")
  })
})
