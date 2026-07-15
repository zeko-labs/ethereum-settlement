import { ethereumNetworkName, type RuntimeConfig } from "../lib/config"
import { shortAddress } from "../lib/wallets"
import { NetworkIcon, Notice, type Direction } from "./BridgeUi"

export const ReviewView = ({ direction, amount, recipient, config, busy, onBack, onConfirm }: {
  direction: Direction
  amount: string
  recipient: string
  config: RuntimeConfig
  busy: boolean
  onBack: () => void
  onConfirm: () => void
}) => {
  const deposit = direction === "deposit"
  const ethereum = ethereumNetworkName(config.expectedEthereumChainId)
  return (
    <section className="review-view" data-screen-label="Review transfer">
      <div className="review-top"><div className="review-title"><h2>Review {direction}</h2><p>Confirm the route and recipient before opening your wallet.</p></div><div className="amount-lockup"><strong>{amount} ETH</strong><span>Native asset</span></div></div>
      <div className="review-route">
        <div className="review-network"><NetworkIcon network={deposit ? "ethereum" : "zeko"} /><strong>{deposit ? ethereum : "Zeko Testnet"}</strong><span>{deposit ? "Settlement & custody" : "Execution network"}</span></div>
        <span className="review-arrow">→</span>
        <div className="review-network"><NetworkIcon network={deposit ? "zeko" : "ethereum"} /><strong>{deposit ? "Zeko Testnet" : ethereum}</strong><span>{deposit ? "Execution network" : "Settlement & custody"}</span></div>
      </div>
      <div className="summary-list">
        <div className="summary-row"><span>Recipient</span><strong title={recipient}>{shortAddress(recipient, 12, 10)}</strong></div>
        <div className="summary-row"><span>Amount received</span><strong>{amount} ETH</strong></div>
        <div className="summary-row"><span>Protocol fees</span><strong>Determined by the bridge SDK</strong></div>
        <div className="summary-row"><span>Signing wallet</span><strong>{deposit ? `Ethereum · ${ethereum}` : `Auro · ${config.minaSigningNetworkId} salt`}</strong></div>
      </div>
      <div className="proof-note"><NetworkIcon network="proof" /><span><strong>Proof-bound settlement.</strong> The bridge transition is accepted only after the SP1 and Ethereum settlement checks succeed.</span></div>
      <Notice kind="warning">Experimental PoC: there is no cancellation or refund path.</Notice>
      <div className="button-row"><button type="button" className="secondary-button" onClick={onBack}>Back</button><button type="button" className="primary-button" disabled={busy} onClick={onConfirm}>{busy ? "Opening wallet…" : deposit ? "Confirm in Ethereum wallet" : "Confirm in Auro"}</button></div>
    </section>
  )
}
