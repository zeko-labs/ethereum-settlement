import type { RuntimeConfig } from "../lib/config"

export const validConfig: RuntimeConfig = {
  schemaVersion: 1,
  gatewayUrl: "http://127.0.0.1:8080",
  sequencerGraphqlUrl: "http://127.0.0.1:1923/graphql",
  zekoArchiveGraphqlUrl: "http://127.0.0.1:8080/archive/graphql",
  actionsApiUrl: "http://127.0.0.1:9101/graphql",
  expectedEthereumChainId: 11155111,
  minaSigningNetworkId: "testnet",
  auroNetworkName: "Zeko Ethereum PoC",
  zekoTransactionFeeNanomina: "100000000",
  ethereumExplorerUrl: "https://sepolia.etherscan.io",
  zekoExplorerUrl: "https://zekoscan.io/testnet",
  pollIntervalMs: 5000
}
