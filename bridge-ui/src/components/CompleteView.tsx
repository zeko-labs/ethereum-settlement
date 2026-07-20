import type { Direction } from "./BridgeUi"

export const CompleteView = ({ direction, amount, hash, url, onActivity, onNewTransfer }: {
  direction: Direction
  amount: string
  hash: string
  url: string
  onActivity: () => void
  onNewTransfer: () => void
}) => (
  <section className="complete-view" data-screen-label="Transfer complete">
    <div className="complete-mark">✓</div>
    <div><h2>{direction === "deposit" ? "Deposit finalized" : "Withdrawal claimed"}</h2><p>{amount} ETH completed its proof-bound route.</p></div>
    <div className="complete-amount">{amount} ETH</div>
    <a className="explorer-link" href={url} target="_blank" rel="noreferrer">View transaction {hash.slice(0, 10)}… ↗</a>
    <div className="complete-actions"><button type="button" className="secondary-button" onClick={onActivity}>View activity</button><button type="button" className="primary-button" onClick={onNewTransfer}>New transfer</button></div>
  </section>
)
