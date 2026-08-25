export type PendingOperation = {
  id: string
  direction: "deposit" | "withdrawal"
  amount: string
  recipient: string
  transactionHash: string
  createdAt: string
  depositNonce?: number
  globalActionIndex?: number
  zekoTransactionHash?: string
  ethereumClaimHash?: string
}

export type MinaWalletKind = "metamask-snap" | "auro"

const STORAGE_PREFIX = "zeko-eth-bridge:v1"
const LEGACY_AURO_CONNECTION_KEY = `${STORAGE_PREFIX}:auro-connected`
const MINA_CONNECTION_KEY = `${STORAGE_PREFIX}:mina-connected`
const MINA_WALLET_KEY = `${STORAGE_PREFIX}:mina-wallet`

export const readMinaWallet = (
  fallback: MinaWalletKind,
  storage: Storage = localStorage
): MinaWalletKind => {
  try {
    const stored = storage.getItem(MINA_WALLET_KEY)
    if (stored === "metamask-snap" || stored === "auro") return stored
    return storage.getItem(LEGACY_AURO_CONNECTION_KEY) === "true" ? "auro" : fallback
  } catch {
    return fallback
  }
}

export const rememberMinaWallet = (
  wallet: MinaWalletKind,
  storage: Storage = localStorage
): void => {
  try {
    storage.setItem(MINA_WALLET_KEY, wallet)
  } catch {
    // The in-memory selection still works when browser storage is unavailable.
  }
}

export const wasMinaWalletConnected = (
  wallet: MinaWalletKind,
  storage: Storage = localStorage
): boolean => {
  try {
    const connectedWallet = storage.getItem(MINA_CONNECTION_KEY)
    if (connectedWallet === wallet) return true
    return connectedWallet === null &&
      wallet === "auro" &&
      storage.getItem(LEGACY_AURO_CONNECTION_KEY) === "true"
  } catch {
    return false
  }
}

export const rememberMinaWalletConnection = (
  wallet: MinaWalletKind,
  connected: boolean,
  storage: Storage = localStorage
): void => {
  try {
    if (connected) storage.setItem(MINA_CONNECTION_KEY, wallet)
    else storage.removeItem(MINA_CONNECTION_KEY)
    storage.removeItem(LEGACY_AURO_CONNECTION_KEY)
  } catch {
    // Reconnection is optional when browser storage is unavailable.
  }
}

export const wasAuroConnected = (storage: Storage = localStorage): boolean =>
  wasMinaWalletConnected("auro", storage)

export const rememberAuroConnection = (
  connected: boolean,
  storage: Storage = localStorage
): void => rememberMinaWalletConnection("auro", connected, storage)

export const operationStorageKey = (chainId: number, bridge: string, wallet: string): string =>
  `${STORAGE_PREFIX}:${chainId}:${bridge.toLowerCase()}:${wallet.toLowerCase()}`

export const readOperations = (key: string, storage: Storage = localStorage): PendingOperation[] => {
  try {
    const parsed: unknown = JSON.parse(storage.getItem(key) ?? "[]")
    if (!Array.isArray(parsed)) return []
    return parsed.filter((row): row is PendingOperation => {
      if (typeof row !== "object" || row === null) return false
      const value = row as Record<string, unknown>
      const nonce = value.depositNonce
      const globalActionIndex = value.globalActionIndex
      return (
        typeof value.id === "string" &&
        (value.direction === "deposit" || value.direction === "withdrawal") &&
        typeof value.amount === "string" &&
        typeof value.recipient === "string" &&
        typeof value.transactionHash === "string" &&
        typeof value.createdAt === "string" &&
        !Number.isNaN(Date.parse(value.createdAt)) &&
        (nonce === undefined ||
          (typeof nonce === "number" && Number.isSafeInteger(nonce) && nonce >= 0)) &&
        (globalActionIndex === undefined ||
          (typeof globalActionIndex === "number" &&
            Number.isSafeInteger(globalActionIndex) &&
            globalActionIndex >= 0)) &&
        (value.zekoTransactionHash === undefined || typeof value.zekoTransactionHash === "string") &&
        (value.ethereumClaimHash === undefined || typeof value.ethereumClaimHash === "string")
      )
    })
  } catch {
    return []
  }
}

export const writeOperations = (
  key: string,
  operations: PendingOperation[],
  storage: Storage = localStorage
): void => storage.setItem(key, JSON.stringify(operations.slice(0, 50)))

export const upsertOperation = (
  key: string,
  operation: PendingOperation,
  storage: Storage = localStorage
): PendingOperation[] => {
  const next = [operation, ...readOperations(key, storage).filter((row) => row.id !== operation.id)]
  writeOperations(key, next, storage)
  return next
}
