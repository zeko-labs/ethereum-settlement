import type { DepositStatus, WithdrawalProof } from "@zeko-labs/eth-bridge-sdk"
import { formatUnits } from "../lib/amount"
import { depositProgress, withdrawalProgress } from "../lib/status"
import type { PendingOperation } from "../lib/storage"
import { NetworkIcon } from "./BridgeUi"

export const ActivityView = ({ deposits, withdrawals, operations, loading, onDeposit, onWithdrawal }: {
  deposits: DepositStatus[]
  withdrawals: WithdrawalProof[]
  operations: PendingOperation[]
  loading: boolean
  onDeposit: (deposit: DepositStatus) => void
  onWithdrawal: (withdrawal: WithdrawalProof | undefined, operation: PendingOperation) => void
}) => {
  const withdrawalRows = withdrawals.map((withdrawal) => ({
    withdrawal,
    operation: operations.find((row) => row.direction === "withdrawal" && row.recipient.toLowerCase() === withdrawal.recipient.toLowerCase() && row.amount === formatUnits(BigInt(withdrawal.amount), 9, 9))
  }))
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
          return <article className="activity-row" key={`deposit-${deposit.nonce}`}><span className="activity-route-icon"><NetworkIcon network="ethereum" compact /><NetworkIcon network="zeko" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{formatUnits(BigInt(deposit.zekoAmount), 9, 9)} ETH · Deposit #{deposit.nonce}</strong><span className={`status-badge ${progress.tone}`}>{deposit.status}</span></div><span className="activity-secondary">{progress.label}</span></div><button type="button" className="secondary-button compact-button" onClick={() => onDeposit(deposit)}>Resume</button></article>
        })}
        {withdrawalRows.map(({ withdrawal, operation }) => {
          const progress = withdrawalProgress(withdrawal)
          const fallback: PendingOperation = operation ?? { id: `withdrawal:${withdrawal.settlementSequence}:${withdrawal.offset}`, direction: "withdrawal", amount: formatUnits(BigInt(withdrawal.amount), 9, 9), recipient: withdrawal.recipient, transactionHash: "Gateway-discovered transaction", createdAt: new Date(0).toISOString() }
          return <article className="activity-row" key={`withdrawal-${withdrawal.settlementSequence}-${withdrawal.offset}`}><span className="activity-route-icon"><NetworkIcon network="zeko" compact /><NetworkIcon network="ethereum" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{formatUnits(BigInt(withdrawal.amount), 9, 9)} ETH · Withdrawal {withdrawal.settlementSequence}:{withdrawal.offset}</strong><span className={`status-badge ${progress.tone}`}>{withdrawal.status}</span></div><span className="activity-secondary">{progress.label}</span></div><button type="button" className="secondary-button compact-button" onClick={() => onWithdrawal(withdrawal, fallback)}>Resume</button></article>
        })}
        {pendingWithdrawals.map((operation) => <article className="activity-row" key={operation.id}><span className="activity-route-icon"><NetworkIcon network="zeko" compact /><NetworkIcon network="ethereum" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{operation.amount} ETH · Withdrawal request</strong><span className="status-badge active">pending</span></div><span className="activity-secondary">Waiting for Zeko settlement</span></div><button type="button" className="secondary-button compact-button" onClick={() => onWithdrawal(undefined, operation)}>Resume</button></article>)}
      </div>}
    </section>
  )
}
