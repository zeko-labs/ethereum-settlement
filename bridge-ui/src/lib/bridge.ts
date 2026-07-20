import type {
  BridgeConfig,
  DepositActivity,
  EthereumBridgeClient,
  WithdrawalRequest,
  WithdrawalProof
} from "@zeko-labs/eth-bridge-sdk"
import type { Transaction as O1Transaction } from "o1js"
import type { Address, Hash } from "viem"
import { formatUnits } from "./amount"
import type { RuntimeConfig } from "./config"
import {
  ensureAuroPoCNetwork,
  getAuroProvider,
  type AuroProvider,
  type EthereumProvider,
  isProviderError
} from "./wallets"

type SdkModule = typeof import("@zeko-labs/eth-bridge-sdk")
type O1Module = typeof import("o1js")
type SignTransaction = (
  transaction: O1Transaction<boolean, false>
) => Promise<O1Transaction<boolean, true>>
let modulePromise: Promise<{ sdk: SdkModule; o1: O1Module }> | undefined

const DEPOSIT_PREPARATION_TIMEOUT_MS = 5 * 60 * 1_000

const isTransientDepositPreparationReason = (reason: string | null): boolean =>
  reason === "No outer commit available yet" ||
  reason === "No deposit witnesses found" ||
  reason?.includes("accepted but not confirmed yet") === true ||
  reason?.includes("not finalizable yet") === true

export const loadBridgeModules = () => {
  modulePromise ??= Promise.all([import("@zeko-labs/eth-bridge-sdk"), import("o1js")]).then(
    ([sdk, o1]) => ({ sdk, o1 })
  )
  return modulePromise
}

export const fetchGatewayConfig = async (config: RuntimeConfig): Promise<BridgeConfig> => {
  const { sdk } = await loadBridgeModules()
  return new sdk.GatewayClient(config.gatewayUrl).config()
}

export const isValidZekoAddress = async (address: string): Promise<boolean> => {
  try {
    const { o1 } = await loadBridgeModules()
    o1.PublicKey.fromBase58(address)
    return true
  } catch {
    return false
  }
}

export const buildZekoSdkConfig = (config: RuntimeConfig) => ({
  zekoUrl: config.sequencerGraphqlUrl,
  zekoArchiveUrl: config.zekoArchiveGraphqlUrl,
  actionsApi: config.actionsApiUrl,
  l1Network: config.minaSigningNetworkId,
  l2Network: config.minaSigningNetworkId,
  pollTimeout: 30 * 60 * 1_000,
  verbose: false,
  v2DepositsStartIndex: 0,
  v2WithdrawalsStartIndex: 0
})

export const createEthereumBridgeClient = async ({
  config,
  provider,
  account,
  withZeko = false
}: {
  config: RuntimeConfig
  provider: EthereumProvider
  account: Address
  withZeko?: boolean
}): Promise<EthereumBridgeClient> => {
  const { sdk } = await loadBridgeModules()
  return sdk.EthereumBridgeClient.init({
    gatewayUrl: config.gatewayUrl,
    provider,
    account,
    expectedChainId: config.expectedEthereumChainId,
    zeko: withZeko ? buildZekoSdkConfig(config) : undefined
  })
}

const parseAuroSignedCommand = (signedData: string): unknown => {
  const parsed: unknown = JSON.parse(signedData)
  if (typeof parsed !== "object" || parsed === null || !("zkappCommand" in parsed)) {
    throw new Error("Auro returned no signed zkApp command")
  }
  return (parsed as { zkappCommand: unknown }).zkappCommand
}

export const createAuroSigner = (
  provider: AuroProvider,
  config: RuntimeConfig
): SignTransaction => {
  const signer = async (
    transaction: Parameters<SignTransaction>[0]
  ): Promise<Awaited<ReturnType<SignTransaction>>> => {
    await ensureAuroPoCNetwork(provider, config)
    if (config.minaSigningNetworkId !== "testnet") {
      throw new Error("Auro PoC transactions must use the testnet signing domain")
    }
    const result = await provider.sendTransaction({
      onlySign: true,
      transaction: transaction.toJSON()
    })
    if (result instanceof Error) throw result
    if (isProviderError(result)) throw new Error(result.message ?? `Auro error ${result.code}`)
    if (!("signedData" in result)) throw new Error("Auro returned no signed transaction")
    const { o1 } = await loadBridgeModules()
    return o1.Transaction.fromJSON(
      parseAuroSignedCommand(result.signedData) as Parameters<typeof o1.Transaction.fromJSON>[0]
    ) as Awaited<ReturnType<SignTransaction>>
  }
  return signer
}

export const depositNative = async ({
  client,
  recipient,
  valueWei
}: {
  client: EthereumBridgeClient
  recipient: string
  valueWei: bigint
}) => {
  const { o1 } = await loadBridgeModules()
  return client.depositNative({ recipient: o1.PublicKey.fromBase58(recipient), valueWei })
}

