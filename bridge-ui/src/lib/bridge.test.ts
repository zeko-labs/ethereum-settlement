import { beforeEach, describe, expect, it, vi } from "vitest"
import { MinaSnapProvider } from "@zeko-labs/mina-snap-provider"
import type { RuntimeConfig } from "./config"
import type { AuroProvider, EthereumProvider } from "./wallets"

const mocks = vi.hoisted(() => ({
  init: vi.fn(),
  fromJSON: vi.fn((value: unknown) => ({ signed: value }))
}))

vi.mock("@zeko-labs/eth-bridge-sdk", () => ({
  EthereumBridgeClient: { init: mocks.init },
  GatewayClient: class {}
}))

vi.mock("o1js", () => ({
  Transaction: { fromJSON: mocks.fromJSON },
  PublicKey: { fromBase58: vi.fn() },
  UInt64: { from: vi.fn() }
}))

import {
  createAuroSigner,
  createEthereumBridgeClient,
  finalizeDeposit,
  zekoTransactionUrl
} from "./bridge"

const config = {
  schemaVersion: 1,
  gatewayUrl: "http://127.0.0.1:8080",
  sequencerGraphqlUrl: "http://127.0.0.1:1923/graphql",
  zekoArchiveGraphqlUrl: "http://127.0.0.1:8080/archive/graphql",
  actionsApiUrl: "http://127.0.0.1:9101/graphql",
  expectedEthereumChainId: 11155111,
  minaSigningNetworkId: "testnet",
  auroNetworkName: "Zeko Ethereum PoC",
  zekoTransactionFeeNanomina: "2500",
  ethereumExplorerUrl: "https://sepolia.etherscan.io",
  zekoExplorerUrl: "https://zekoscan.io/testnet",
  pollIntervalMs: 5000
} satisfies RuntimeConfig

