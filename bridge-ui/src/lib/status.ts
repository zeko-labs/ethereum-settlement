import type { DepositStatus, WithdrawalProof } from "@zeko-labs/eth-bridge-sdk"

export type ProgressTone = "waiting" | "active" | "ready" | "complete" | "failed"

export type UiProgress = {
  label: string
  detail: string
  step: number
  tone: ProgressTone
}

const DEPOSIT_PROGRESS: Record<string, UiProgress> = {
  confirming: { label: "Confirming Ethereum deposit", detail: "Waiting for Ethereum finality.", step: 0, tone: "active" },
  locked: { label: "ETH locked", detail: "Waiting for the operator to queue the SP1 bridge proof.", step: 1, tone: "waiting" },
  proofQueued: { label: "Proof queued", detail: "The deposit batch is queued for SP1 proving.", step: 1, tone: "active" },
  awaitingProofApproval: { label: "Awaiting proof approval", detail: "The operator must approve the quoted prover-network cost.", step: 1, tone: "waiting" },
  proving: { label: "Proving deposit batch", detail: "SP1 is proving the ordered Ethereum deposit batch.", step: 1, tone: "active" },
  submitting: { label: "Submitting bridge proof", detail: "The proven outer action is being submitted to Ethereum.", step: 2, tone: "active" },
  executed: { label: "Proof executed", detail: "Operator submission is required to publish the bridge result.", step: 2, tone: "waiting" },
  bridgeProven: { label: "Bridge proof accepted", detail: "Waiting for a Zeko settlement commit to synchronize the deposit.", step: 2, tone: "active" },
  synchronized: { label: "Ready to finalize", detail: "Sign once with Auro to credit native ETH on Zeko.", step: 3, tone: "ready" },
  finalized: { label: "Deposit finalized", detail: "Native ETH was credited on Zeko.", step: 3, tone: "complete" },
  proofFailed: { label: "Proof failed", detail: "The operator must retry this bridge proof.", step: 1, tone: "failed" }
}

export const depositProgress = (deposit: DepositStatus): UiProgress =>
  DEPOSIT_PROGRESS[deposit.status] ?? {
    label: deposit.status,
    detail: deposit.nextAction,
    step: 1,
    tone: "waiting"
  }

export const withdrawalProgress = (withdrawal: WithdrawalProof): UiProgress => {
  if (withdrawal.status === "processed") {
    return { label: "Withdrawal claimed", detail: "Ethereum advanced the recipient cursor past this action.", step: 3, tone: "complete" }
  }
  if (withdrawal.status === "claimable") {
    return { label: "Ready to claim", detail: "The delay has passed and the Merkle proof is ready.", step: 3, tone: "ready" }
  }
  return {
    label: "Waiting for safety delay",
    detail: `Claimable at slot ${withdrawal.claimableSlot}; current slot ${withdrawal.currentVirtualSlot}.`,
    step: 2,
    tone: "active"
  }
}

export const DEPOSIT_STEPS = ["Lock ETH", "SP1 proof", "Sync settlement", "Finalize"]
export const WITHDRAWAL_STEPS = ["Request", "Settle proof", "Safety delay", "Claim ETH"]