export const finalizeDeposit = async ({
  client,
  recipient,
  config,
  provider = getAuroProvider()
}: {
  client: EthereumBridgeClient
  recipient: string
  config: RuntimeConfig
  provider?: AuroProvider
}): Promise<string> => {
  const { o1 } = await loadBridgeModules()
  const publicKey = o1.PublicKey.fromBase58(recipient)
  const deadline = Date.now() + DEPOSIT_PREPARATION_TIMEOUT_MS
  for (;;) {
    const preparation = await client.prepareDepositFinalization(publicKey)
    if (preparation.available) break
    if (!isTransientDepositPreparationReason(preparation.reason) || Date.now() >= deadline) {
      throw new Error(preparation.reason ?? "Deposit is not ready to finalize")
    }
    await new Promise((resolve) => window.setTimeout(resolve, config.pollIntervalMs))
  }
  return client.finalizeDeposit(publicKey, createAuroSigner(provider, config))
}

export const requestNativeWithdrawal = async ({
  client,
  sender,
  recipient,
  amount,
  config,
  provider = getAuroProvider()
}: {
  client: EthereumBridgeClient
  sender: string
  recipient: Address
  amount: bigint
  config: RuntimeConfig
  provider?: AuroProvider
}): Promise<string> => {
  const { o1 } = await loadBridgeModules()
  const publicKey = o1.PublicKey.fromBase58(sender)
  return client.requestWithdrawal({
    feePayer: { sender: publicKey, fee: config.zekoTransactionFeeNanomina },
    sender: publicKey,
    recipient,
    amount: o1.UInt64.from(amount),
    signTransaction: createAuroSigner(provider, config)
  })
}

export const listWalletActivity = async ({
  client,
  zekoRecipient,
  ethereumRecipient
}: {
  client: EthereumBridgeClient
  zekoRecipient?: string
  ethereumRecipient?: Address
}): Promise<{
  deposits: DepositActivity[]
  withdrawals: WithdrawalProof[]
  withdrawalRequests: WithdrawalRequest[]
}> => {
  const { o1 } = await loadBridgeModules()
  const depositsPromise = async () => {
    if (!zekoRecipient) return []
    const recipient = o1.PublicKey.fromBase58(zekoRecipient)
    const rows: DepositActivity[] = []
    let after: number | undefined
    for (;;) {
      const page = await client.listDepositsWithStates({ recipient, after, limit: 100 })
      rows.push(...page)
      if (page.length < 100) return rows
      const next = page.at(-1)?.nonce
      if (next === undefined || next === after) throw new Error("Deposit pagination did not advance")
      after = next
    }
  }
  const withdrawalsPromise = async () => {
    if (!ethereumRecipient) return []
    const rows: WithdrawalProof[] = []
    let after: number | undefined
    for (;;) {
      const page = await client.listWithdrawals({ recipient: ethereumRecipient, after, limit: 100 })
      rows.push(...page)
      if (page.length < 100) return rows
      const next = page.at(-1)?.globalActionIndex
      if (next === undefined || next === after) throw new Error("Withdrawal pagination did not advance")
      after = next
    }
  }
  const withdrawalRequestsPromise = async () => {
    if (!ethereumRecipient) return []
    const rows: WithdrawalRequest[] = []
    let after: number | undefined
    for (;;) {
      const page = await client.listWithdrawalRequests({ recipient: ethereumRecipient, after, limit: 100 })
      rows.push(...page)
      if (page.length < 100) return rows
      const next = page.at(-1)?.globalActionIndex
      if (next === undefined || next === after) throw new Error("Withdrawal request pagination did not advance")
      after = next
    }
  }
  const [deposits, withdrawals, withdrawalRequests] = await Promise.all([
    depositsPromise(),
    withdrawalsPromise(),
    withdrawalRequestsPromise()
  ])
  return { deposits, withdrawals, withdrawalRequests }
}

export const fetchEthereumBalance = async (
  provider: EthereumProvider,
  account: Address
): Promise<string> => {
  const value = await provider.request({ method: "eth_getBalance", params: [account, "latest"] })
  return formatUnits(BigInt(String(value)), 18, 5)
}

export const fetchZekoBalance = async (endpoint: string, account: string): Promise<string> => {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      query: `query AccountBalance($publicKey: PublicKey!, $tokenId: Field) {
        account(publicKey: $publicKey, tokenId: $tokenId) { balance { total } }
      }`,
      variables: { publicKey: account, tokenId: null }
    })
  })
  if (!response.ok) throw new Error(`Zeko balance request returned ${response.status}`)
  const body = (await response.json()) as {
    data?: { account?: { balance?: { total?: string | number } } }
    errors?: Array<{ message?: string }>
  }
  if (body.errors?.length) throw new Error(body.errors[0]?.message ?? "Zeko balance query failed")
  const total = body.data?.account?.balance?.total
  if (total === undefined) return "0"
  // Mina-compatible GraphQL reports the balance as whole native units.
  return typeof total === "number" ? total.toString() : total
}

export const ethereumTransactionUrl = (config: RuntimeConfig, hash: Hash | string): string =>
  `${config.ethereumExplorerUrl}/tx/${hash}`

export const zekoTransactionUrl = (config: RuntimeConfig, hash: string): string =>
  `${config.zekoExplorerUrl}/transactions/${hash}`
