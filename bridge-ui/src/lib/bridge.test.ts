import { beforeEach, describe, expect, it, vi } from "vitest"
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

import { createAuroSigner, createEthereumBridgeClient } from "./bridge"

const config = {
  schemaVersion: 1,
  gatewayUrl: "http://127.0.0.1:8080",
  sequencerGraphqlUrl: "http://127.0.0.1:1923/graphql",
  zekoArchiveGraphqlUrl: "http://127.0.0.1:1923/graphql",
  actionsApiUrl: "http://127.0.0.1:9101/graphql",
  expectedEthereumChainId: 11155111,
  minaSigningNetworkId: "testnet",
  auroNetworkName: "Zeko Ethereum PoC",
  zekoTransactionFeeNanomina: "100000000",
  ethereumExplorerUrl: "https://sepolia.etherscan.io",
  zekoExplorerUrl: "https://zekoscan.io/testnet",
  pollIntervalMs: 5000,
  maxDepositWei: "100000000000000000"
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
        zeko: expect.objectContaining({ l2Network: "testnet", v2DepositsStartIndex: 0 })
      })
    )
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
})
