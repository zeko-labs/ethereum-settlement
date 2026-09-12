import { activityAsset } from "../lib/withdrawalRecovery"
import type { DepositStatus, TokenWithdrawalProof, WithdrawalProof } from "@zeko-labs/eth-bridge-sdk"
import { formatUnits, parseDecimalUnits } from "../lib/amount"
import { NATIVE_ASSET, type BridgeAsset } from "../lib/assets"
import { depositProgress, withdrawalProgress } from "../lib/status"
import type { PendingOperation } from "../lib/storage"
import { NetworkIcon } from "./BridgeUi"

type AnyWithdrawal = WithdrawalProof | TokenWithdrawalProof

export const ActivityView = ({ deposits, withdrawals, operations, assets, loading, onDeposit, onWithdrawal }: {
  deposits: DepositStatus[]
  withdrawals: AnyWithdrawal[]
  operations: PendingOperation[]
  assets?: BridgeAsset[]
  loading: boolean
  onDeposit: (deposit: DepositStatus) => void
  onWithdrawal: (withdrawal: AnyWithdrawal | undefined, operation: PendingOperation) => void
}) => {
  const knownAssets = assets ?? [NATIVE_ASSET]
  const assetForToken = (token?: string, assetId?: string) => activityAsset(knownAssets, token, assetId)
  const unavailable = (id: string, token: string, amount: string) =>
    <article className="activity-row" key={id} data-testid={id}><div className="activity-main"><strong>{amount} base units · Token {token}</strong><span>Unavailable: asset is not authenticated and active</span></div><button type="button" disabled>Resume</button></article>
  const matchedIds = new Set<string>()
  const withdrawalRows = withdrawals.map((withdrawal) => {
    const asset = assetForToken("token" in withdrawal ? withdrawal.token : undefined, "assetId" in withdrawal ? withdrawal.assetId : undefined)
    const operation = operations.find((row) => {
      if (!asset || row.direction !== "withdrawal" || matchedIds.has(row.id)) return false
      if ((row.asset?.token.toLowerCase() ?? "") !== ("token" in withdrawal ? withdrawal.token.toLowerCase() : "")) return false
      if (row.asset && row.asset.assetId.toLowerCase() !== ("assetId" in withdrawal ? withdrawal.assetId.toLowerCase() : "")) return false
      if (row.globalActionIndex !== undefined) return row.globalActionIndex === withdrawal.globalActionIndex
      if (!("token" in withdrawal) || row.asset?.token.toLowerCase() !== withdrawal.token.toLowerCase()) return false
      try {
        return row.recipient.toLowerCase() === withdrawal.recipient.toLowerCase() &&
          parseDecimalUnits(row.amount, asset.zekoDecimals) === BigInt(withdrawal.amount)
      } catch {
        return false
      }
    })
    if (operation) matchedIds.add(operation.id)
    return { withdrawal, operation }
  })
  const matchedOperationIds = new Set(withdrawalRows.flatMap(({ operation }) => operation ? [operation.id] : []))
  const pendingWithdrawals = operations.filter(
    (operation) => operation.direction === "withdrawal" && !matchedOperationIds.has(operation.id)
  )
  const rows = deposits.length + withdrawalRows.length + pendingWithdrawals.length
  return (
    <section className="activity-view" data-screen-label="Bridge activity">
      <div className="activity-heading"><div><h2>Bridge activity</h2><p>Recovered from gateway state for the connected recipients.</p></div><span className="prototype-badge">{loading ? "Refreshing" : `${rows} indexed`}</span></div>
      {rows === 0 ? <div className="empty-state"><strong>No indexed bridge activity</strong><span>Connect both wallets or submit a new transfer.</span></div> : <div className="activity-list">
        {deposits.map((deposit) => {
          const progress = depositProgress(deposit)
          const asset = assetForToken(deposit.assetId ? deposit.token : undefined, deposit.assetId ?? undefined)
          if (!asset) return unavailable(`activity-deposit-${deposit.nonce}`, deposit.token, deposit.zekoAmount)
          return <article className="activity-row" data-testid={`activity-deposit-${deposit.nonce}`} key={`deposit-${deposit.nonce}`}><span className="activity-route-icon"><NetworkIcon network="ethereum" compact /><NetworkIcon network="zeko" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{formatUnits(BigInt(deposit.zekoAmount), asset.zekoDecimals, asset.zekoDecimals)} {asset.symbol} · Deposit #{deposit.nonce}</strong><span className={`status-badge ${progress.tone}`}>{deposit.status}</span></div><span className="activity-secondary">{progress.label}</span></div><button type="button" className="secondary-button compact-button" onClick={() => onDeposit(deposit)}>Resume</button></article>
        })}
        {withdrawalRows.map(({ withdrawal, operation }) => {
          const progress = withdrawalProgress(withdrawal)
          const asset = assetForToken("token" in withdrawal ? withdrawal.token : undefined, "assetId" in withdrawal ? withdrawal.assetId : undefined)
          if (!asset) return unavailable(`activity-withdrawal-${withdrawal.globalActionIndex}`, "token" in withdrawal ? withdrawal.token : "", withdrawal.amount)
          const fallback: PendingOperation = operation ?? {
            id: `withdrawal:${withdrawal.settlementSequence}:${withdrawal.offset}`,
            direction: "withdrawal",
            amount: formatUnits(BigInt(withdrawal.amount), asset.zekoDecimals, asset.zekoDecimals),
            recipient: withdrawal.recipient,
            transactionHash: "Gateway-discovered transaction",
            createdAt: new Date(0).toISOString(),
            asset: asset.kind === "erc20"
              ? { kind: "erc20", token: asset.token, assetId: asset.assetId, symbol: asset.symbol, decimals: asset.zekoDecimals }
              : undefined
          }
          return <article className="activity-row" data-testid={`activity-withdrawal-${withdrawal.globalActionIndex}`} key={`withdrawal-${withdrawal.settlementSequence}-${withdrawal.offset}`}><span className="activity-route-icon"><NetworkIcon network="zeko" compact /><NetworkIcon network="ethereum" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{formatUnits(BigInt(withdrawal.amount), asset.zekoDecimals, asset.zekoDecimals)} {asset.symbol} · Withdrawal {withdrawal.settlementSequence}:{withdrawal.offset}</strong><span className={`status-badge ${progress.tone}`}>{withdrawal.status}</span></div><span className="activity-secondary">{progress.label}</span></div><button type="button" className="secondary-button compact-button" onClick={() => onWithdrawal(withdrawal, fallback)}>Resume</button></article>
        })}
        {pendingWithdrawals.map((operation) => {
          const asset = assetForToken(operation.asset?.token, operation.asset?.assetId)
          if (!asset) return unavailable(operation.id, operation.asset?.token ?? "", "Unknown")
          return <article className="activity-row" data-testid={operation.globalActionIndex === undefined ? operation.id : `activity-withdrawal-${operation.globalActionIndex}`} key={operation.id}><span className="activity-route-icon"><NetworkIcon network="zeko" compact /><NetworkIcon network="ethereum" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{operation.amount} {asset.symbol} · Withdrawal request</strong><span className="status-badge active">pending</span></div><span className="activity-secondary">Waiting for Zeko settlement</span></div><button type="button" className="secondary-button compact-button" onClick={() => onWithdrawal(undefined, operation)}>Resume</button></article>})}
      </div>}
    </section>
  )
}