describe("SDK integration", () => {
  beforeEach(() => {
    mocks.init.mockReset()
    mocks.fromJSON.mockClear()
  })

  it("initializes the bridge SDK with the Auro-compatible testnet L2 network", async () => {
    const client = { account: "0xabc" }
    mocks.init.mockResolvedValue(client)
    const provider = { request: vi.fn() } as unknown as EthereumProvider
    await expect(
      createEthereumBridgeClient({
        config,
        provider,
        account: "0x0000000000000000000000000000000000000001",
        withZeko: true
      })
    ).resolves.toBe(client)
    expect(mocks.init).toHaveBeenCalledWith(
      expect.objectContaining({
        fetch: expect.any(Function),
        zeko: expect.objectContaining({
          l1Network: "testnet",
          l2Network: "testnet",
          v2DepositsStartIndex: 0
        })
      })
    )
    const fetcher = mocks.init.mock.calls[0]?.[0]?.fetch as typeof globalThis.fetch
    const response = new Response("{}", { headers: { "content-type": "application/json" } })
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(response)
    await fetcher("http://127.0.0.1:8080/v1/bridge/deposits/1")
    expect(fetchSpy).toHaveBeenCalledWith(
      "http://127.0.0.1:8080/v1/bridge/deposits/1",
      { cache: "no-store" }
    )
    fetchSpy.mockRestore()
  })

  it("converts an Auro onlySign response back into an o1js transaction", async () => {
    const provider = {
      addChain: vi.fn(async () => ({ networkID: "testnet" })),
      switchChain: vi.fn(async () => ({ networkID: "testnet" })),
      requestNetwork: vi.fn(async () => ({ networkID: "testnet" })),
      sendTransaction: vi.fn(async () => ({
        signedData: JSON.stringify({ zkappCommand: { feePayer: { body: { fee: "1" } } } })
      }))
    } as unknown as AuroProvider
    const signer = createAuroSigner(provider, config)
    const transaction = { toJSON: () => "{\"unsigned\":true}" }
    const signed = await signer(transaction as Parameters<typeof signer>[0])
    expect(provider.sendTransaction).toHaveBeenCalledWith({
      onlySign: true,
      transaction: "{\"unsigned\":true}"
    })
    expect(mocks.fromJSON).toHaveBeenCalledWith({ feePayer: { body: { fee: "1" } } })
    expect(signed).toEqual({ signed: { feePayer: { body: { fee: "1" } } } })
  })

  it("uses the MetaMask Snap through the same Auro onlySign bridge boundary", async () => {
    const request = vi.fn(async ({ method, params }: { method: string; params?: unknown }) => {
      if (method === "wallet_getSnaps") {
        return { "npm:@mondejka/mina-snap": { id: "npm:@mondejka/mina-snap" } }
      }
      if (method === "wallet_requestSnaps") {
        return { "npm:@mondejka/mina-snap": { id: "npm:@mondejka/mina-snap" } }
      }
      const minaRequest = (params as {
        request: { method: string }
      }).request
      if (minaRequest.method === "mina_requestNetwork") {
        return { networkID: "zeko:testnet" }
      }
      if (minaRequest.method === "mina_addChain") {
        return { networkID: "zeko:testnet" }
      }
      if (minaRequest.method === "mina_sendTransaction") {
        return {
          signedData: JSON.stringify({
            zkappCommand: { feePayer: { body: { fee: "1" } } }
          })
        }
      }
      throw new Error(`Unexpected Mina method ${minaRequest.method}`)
    })
    const provider = new MinaSnapProvider({ request }) as unknown as AuroProvider
    const signer = createAuroSigner(provider, config)

    await expect(signer({
      toJSON: () => "{\"unsigned\":true}"
    } as Parameters<typeof signer>[0])).resolves.toEqual({
      signed: { feePayer: { body: { fee: "1" } } }
    })
    expect(request).toHaveBeenLastCalledWith({
      method: "wallet_invokeSnap",
      params: {
        snapId: "npm:@mondejka/mina-snap",
        request: {
          method: "mina_sendTransaction",
          params: { onlySign: true, transaction: "{\"unsigned\":true}" }
        }
      }
    })
  })

  it("propagates a rejected Auro signature without parsing a transaction", async () => {
    const provider = {
      addChain: vi.fn(async () => ({ networkID: "testnet" })),
      switchChain: vi.fn(async () => ({ networkID: "testnet" })),
      requestNetwork: vi.fn(async () => ({ networkID: "testnet" })),
      sendTransaction: vi.fn(async () => ({ code: 1002, message: "User rejected signing" }))
    } as unknown as AuroProvider
    const signer = createAuroSigner(provider, config)
    const transaction = { toJSON: () => "{\"unsigned\":true}" }
    await expect(signer(transaction as Parameters<typeof signer>[0])).rejects.toThrow("User rejected signing")
    expect(mocks.fromJSON).not.toHaveBeenCalled()
  })

  it("retries deposit preparation while the synchronized outer commit reaches the actions indexer", async () => {
    vi.useFakeTimers()
    const publicKey = { toBase58: () => "B62recipient" }
    vi.mocked((await import("o1js")).PublicKey.fromBase58).mockReturnValue(
      publicKey as never
    )
    const client = {
      prepareDepositFinalization: vi.fn()
        .mockResolvedValueOnce({ available: false, reason: "No outer commit available yet" })
        .mockResolvedValueOnce({ available: true, reason: null, index: 17 }),
      finalizeDeposit: vi.fn().mockResolvedValue("5Jfinalized")
    }
    const provider = {
      requestNetwork: vi.fn(async () => ({ networkID: "testnet" }))
    } as unknown as AuroProvider

    const result = finalizeDeposit({
      client: client as never,
      recipient: "B62recipient",
      config,
      provider
    })
    await vi.advanceTimersByTimeAsync(config.pollIntervalMs)

    await expect(result).resolves.toBe("5Jfinalized")
    expect(client.prepareDepositFinalization).toHaveBeenCalledTimes(2)
    expect(client.finalizeDeposit).toHaveBeenCalledWith(publicKey, expect.any(Function), {
      attempts: 3,
      feeNanomina: BigInt(config.zekoTransactionFeeNanomina)
    })
    vi.useRealTimers()
  })

  it("does not retry a non-transient deposit preparation rejection", async () => {
    const client = {
      prepareDepositFinalization: vi.fn().mockResolvedValue({
        available: false,
        reason: "No finalizable deposit found"
      })
    }
    const provider = {
      requestNetwork: vi.fn(async () => ({ networkID: "testnet" }))
    } as unknown as AuroProvider

    await expect(finalizeDeposit({
      client: client as never,
      recipient: "B62recipient",
      config,
      provider
    })).rejects.toThrow("No finalizable deposit found")
    expect(client.prepareDepositFinalization).toHaveBeenCalledTimes(1)
  })

  it("rebuilds a finalization whose account precondition became stale", async () => {
    vi.useFakeTimers()
    const publicKey = { toBase58: () => "B62recipient" }
    vi.mocked((await import("o1js")).PublicKey.fromBase58).mockReturnValue(publicKey as never)
    const client = {
      prepareDepositFinalization: vi.fn().mockResolvedValue({ available: true, reason: null }),
      finalizeDeposit: vi.fn()
        .mockRejectedValueOnce(new Error("Account_app_state_precondition_unsatisfied"))
        .mockResolvedValueOnce("5Jrebuilt")
    }
    const provider = { requestNetwork: vi.fn(async () => ({ networkID: "testnet" })) } as unknown as AuroProvider

    const result = finalizeDeposit({ client: client as never, recipient: "B62recipient", config, provider })
    await vi.advanceTimersByTimeAsync(config.pollIntervalMs)

    await expect(result).resolves.toBe("5Jrebuilt")
    expect(client.finalizeDeposit).toHaveBeenCalledTimes(2)
    vi.useRealTimers()
  })

  it("uses the explorer's canonical transaction detail route", () => {
    expect(zekoTransactionUrl(config, "5Jtransaction")).toBe(
      "https://zekoscan.io/testnet/transactions/5Jtransaction"
    )
  })
})
