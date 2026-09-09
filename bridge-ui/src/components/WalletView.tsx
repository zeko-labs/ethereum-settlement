import { useState } from "react"
import type { RuntimeConfig } from "../lib/config"
import { sendMftPayment, sendNativePayment } from "../lib/payments"
import { formatWalletError, type AuroProvider } from "../lib/wallets"
import { Notice } from "./BridgeUi"

type Props = {
  config: RuntimeConfig
  getProvider: () => AuroProvider
  account?: string
  onConnect: () => void | Promise<void>
  onSubmitted: (hash: string, kind: "MINA" | "MFT") => void
}

const MINIMUM_MFT_FEE_NANOMINA = 1_000_000_000n

export const WalletView = ({ config, getProvider, account, onConnect, onSubmitted }: Props) => {
  const [kind, setKind] = useState<"MINA" | "MFT">("MINA")
  const [recipient, setRecipient] = useState("")
  const [amount, setAmount] = useState("")
  const [fee, setFee] = useState("0.1")
  const [memo, setMemo] = useState("")
  const [tokenOwner, setTokenOwner] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const mftFee = BigInt(config.zekoTransactionFeeNanomina) > MINIMUM_MFT_FEE_NANOMINA
    ? config.zekoTransactionFeeNanomina
    : MINIMUM_MFT_FEE_NANOMINA.toString()

  const submit = async () => {
    setBusy(true)
    setError("")
    try {
      if (!account) {
        await onConnect()
        return
      }
      const provider = getProvider()
      const hash = kind === "MINA"
        ? await sendNativePayment({
            config,
            provider,
            input: { recipient, amountMina: amount, feeMina: fee, memo }
          })
        : await sendMftPayment({
            config,
            provider,
            sender: account,
            input: {
              tokenOwner,
              recipient,
              amountBaseUnits: amount,
              feeNanomina: fee
            }
          })
      onSubmitted(hash, kind)
      setAmount("")
    } catch (value) {
      setError(formatWalletError(value))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="bridge-form" aria-label="Mina wallet payments">
      <div className="form-heading">
        <h2>Send from Zeko</h2>
        <div className="route-kicker"><strong>Mina wallet</strong><span>·</span><span>Payments and standard MFT tokens</span></div>
      </div>
      <div className="tabs" role="tablist" aria-label="Asset type">
        <button type="button" className={`tab${kind === "MINA" ? " active" : ""}`} role="tab" aria-selected={kind === "MINA"} onClick={() => { setKind("MINA"); setFee("0.1") }}>MINA</button>
        <button type="button" className={`tab${kind === "MFT" ? " active" : ""}`} role="tab" aria-selected={kind === "MFT"} onClick={() => { setKind("MFT"); setFee(mftFee) }}>MFT token</button>
      </div>
      {kind === "MFT" && (
        <label className="recipient-editor"><span className="recipient-label-text">MFT token owner</span><input className="recipient-input" aria-label="MFT token owner" placeholder="B62…" value={tokenOwner} onChange={(event) => setTokenOwner(event.target.value.trim())} /></label>
      )}
      <label className="recipient-editor"><span className="recipient-label-text">Recipient</span><input className="recipient-input" aria-label="Payment recipient" placeholder="B62…" value={recipient} onChange={(event) => setRecipient(event.target.value.trim())} /></label>
      <label className="recipient-editor"><span className="recipient-label-text">{kind === "MINA" ? "Amount (MINA)" : "Amount (exact base units)"}</span><input className="recipient-input" aria-label="Payment amount" inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value.trim())} /></label>
      <label className="recipient-editor"><span className="recipient-label-text">{kind === "MINA" ? "Fee (MINA)" : "Fee (nanomina)"}</span><input className="recipient-input" aria-label="Payment fee" inputMode="decimal" value={fee} onChange={(event) => setFee(event.target.value.trim())} /></label>
      {kind === "MINA" && <label className="recipient-editor"><span className="recipient-label-text">Memo (optional)</span><input className="recipient-input" aria-label="Payment memo" value={memo} onChange={(event) => setMemo(event.target.value)} /></label>}
      {kind === "MFT" && <Notice kind="warning">MFT amounts use exact base units. The first transfer compiles and proves the standard token contract locally and can take several minutes.</Notice>}
      {error && <Notice kind="error">{error}</Notice>}
      <button type="button" className="primary-button" disabled={busy} onClick={() => void submit()}>{busy ? (kind === "MFT" ? "Building and proving…" : "Opening Mina wallet…") : account ? `Review ${kind} payment` : "Connect Mina wallet"}</button>
    </section>
  )
}
