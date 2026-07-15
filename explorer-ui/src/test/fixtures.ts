import type { RuntimeConfig } from "../lib/runtime"

export const runtimeConfig: RuntimeConfig = {
  schemaVersion: 1,
  gatewayUrl: "http://gateway.test",
  bridgeUiUrl: "http://bridge.test",
  ethereumExplorerUrl: "https://sepolia.etherscan.io",
  networkName: "Zeko Testnet",
  pollIntervalMs: 5000
}

export const summary = {
  schemaVersion: 1,
  asOf: "2026-07-15T15:00:00Z",
  sources: { archive: true, gateway: true, ethereum: true, sequencer: true },
  l2: { blockHeight: "18492", transactionCount: "18491", accountCount: "9007199254740993" },
  settlement: {
    latestSequence: "284",
    commitSchedule: {
      periodSeconds: 900,
      phase: "WAITING" as const,
      lastAttemptStartedAt: "2026-07-15T14:52:30Z",
      nextAttemptAt: "2026-07-15T15:07:30Z",
    },
  },
  bridge: { depositCount: "147", withdrawalCount: "39", depositedAmount: "24820000000000000000" }
}

export const transaction = {
  hash: "5JtY7ZkLwcDaV8Njzv9zc1H7EmL4WQwNwJ8eqZ9GzGCxM2nu82qR",
  kind: "zkapp",
  status: "applied",
  failureReason: null,
  blockHeight: "18492",
  stateHash: "3NLstateHashExample18492",
  timestamp: "2026-07-15T14:59:48Z",
  feePayer: "B62qkJzXExampleFeePayer111111111111111111111111111x7Kj",
  source: null,
  receiver: null,
  amount: null,
  fee: "100000000",
  nonce: "18446744073709551615",
  memo: "Zeko transaction",
  accountUpdateCount: "1",
  accountUpdates: [{ index: "0", publicKey: "B62qAccountUpdate11111111111111111111111111111111111", tokenId: "1", balanceChange: "1000000000", incrementNonce: false, callDepth: "0", authorizationKind: "Proof", useFullCommitment: true, mayUseToken: "No" }]
}

export const block = {
  height: "18492",
  stateHash: "3NLstateHashExample18492",
  parentHash: "3NLstateHashExample18491",
  timestamp: "2026-07-15T14:59:48Z",
  chainStatus: "canonical",
  creator: "B62qCreator111111111111111111111111111111111111111",
  blockWinner: "B62qWinner1111111111111111111111111111111111111111",
  ledgerHash: "jwLedgerHash111111111111111111111111111111111111111111",
  globalSlot: "91884",
  transactionCount: "1",
  transaction: { hash: transaction.hash, kind: transaction.kind, status: transaction.status }
}

export const settlement = {
  id: "event-284",
  source: "event",
  status: "confirmed",
  createdAt: "2026-07-15T14:58:00Z",
  batchSequence: "284",
  settlementCommandDigest: "5JSettlementCommandDigest111111111111111111111111111111",
  ethereumTransactionHash: "0x25e200000000000000000000000000000000000000000000000000000000b8c1",
  ledgerHash: "jwLedgerHash111111111111111111111111111111111111111111",
  outerActionState: "19483838493049493930494930394930394930394930394930394",
  outerActionStateLength: "147",
  innerActionState: "20483838493049493930494930394930394930394930394930394",
  innerActionStateLength: "39",
  slotLower: "91884",
  slotUpper: "91948",
  innerActionRoot: "0xroot000000000000000000000000000000000000000000000000000000000001",
  innerActionStartIndex: "38",
  innerActionCount: "1",
  claimableSlot: "91968",
  confirmations: "18",
  ethereumGasUsed: "450000",
  cycleCount: "52146595101",
}

export const deposit = {
  nonce: "147",
  token: "0x0000000000000000000000000000000000000000",
  sender: "0x8Fb20000000000000000000000000000000019A0",
  zekoRecipient: "B62qDepositRecipient111111111111111111111111111111x7Kj",
  ethereumAmount: "84000000000000000",
  zekoAmount: "84000000",
  timeout: "1000",
  ethereumTransactionHash: "0xdeposit00000000000000000000000000000000000000000000000000000147",
  ethereumBlockNumber: "8800147",
  ethereumFinalized: true,
  bridgeJobId: "98ab8f02-6d31-489a-b251-36cae4f7968c",
  bridgeJobStatus: "confirmed",
  outerActionSequence: "147",
  outerActionStateAfter: "123456789",
  synchronizedSettlementSequence: "284",
  status: "synchronized",
  nextAction: "finalizeOnZeko",
  accuracyNote: "Synchronization is authoritative; the archive does not persist a canonical deposit-nonce to L2-finalization mapping."
}

export const withdrawal = {
  settlementSequence: "284",
  offset: 3,
  globalActionIndex: "38",
  recipient: "0x71aC000000000000000000000000000000000b2E",
  amount: "12000000",
  actionFieldsHash: "0xaction00000000000000000000000000000000000000000000000000000001",
  siblings: Array.from({ length: 16 }, (_, index) => `0xsibling${index.toString().padStart(2, "0")}000000000000000000000000000000000000000000000000000000`),
  innerActionRoot: settlement.innerActionRoot,
  commitSlotUpper: 91948,
  claimableSlot: "91968",
  currentVirtualSlot: "91965",
  recipientCursor: "38",
  status: "waitingForDelay",
  nextAction: "waitForWithdrawalDelay"
}

export const account = {
  publicKey: transaction.feePayer,
  tokenId: "1",
  balance: "12345678901234567890",
  nonce: "18446744073709551615",
  delegate: null,
  lastUpdatedBlock: "18492",
  lastUpdatedStateHash: block.stateHash,
  transactions: [transaction]
}

export function responseFor(path: string): unknown {
  const url = new URL(path, "http://gateway.test")
  const pathname = url.pathname
  if (pathname.endsWith("/summary")) return summary
  if (pathname.endsWith("/search")) return { query: url.searchParams.get("q"), groups: { blocks: [{ height: block.height, stateHash: block.stateHash }], transactions: [{ hash: transaction.hash, kind: transaction.kind }], accounts: [], settlements: [], deposits: [], withdrawals: [] } }
  if (pathname.endsWith("/blocks")) return { items: [block], nextCursor: null }
  if (pathname.includes("/blocks/")) return block
  if (pathname.endsWith("/transactions")) return { items: [transaction], nextCursor: null }
  if (pathname.includes("/transactions/")) return transaction
  if (pathname.includes("/accounts/")) return account
  if (pathname.endsWith("/settlements")) return { items: [settlement], nextCursor: null }
  if (pathname.includes("/settlements/")) return settlement
  if (pathname.endsWith("/deposits")) return { items: [deposit], nextCursor: null }
  if (pathname.includes("/deposits/")) return deposit
  if (pathname.endsWith("/withdrawals")) return { items: [withdrawal], nextCursor: null }
  if (pathname.includes("/withdrawals/")) return withdrawal
  throw new Error(`Unhandled fixture request: ${pathname}`)
}
