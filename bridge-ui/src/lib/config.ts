export type RuntimeConfig = {
  schemaVersion: 1
  gatewayUrl: string
  sequencerGraphqlUrl: string
  zekoArchiveGraphqlUrl: string
  actionsApiUrl: string
  expectedEthereumChainId: number
  minaSigningNetworkId: "testnet"
  auroNetworkName: string
  zekoTransactionFeeNanomina: string
  ethereumExplorerUrl: string
  zekoExplorerUrl: string
  pollIntervalMs: number
}

export const ethereumNetworkName = (chainId: number): string =>
  chainId === 31_337 ? "Local Ethereum" : "Sepolia"

const asRecord = (value: unknown, label: string): Record<string, unknown> => {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value as Record<string, unknown>
}

const requiredString = (row: Record<string, unknown>, key: string): string => {
  const value = row[key]
  if (typeof value !== "string" || value.length === 0) throw new Error(`Invalid ${key}`)
  return value
}

const requiredUrl = (row: Record<string, unknown>, key: string): string => {
  const value = requiredString(row, key)
  const url = new URL(value)
  if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error(`Invalid ${key}`)
  return value.replace(/\/$/, "")
}

const requiredInteger = (
  row: Record<string, unknown>,
  key: string,
  { min = 0, max = Number.MAX_SAFE_INTEGER }: { min?: number; max?: number } = {}
): number => {
  const value = row[key]
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < min || value > max) {
    throw new Error(`Invalid ${key}`)
  }
  return value
}

const requiredUintString = (row: Record<string, unknown>, key: string): string => {
  const value = requiredString(row, key)
  if (!/^(0|[1-9][0-9]*)$/.test(value) || BigInt(value) > 2n ** 64n - 1n) {
    throw new Error(`Invalid ${key}`)
  }
  return value
}

export const parseRuntimeConfig = (value: unknown): RuntimeConfig => {
  const row = asRecord(value, "runtime config")
  if (row.schemaVersion !== 1) throw new Error("Unsupported runtime config schema")
  if (row.minaSigningNetworkId !== "testnet") {
    throw new Error('The Auro PoC requires minaSigningNetworkId "testnet"')
  }

  const fee = requiredUintString(row, "zekoTransactionFeeNanomina")
  const chainId = requiredInteger(row, "expectedEthereumChainId", { min: 1 })
  if (chainId !== 11_155_111 && chainId !== 31_337) {
    throw new Error("This PoC requires Ethereum Sepolia or local chain 31337")
  }

  return {
    schemaVersion: 1,
    gatewayUrl: requiredUrl(row, "gatewayUrl"),
    sequencerGraphqlUrl: requiredUrl(row, "sequencerGraphqlUrl"),
    zekoArchiveGraphqlUrl: requiredUrl(row, "zekoArchiveGraphqlUrl"),
    actionsApiUrl: requiredUrl(row, "actionsApiUrl"),
    expectedEthereumChainId: chainId,
    minaSigningNetworkId: "testnet",
    auroNetworkName: requiredString(row, "auroNetworkName"),
    zekoTransactionFeeNanomina: fee,
    ethereumExplorerUrl: requiredUrl(row, "ethereumExplorerUrl"),
    zekoExplorerUrl: requiredUrl(row, "zekoExplorerUrl"),
    pollIntervalMs: requiredInteger(row, "pollIntervalMs", { min: 1_000, max: 60_000 })
  }
}

export const loadRuntimeConfig = async (fetcher: typeof fetch = fetch): Promise<RuntimeConfig> => {
  const response = await fetcher("/runtime-config.json", {
    cache: "no-store",
    headers: { accept: "application/json" }
  })
  if (!response.ok) throw new Error(`Runtime configuration returned ${response.status}`)
  return parseRuntimeConfig(await response.json())
}
