import type {
  EthereumAssetRecord,
  EthereumAssetRegistrySnapshot,
  EthereumBridgeClient
} from "@zeko-labs/eth-bridge-sdk"
import {
  createPublicClient,
  createWalletClient,
  custom,
  getAddress,
  isAddress,
  type Address,
  type Hex
} from "viem"
import { formatUnits } from "./amount"
import type { EthereumProvider } from "./wallets"

export type NativeBridgeAsset = {
  kind: "native"
  id: "native"
  name: "Ether"
  symbol: "ETH"
  ethereumDecimals: 18
  zekoDecimals: 9
}

export type TokenBridgeAsset = {
  kind: "erc20"
  id: Address
  name: string
  symbol: string
  ethereumDecimals: number
  zekoDecimals: number
  token: Address
  assetId: Hex
  tokenIdL2: Hex
  inventoryCap: bigint
}

export type BridgeAsset = NativeBridgeAsset | TokenBridgeAsset

export const NATIVE_ASSET: NativeBridgeAsset = {
  kind: "native",
  id: "native",
  name: "Ether",
  symbol: "ETH",
  ethereumDecimals: 18,
  zekoDecimals: 9
}

const erc20Abi = [
  {
    type: "function",
    name: "name",
    stateMutability: "view",
    inputs: [],
    outputs: [{ type: "string" }]
  },
  {
    type: "function",
    name: "symbol",
    stateMutability: "view",
    inputs: [],
    outputs: [{ type: "string" }]
  },
  {
    type: "function",
    name: "decimals",
    stateMutability: "view",
    inputs: [],
    outputs: [{ type: "uint8" }]
  },
  {
    type: "function",
    name: "balanceOf",
    stateMutability: "view",
    inputs: [{ name: "account", type: "address" }],
    outputs: [{ type: "uint256" }]
  },
  {
    type: "function",
    name: "allowance",
    stateMutability: "view",
    inputs: [{ name: "owner", type: "address" }, { name: "spender", type: "address" }],
    outputs: [{ type: "uint256" }]
  },
  {
    type: "function",
    name: "approve",
    stateMutability: "nonpayable",
    inputs: [{ name: "spender", type: "address" }, { name: "amount", type: "uint256" }],
    outputs: [{ type: "bool" }]
  }
] as const

const asRecord = (value: unknown, label: string): Record<string, unknown> => {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value as Record<string, unknown>
}

const integer = (value: unknown, label: string): number => {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Error(`Invalid ${label}`)
  }
  return value
}

const string = (value: unknown, label: string): string => {
  if (typeof value !== "string" || value.length === 0) throw new Error(`Invalid ${label}`)
  return value
}

const word = (value: unknown, label: string): Hex => {
  const result = string(value, label)
  if (!/^0x[0-9a-fA-F]{64}$/.test(result)) throw new Error(`Invalid ${label}`)
  return result.toLowerCase() as Hex
}

const parseAssetRecord = (value: unknown): EthereumAssetRecord => {
  const row = asRecord(value, "Ethereum asset record")
  const token = string(row.ethereumTokenAddress, "ethereumTokenAddress")
  if (!isAddress(token)) throw new Error("Invalid ethereumTokenAddress")
  const cap = string(row.inventoryCap, "inventoryCap")
  if (!/^[1-9][0-9]*$/.test(cap)) throw new Error("Invalid inventoryCap")
  return {
    schemaVersion: integer(row.schemaVersion, "schemaVersion"),
    registryIndex: integer(row.registryIndex, "registryIndex"),
    assetId: word(row.assetId, "assetId"),
    ethereumTokenAddress: getAddress(token),
    tokenOwnerL2: word(row.tokenOwnerL2, "tokenOwnerL2"),
    tokenIdL2: word(row.tokenIdL2, "tokenIdL2"),
    decimals: integer(row.decimals, "decimals"),
    inventoryCap: BigInt(cap),
    mftStandardVkId: word(row.mftStandardVkId, "mftStandardVkId"),
    vaultPublicKey: word(row.vaultPublicKey, "vaultPublicKey"),
    universalBridgeVkId: word(row.universalBridgeVkId, "universalBridgeVkId")
  }
}

const sameAssetRecord = (left: EthereumAssetRecord, right: EthereumAssetRecord): boolean =>
  left.schemaVersion === right.schemaVersion &&
  left.registryIndex === right.registryIndex &&
  left.assetId.toLowerCase() === right.assetId.toLowerCase() &&
  left.ethereumTokenAddress.toLowerCase() === right.ethereumTokenAddress.toLowerCase() &&
  left.tokenOwnerL2.toLowerCase() === right.tokenOwnerL2.toLowerCase() &&
  left.tokenIdL2.toLowerCase() === right.tokenIdL2.toLowerCase() &&
  left.decimals === right.decimals &&
  left.inventoryCap === right.inventoryCap &&
  left.mftStandardVkId.toLowerCase() === right.mftStandardVkId.toLowerCase() &&
  left.vaultPublicKey.toLowerCase() === right.vaultPublicKey.toLowerCase() &&
  left.universalBridgeVkId.toLowerCase() === right.universalBridgeVkId.toLowerCase()

