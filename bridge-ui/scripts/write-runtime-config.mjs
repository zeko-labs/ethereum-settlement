import { mkdir, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"

const env = process.env

const integer = (name, fallback, { min = 0, max = Number.MAX_SAFE_INTEGER } = {}) => {
  const raw = env[name] ?? fallback
  const value = Number(raw)
  if (!Number.isSafeInteger(value) || value < min || value > max) {
    throw new Error(`${name} must be an integer between ${min} and ${max}`)
  }
  return value
}

const uintString = (name, fallback) => {
  const value = env[name] ?? fallback
  if (!/^(0|[1-9][0-9]*)$/.test(value) || BigInt(value) > 2n ** 64n - 1n) {
    throw new Error(`${name} must be an unsigned 64-bit decimal string`)
  }
  return value
}

const url = (name, fallback) => {
  const value = env[name] ?? fallback
  const parsed = new URL(value)
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(`${name} must use http or https`)
  }
  return value.replace(/\/$/, "")
}

const chainId = integer("BRIDGE_UI_ETHEREUM_CHAIN_ID", "11155111", { min: 1 })
if (chainId !== 11155111 && chainId !== 31337) {
  throw new Error("The browser PoC supports Sepolia or local chain 31337")
}

const config = {
  schemaVersion: 1,
  gatewayUrl: url("BRIDGE_UI_GATEWAY_URL", "http://127.0.0.1:8080"),
  sequencerGraphqlUrl: url("BRIDGE_UI_SEQUENCER_GRAPHQL_URL", "http://127.0.0.1:1923/graphql"),
  zekoArchiveGraphqlUrl: url("BRIDGE_UI_ZEKO_ARCHIVE_GRAPHQL_URL", "http://127.0.0.1:1923/graphql"),
  actionsApiUrl: url("BRIDGE_UI_ACTIONS_API_URL", "http://127.0.0.1:9101/graphql"),
  expectedEthereumChainId: chainId,
  minaSigningNetworkId: "testnet",
  auroNetworkName: env.BRIDGE_UI_AURO_NETWORK_NAME ?? "Zeko Ethereum PoC",
  zekoTransactionFeeNanomina: uintString("BRIDGE_UI_ZEKO_FEE_NANOMINA", "100000000"),
  ethereumExplorerUrl: url("BRIDGE_UI_ETHEREUM_EXPLORER_URL", "https://sepolia.etherscan.io"),
  zekoExplorerUrl: url("BRIDGE_UI_ZEKO_EXPLORER_URL", "https://zekoscan.io/testnet"),
  pollIntervalMs: integer("BRIDGE_UI_POLL_INTERVAL_MS", "5000", { min: 1000, max: 60000 })
}

if (config.auroNetworkName.length === 0) throw new Error("BRIDGE_UI_AURO_NETWORK_NAME must not be empty")

const [destination] = process.argv.slice(2).filter((argument) => argument !== "--")
const serialized = `${JSON.stringify(config, null, 2)}\n`
if (destination === "--stdout") {
  process.stdout.write(serialized)
} else {
  const output = resolve(destination ?? "public/runtime-config.json")
  await mkdir(dirname(output), { recursive: true })
  await writeFile(output, serialized)
  process.stdout.write(`Wrote public bridge runtime config to ${output}\n`)
}
