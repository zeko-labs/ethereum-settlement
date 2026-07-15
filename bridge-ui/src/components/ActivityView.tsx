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
  onWithdrawal: (withdrawal: WithdrawalProof, operation?: PendingOperation) => void
}) => {
  const rows = deposits.length + withdrawals.length
  return (
    <section className="activity-view" data-screen-label="Bridge activity">
      <div className="activity-heading"><div><h2>Bridge activity</h2><p>Recovered from gateway state for the connected recipients.</p></div><span className="prototype-badge">{loading ? "Refreshing" : `${rows} indexed`}</span></div>
      {rows === 0 ? <div className="empty-state"><strong>No indexed bridge activity</strong><span>Connect both wallets or submit a new transfer.</span></div> : <div className="activity-list">
        {deposits.map((deposit) => {
          const progress = depositProgress(deposit)
          return <article className="activity-row" key={`deposit-${deposit.nonce}`}><span className="activity-route-icon"><NetworkIcon network="ethereum" compact /><NetworkIcon network="zeko" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{formatUnits(BigInt(deposit.zekoAmount), 9, 9)} ETH · Deposit #{deposit.nonce}</strong><span className={`status-badge ${progress.tone}`}>{deposit.status}</span></div><span className="activity-secondary">{progress.label}</span></div><button type="button" className="secondary-button compact-button" onClick={() => onDeposit(deposit)}>Resume</button></article>
        })}
        {withdrawals.map((withdrawal) => {
          const progress = withdrawalProgress(withdrawal)
          const operation = operations.find((row) => row.direction === "withdrawal" && row.recipient.toLowerCase() === withdrawal.recipient.toLowerCase() && row.amount === formatUnits(BigInt(withdrawal.amount), 9, 9))
          return <article className="activity-row" key={`withdrawal-${withdrawal.settlementSequence}-${withdrawal.offset}`}><span className="activity-route-icon"><NetworkIcon network="zeko" compact /><NetworkIcon network="ethereum" compact /></span><div className="activity-main"><div className="activity-primary"><strong>{formatUnits(BigInt(withdrawal.amount), 9, 9)} ETH · Withdrawal {withdrawal.settlementSequence}:{withdrawal.offset}</strong><span className={`status-badge ${progress.tone}`}>{withdrawal.status}</span></div><span className="activity-secondary">{progress.label}</span></div><button type="button" className="secondary-button compact-button" onClick={() => onWithdrawal(withdrawal, operation)}>Resume</button></article>
        })}
      </div>}
    </section>
  )
}
