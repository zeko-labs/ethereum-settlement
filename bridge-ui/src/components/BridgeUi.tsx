import type { ReactNode } from "react"
import { useEffect, useRef } from "react"
import type { DepositStatus, WithdrawalProof } from "@zeko-labs/eth-bridge-sdk"
import { formatUnits } from "../lib/amount"
import { DEPOSIT_STEPS, depositProgress, WITHDRAWAL_STEPS, withdrawalProgress } from "../lib/status"
import { shortAddress } from "../lib/wallets"

export type Direction = "deposit" | "withdrawal"

export const NetworkIcon = ({ network, compact = false }: { network: "ethereum" | "zeko" | "proof"; compact?: boolean }) => {
  if (network === "zeko") {
    return (
      <span className={`network-icon zeko${compact ? " compact" : ""}`} aria-hidden="true">
        <img src="/assets/zeko-token.png" alt="" />
      </span>
    )
  }
  if (network === "proof") return <span className="network-icon proof" aria-hidden="true">SP1</span>
  return <span className={`network-icon ethereum${compact ? " compact" : ""}`} aria-hidden="true"><span>◆</span></span>
}

export const BackgroundWave = ({ className, src, storageKey }: { className: string; src: string; storageKey: string }) => {
  const ref = useRef<HTMLVideoElement>(null)
  useEffect(() => {
    const video = ref.current
    if (!video) return
    const restore = () => {
      try {
        const saved = Number(localStorage.getItem(storageKey))
        if (Number.isFinite(saved) && saved > 0 && saved < video.duration) video.currentTime = saved
      } catch {
        // Ambient motion still works when storage is unavailable.
      }
    }
    const save = () => {
      try {
        localStorage.setItem(storageKey, String(video.currentTime))
      } catch {
        // Ambient motion still works when storage is unavailable.
      }
    }
    video.addEventListener("loadedmetadata", restore)
    video.addEventListener("timeupdate", save)
    return () => {
      video.removeEventListener("loadedmetadata", restore)
      video.removeEventListener("timeupdate", save)
    }
  }, [storageKey])
  return (
    <video ref={ref} className={`contour-background ${className}`} autoPlay loop muted playsInline aria-hidden="true">
      <source src={src} type="video/webm" />
    </video>
  )
}

export const WalletChip = ({
  network,
  ethereumNetworkName = "Sepolia",
  account,
  balance,
  onClick
}: {
  network: "ethereum" | "zeko"
  ethereumNetworkName?: string
  account?: string
  balance?: string
  onClick: () => void
}) => (
  <button
    type="button"
    className="wallet-chip"
    onClick={onClick}
    aria-label={account ? `${network === "ethereum" ? "Ethereum" : "Auro"} wallet ${shortAddress(account)}` : `Connect ${network === "ethereum" ? "wallet" : "Auro"}`}
  >
    <NetworkIcon network={network} compact />
    <span className="wallet-copy">
      <span className="wallet-network">{network === "ethereum" ? ethereumNetworkName : "Zeko Testnet"}</span>
      <span className="wallet-address">{account ? shortAddress(account) : `Connect ${network === "ethereum" ? "wallet" : "Auro"}`}</span>
      {balance !== undefined && <span className="wallet-balance">{balance} ETH</span>}
    </span>
  </button>
)

export const StepTrack = ({ steps, current, tone }: { steps: string[]; current: number; tone: string }) => (
  <div className="progress-track" aria-label={`Step ${current + 1} of ${steps.length}`}>
    {steps.map((label, index) => (
      <div key={label} className={`progress-step${index < current ? " done" : ""}${index === current ? ` current ${tone}` : ""}`}>
        <span className="progress-node">{index < current ? "✓" : index + 1}</span>
        <span className="progress-label">{label}</span>
      </div>
    ))}
  </div>
)

export const Notice = ({ kind = "info", children }: { kind?: "info" | "warning" | "error"; children: ReactNode }) => (
  <div className={`notice ${kind}`} role={kind === "error" ? "alert" : undefined}>
    <span className="notice-mark">{kind === "error" ? "!" : "i"}</span>
    <span>{children}</span>
  </div>
)

