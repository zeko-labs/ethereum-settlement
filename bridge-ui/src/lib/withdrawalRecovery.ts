import type { TokenWithdrawalProof, WithdrawalRequest } from "@zeko-labs/eth-bridge-sdk"
import { getAddress, type Hex } from "viem"
import { formatUnits } from "./amount"
import { NATIVE_ASSET, type BridgeAsset } from "./assets"
import type { PendingOperation } from "./storage"

export type TokenWithdrawalRequest = WithdrawalRequest & { token: string; assetId: string }

export const activityAsset = (assets: BridgeAsset[], token?: string, assetId?: string): BridgeAsset | undefined =>
  token === undefined ? NATIVE_ASSET : assets.find((asset) =>
    asset.kind === "erc20" && asset.token.toLowerCase() === token.toLowerCase() &&
    asset.assetId.toLowerCase() === assetId?.toLowerCase())

const record = (value: unknown): Record<string, unknown> => {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid withdrawal")
  return value as Record<string, unknown>
}
const string = (value: unknown): string => {
  if (typeof value !== "string" || !value) throw new Error("Invalid withdrawal string")
  return value
}
const integer = (value: unknown): number => {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw new Error("Invalid withdrawal index")
  return value
}
const word = (value: unknown): Hex => {
  const result = string(value)
  if (!/^0x[0-9a-fA-F]{64}$/.test(result)) throw new Error("Invalid withdrawal hash")
  return result as Hex
}
const identity = (row: Record<string, unknown>) => {
  const amount = string(row.amount)
  if (!/^[0-9]+$/.test(amount) || BigInt(amount) > 18446744073709551615n) throw new Error("Invalid withdrawal amount")
  return {
    globalActionIndex: integer(row.globalActionIndex),
    token: getAddress(string(row.token)),
    assetId: word(row.assetId),
    recipient: getAddress(string(row.recipient)),
    amount
  }
}
export const parseTokenWithdrawal = (value: unknown): TokenWithdrawalProof => {
  const row = record(value)
  if (!Array.isArray(row.siblings)) throw new Error("Invalid withdrawal siblings")
  return {
    ...identity(row),
    settlementSequence: integer(row.settlementSequence),
    offset: integer(row.offset),
    actionFieldsHash: word(row.actionFieldsHash),
    siblings: row.siblings.map(word),
    innerActionRoot: word(row.innerActionRoot),
    commitSlotUpper: integer(row.commitSlotUpper),
    claimableSlot: integer(row.claimableSlot),
    currentVirtualSlot: integer(row.currentVirtualSlot),
    recipientCursor: integer(row.recipientCursor),
    status: string(row.status),
    nextAction: string(row.nextAction)
  }
}
export const parseTokenWithdrawalRequest = (value: unknown): TokenWithdrawalRequest => {
  const row = record(value)
  if (row.status !== "pendingSettlement" || row.nextAction !== "waitForSettlement") throw new Error("Invalid pending withdrawal status")
  return {
    ...identity(row),
    transactionHash: string(row.transactionHash),
    timestamp: string(row.timestamp),
    blockHeight: integer(row.blockHeight),
    status: "pendingSettlement",
    nextAction: "waitForSettlement"
  }
}
export const recoverWithdrawalOperation = (
  request: WithdrawalRequest | TokenWithdrawalRequest,
  assets: BridgeAsset[],
  createdAt: string
): PendingOperation => {
  const asset = activityAsset(assets, "token" in request ? request.token : undefined, "assetId" in request ? request.assetId : undefined)
  if (!asset) throw new Error(`Token withdrawal ${request.globalActionIndex} unavailable: asset is not authenticated and active`)
  return {
    id: `withdrawal:${request.transactionHash}`,
    direction: "withdrawal",
    amount: formatUnits(BigInt(request.amount), asset.zekoDecimals, asset.zekoDecimals),
    recipient: request.recipient,
    transactionHash: request.transactionHash,
    createdAt,
    globalActionIndex: request.globalActionIndex,
    asset: asset.kind === "erc20" ? {
      kind: "erc20", token: asset.token, assetId: asset.assetId, symbol: asset.symbol, decimals: asset.zekoDecimals
    } : undefined
  }
}
