import { normalizeAmountInput } from "../lib/amount"
import { ethereumNetworkName, type RuntimeConfig } from "../lib/config"
import { shortAddress } from "../lib/wallets"
import { NetworkIcon, Notice, type Direction } from "./BridgeUi"

type Props = {
  direction: Direction
  amount: string
  recipient: string
  ethereumAccount?: string
  zekoAccount?: string
  ethereumBalance?: string
  zekoBalance?: string
  config: RuntimeConfig
  validation?: string
  canReview: boolean
  showDetails: boolean
  onAmountChange: (value: string) => void
  onRecipientChange: (value: string) => void
  onSwap: () => void
  onReview: () => void
  onToggleDetails: () => void
}

export const BridgeForm = (props: Props) => {
  const deposit = props.direction === "deposit"
  const source = deposit ? "ethereum" : "zeko"
  const destination = deposit ? "zeko" : "ethereum"
  const sourceAccount = deposit ? props.ethereumAccount : props.zekoAccount
  const sourceBalance = deposit ? props.ethereumBalance : props.zekoBalance
  const ethereum = ethereumNetworkName(props.config.expectedEthereumChainId)
  return (
    <section className="bridge-form" data-screen-label="Bridge form">
      <div className="form-heading">
        <h2>{deposit ? "Deposit to Zeko" : "Withdraw to Ethereum"}</h2>
        <div className="route-kicker"><strong>Native ETH</strong><span>·</span><span>Verified settlement route</span></div>
      </div>
      <div className="transfer-surface">
        <div className="transfer-panel">
          <div className="amount-side">
            <div className="field-topline"><span>You send</span><span>{sourceBalance === undefined ? "Balance unavailable" : `Balance ${sourceBalance} ETH`}</span></div>
            <input className="amount-input" aria-label="Amount of native ETH to bridge" inputMode="decimal" placeholder="0.00" value={props.amount} onChange={(event) => props.onAmountChange(normalizeAmountInput(event.target.value))} />
            <span className="fiat-value">Native ETH · maximum 9 decimal places</span>
          </div>
          <div className="network-side">
            <div className="network-pill"><NetworkIcon network={source} /><span className="network-pill-copy"><span className="network-name">{deposit ? ethereum : "Zeko Testnet"}</span><span className="network-type">{deposit ? "Settlement & custody" : "Execution network"}</span></span><span className="token-label">ETH</span></div>
            <div className="recipient-row"><span>{sourceAccount ? "Connected" : "Required"}</span><strong>{sourceAccount ? shortAddress(sourceAccount) : `Connect ${deposit ? "Ethereum wallet" : "Auro"}`}</strong></div>
          </div>
        </div>
        <button type="button" className="swap-button" onClick={props.onSwap} aria-label="Reverse bridge direction"><span>↕</span></button>
        <div className="transfer-panel">
          <div className="amount-side">
            <div className="field-topline"><span>Requested amount</span><span>Final fees determined by SDK</span></div>
            <div className={`amount-input amount-output${props.amount ? "" : " empty"}`}>{props.amount || "0.00"}</div>
            <span className="fiat-value">Network gas is paid separately in the signing wallet</span>
          </div>
          <div className="network-side">
            <div className="network-pill"><NetworkIcon network={destination} /><span className="network-pill-copy"><span className="network-name">{deposit ? "Zeko Testnet" : ethereum}</span><span className="network-type">{deposit ? "Execution network" : "Settlement & custody"}</span></span><span className="token-label">ETH</span></div>
            <label className="recipient-editor"><span className="recipient-label-text">Recipient</span><input className="recipient-input" aria-label={deposit ? "Zeko recipient" : "Ethereum recipient"} placeholder={deposit ? "B62…" : "0x…"} value={props.recipient} onChange={(event) => props.onRecipientChange(event.target.value.trim())} /></label>
          </div>
        </div>
      </div>
      {props.validation && <div className="validation-message" role="alert"><span>!</span><span>{props.validation}</span></div>}
      <div className="route-summary"><div className="route-line"><span className="route-label">Route</span><span className="route-value"><span className="route-node">{deposit ? "Ethereum" : "Zeko"}</span><span className="route-arrow">→</span><span className="route-node"><NetworkIcon network="proof" compact /> SP1</span><span className="route-arrow">→</span><span className="route-node">{deposit ? "Zeko" : "Ethereum"}</span></span></div><button className="details-button" type="button" onClick={props.onToggleDetails} aria-expanded={props.showDetails}>{props.showDetails ? "Hide details" : "Route details"}</button></div>
      {props.showDetails && <div className="route-details"><div className="detail-cell"><span className="detail-label">Custody</span><strong className="detail-value">Ethereum bridge escrow</strong></div><div className="detail-cell"><span className="detail-label">Deposit policy</span><strong className="detail-value">No cancellation/refund</strong></div><div className="detail-cell"><span className="detail-label">Signing domain</span><strong className="detail-value">Auro · testnet placeholder</strong></div></div>}
      <Notice kind="warning"><strong>No cancellation/refund.</strong> {deposit ? "First sign the ETH lock in your Ethereum wallet." : "First sign the withdrawal request with Auro. Ethereum claim becomes available after settlement and the safety delay."}</Notice>
      <Notice kind="warning">Zeko Testnet currently uses Mina’s <code>testnet</code> signing-domain placeholder in Auro.</Notice>
      <button type="button" className="primary-button" disabled={!props.canReview} onClick={props.onReview}>Review {props.direction}<span>→</span></button>
    </section>
  )
}