export const DepositProgress = ({
  deposit,
  ethereumTransactionUrl,
  onFinalize,
  busy
}: {
  deposit: DepositStatus
  ethereumTransactionUrl: string
  onFinalize: () => void
  busy: boolean
}) => {
  const progress = depositProgress(deposit)
  return (
    <section className="progress-view" data-screen-label="Deposit progress" data-testid="deposit-progress">
      <div className="progress-top">
        <div><h2>Deposit #{deposit.nonce}</h2><p>Gateway status is authoritative and refreshes automatically.</p></div>
        <div className="amount-lockup"><strong>{formatUnits(BigInt(deposit.zekoAmount), 9, 9)} ETH</strong><span>Ethereum → Zeko</span></div>
      </div>
      <StepTrack steps={DEPOSIT_STEPS} current={progress.step} tone={progress.tone} />
      <div className={`current-status ${progress.tone}`}>
        <span className="status-orbit">◌</span>
        <div className="status-copy"><strong>{progress.label}</strong><p>{progress.detail}</p></div>
        <span className="status-time">{deposit.status}</span>
      </div>
      <div className="summary-list">
        <div className="summary-row"><span>Ethereum transaction</span><a className="inline-transaction-link" href={ethereumTransactionUrl} target="_blank" rel="noreferrer">{shortAddress(deposit.ethereumTransactionHash, 10, 8)} ↗</a></div>
        <div className="summary-row"><span>Next action</span><strong>{deposit.nextAction}</strong></div>
        <div className="summary-row"><span>Settlement sequence</span><strong>{deposit.synchronizedSettlementSequence ?? "Pending"}</strong></div>
      </div>
      <button className="primary-button" type="button" disabled={busy || deposit.status !== "synchronized"} onClick={onFinalize}>
        {busy ? "Opening Auro…" : deposit.status === "synchronized" ? "Finalize on Zeko" : "Waiting for settlement"}
      </button>
    </section>
  )
}

export const WithdrawalProgress = ({
  withdrawal,
  amount,
  transactionHash,
  zekoTransactionUrl,
  onClaim,
  busy
}: {
  withdrawal?: WithdrawalProof
  amount: string
  transactionHash: string
  zekoTransactionUrl?: string
  onClaim: () => void
  busy: boolean
}) => {
  const progress = withdrawal
    ? withdrawalProgress(withdrawal)
    : { label: "Withdrawal request submitted", detail: "Waiting for a Zeko commit and Ethereum settlement proof.", step: 0, tone: "active" as const }
  return (
    <section className="progress-view" data-screen-label="Withdrawal progress" data-testid="withdrawal-progress">
      <div className="progress-top">
        <div><h2>Withdrawal in progress</h2><p>The request is recovered from Zeko and Ethereum state after reload.</p></div>
        <div className="amount-lockup"><strong>{amount} ETH</strong><span>Zeko → Ethereum</span></div>
      </div>
      <StepTrack steps={WITHDRAWAL_STEPS} current={progress.step} tone={progress.tone} />
      <div className={`current-status ${progress.tone}`}>
        <span className="status-orbit">◌</span>
        <div className="status-copy"><strong>{progress.label}</strong><p>{progress.detail}</p></div>
        <span className="status-time">{withdrawal?.status ?? "waitingForSettlement"}</span>
      </div>
      <div className="summary-list">
        <div className="summary-row"><span>Zeko transaction</span>{zekoTransactionUrl ? <a className="inline-transaction-link" href={zekoTransactionUrl} target="_blank" rel="noreferrer">{shortAddress(transactionHash, 10, 8)} ↗</a> : <strong>{shortAddress(transactionHash, 10, 8)}</strong>}</div>
        <div className="summary-row"><span>Settlement action</span><strong>{withdrawal ? `${withdrawal.settlementSequence}:${withdrawal.offset}` : "Pending"}</strong></div>
        <div className="summary-row"><span>Recipient cursor</span><strong>{withdrawal?.recipientCursor ?? "Pending"}</strong></div>
      </div>
      <button className="primary-button" type="button" disabled={busy || withdrawal?.status !== "claimable"} onClick={onClaim}>
        {busy ? "Opening Ethereum wallet…" : withdrawal?.status === "claimable" ? "Claim ETH on Ethereum" : "Waiting for settlement"}
      </button>
    </section>
  )
}