export const parseAssetRegistrySnapshot = (value: unknown): EthereumAssetRegistrySnapshot => {
  const row = asRecord(value, "Ethereum asset registry")
  if (!Array.isArray(row.records)) throw new Error("Invalid Ethereum asset registry records")
  return {
    schemaVersion: integer(row.schemaVersion, "registry schemaVersion"),
    root: word(row.root, "registry root"),
    count: integer(row.count, "registry count"),
    depth: integer(row.depth, "registry depth"),
    records: row.records.map((value, index) => {
      const entry = asRecord(value, `Ethereum asset registry record ${index}`)
      if (!Array.isArray(entry.path)) throw new Error(`Invalid registry path ${index}`)
      return {
        record: parseAssetRecord(entry.record),
        path: entry.path.map((value, pathIndex) => word(value, `registry path ${index}:${pathIndex}`))
      }
    })
  }
}

export const fetchAssetRegistrySnapshot = async (
  actionsApiUrl: string,
  fetcher: typeof fetch = fetch
): Promise<EthereumAssetRegistrySnapshot | null> => {
  const response = await fetcher(actionsApiUrl, {
    method: "POST",
    cache: "no-store",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      query: `query EthereumAssetRegistry {
        ethereumAssetRegistry {
          schemaVersion root count depth
          records {
            path
            record {
              schemaVersion registryIndex assetId ethereumTokenAddress tokenOwnerL2 tokenIdL2
              decimals inventoryCap mftStandardVkId vaultPublicKey universalBridgeVkId
            }
          }
        }
      }`
    })
  })
  if (!response.ok) throw new Error(`Asset registry request returned ${response.status}`)
  const body = asRecord(await response.json(), "Asset registry response")
  if (Array.isArray(body.errors) && body.errors.length > 0) {
    const first = asRecord(body.errors[0], "Asset registry GraphQL error")
    throw new Error(typeof first.message === "string" ? first.message : "Asset registry query failed")
  }
  const data = asRecord(body.data, "Asset registry data")
  return data.ethereumAssetRegistry === null
    ? null
    : parseAssetRegistrySnapshot(data.ethereumAssetRegistry)
}

export const discoverBridgeAssets = async (
  client: EthereumBridgeClient,
  provider: EthereumProvider
): Promise<{ assets: BridgeAsset[]; warnings: string[] }> => {
  const registry = client.assetRegistry
  if (!registry) return { assets: [NATIVE_ASSET], warnings: [] }
  const publicClient = createPublicClient({ transport: custom(provider) })
  const warnings: string[] = []
  const tokens = await Promise.all(Array.from({ length: registry.count }, async (_, index) => {
    const { record } = registry.byIndex(index)
    try {
      const [proposal, registration, name, symbol, decimals] = await Promise.all([
        client.getAssetProposal(record.ethereumTokenAddress),
        client.getTokenRegistration(record.ethereumTokenAddress),
        publicClient.readContract({ abi: erc20Abi, address: record.ethereumTokenAddress, functionName: "name" }),
        publicClient.readContract({ abi: erc20Abi, address: record.ethereumTokenAddress, functionName: "symbol" }),
        publicClient.readContract({ abi: erc20Abi, address: record.ethereumTokenAddress, functionName: "decimals" })
      ])
      const matches = proposal.status === "active" &&
        sameAssetRecord(proposal.record, record) &&
        registration.registered && registration.allowed &&
        registration.assetId.toLowerCase() === record.assetId.toLowerCase() &&
        registration.ethereumDecimals === record.decimals &&
        registration.zekoDecimals === record.decimals &&
        registration.depositCap.toBigInt() === record.inventoryCap &&
        decimals === record.decimals
      if (!matches) throw new Error("registry and Ethereum activation do not agree")
      if (!name.trim() || !symbol.trim() || symbol.length > 16) throw new Error("invalid ERC20 metadata")
      return {
        kind: "erc20",
        id: record.ethereumTokenAddress,
        token: record.ethereumTokenAddress,
        assetId: record.assetId,
        tokenIdL2: record.tokenIdL2,
        name: name.trim(),
        symbol: symbol.trim(),
        ethereumDecimals: record.decimals,
        zekoDecimals: record.decimals,
        inventoryCap: record.inventoryCap
      } satisfies TokenBridgeAsset
    } catch (error) {
      warnings.push(`${record.ethereumTokenAddress}: ${error instanceof Error ? error.message : String(error)}`)
      return undefined
    }
  }))
  return { assets: [NATIVE_ASSET, ...tokens.filter((asset): asset is TokenBridgeAsset => asset !== undefined)], warnings }
}

export const fetchTokenBalance = async (
  provider: EthereumProvider,
  account: Address,
  asset: TokenBridgeAsset
): Promise<string> => {
  const client = createPublicClient({ transport: custom(provider) })
  const balance = await client.readContract({
    abi: erc20Abi,
    address: asset.token,
    functionName: "balanceOf",
    args: [account]
  })
  return formatUnits(balance, asset.ethereumDecimals, Math.min(asset.ethereumDecimals, 6))
}

export const ensureTokenAllowance = async ({
  provider,
  account,
  token,
  spender,
  amount
}: {
  provider: EthereumProvider
  account: Address
  token: Address
  spender: Address
  amount: bigint
}): Promise<Hex | undefined> => {
  const publicClient = createPublicClient({ transport: custom(provider) })
  const allowance = await publicClient.readContract({
    abi: erc20Abi,
    address: token,
    functionName: "allowance",
    args: [account, spender]
  })
  if (allowance >= amount) return undefined
  const walletClient = createWalletClient({ account, transport: custom(provider) })
  const hash = await walletClient.writeContract({
    abi: erc20Abi,
    account,
    address: token,
    functionName: "approve",
    args: [spender, amount],
    chain: null
  })
  const receipt = await publicClient.waitForTransactionReceipt({ hash })
  if (receipt.status !== "success") throw new Error(`ERC20 approval ${hash} reverted`)
  return hash
}
