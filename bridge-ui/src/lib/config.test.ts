import { describe, expect, it } from "vitest"
import { parseRuntimeConfig } from "./config"

const validConfig = {
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
  pollIntervalMs: 5000
}

describe("runtime configuration", () => {
  it("accepts the PoC testnet signing domain", () => {
    expect(parseRuntimeConfig(validConfig)).toMatchObject({
      minaSigningNetworkId: "testnet",
      expectedEthereumChainId: 11155111
    })
  })

  it("accepts the local manual-flow chain", () => {
    expect(parseRuntimeConfig({ ...validConfig, expectedEthereumChainId: 31_337 }))
      .toMatchObject({ expectedEthereumChainId: 31_337 })
  })

  it("rejects custom Auro signing salts", () => {
    expect(() => parseRuntimeConfig({ ...validConfig, minaSigningNetworkId: "zeko-testnet" })).toThrow(
      /requires minaSigningNetworkId "testnet"/
    )
  })

  it.each([
    ["schemaVersion", 2],
    ["pollIntervalMs", 10],
    ["gatewayUrl", "file:///tmp/gateway"]
  ])("rejects invalid %s", (key, value) => {
    expect(() => parseRuntimeConfig({ ...validConfig, [key]: value })).toThrow()
  })
})
