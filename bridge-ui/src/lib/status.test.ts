import { describe, expect, it } from "vitest"
import type { DepositStatus, WithdrawalProof } from "@zeko-labs/eth-bridge-sdk"
import { depositProgress, withdrawalProgress } from "./status"

const deposit = (status: string): DepositStatus => ({
  nonce: 1,
  token: "0x0000000000000000000000000000000000000000",
  sender: "0x0000000000000000000000000000000000000001",
  zekoRecipient: "0x01",
  ethereumAmount: "1000000000",
  zekoAmount: "1",
  timeout: 100,
  ethereumTransactionHash: "0x01",
  ethereumFinalized: true,
  bridgeJobId: null,
  bridgeJobStatus: null,
  outerActionSequence: null,
  outerActionStateAfter: null,
  synchronizedSettlementSequence: null,
  status,
  nextAction: "wait"
})

const withdrawal = (status: string): WithdrawalProof => ({
  settlementSequence: 3,
  offset: 2,
  globalActionIndex: 5,
  recipient: "0x0000000000000000000000000000000000000001",
  amount: "1",
  actionFieldsHash: "0x01",
  siblings: [],
  innerActionRoot: "0x01",
  commitSlotUpper: 10,
  claimableSlot: 20,
  currentVirtualSlot: 15,
  recipientCursor: 0,
  status,
  nextAction: "wait"
})

describe("gateway status mapping", () => {
  it("treats proof approval as normal waiting progress", () => {
    expect(depositProgress(deposit("awaitingProofApproval"))).toMatchObject({ step: 1, tone: "waiting" })
  })

  it("makes only synchronized deposits ready for finalization", () => {
    expect(depositProgress(deposit("synchronized"))).toMatchObject({ step: 3, tone: "ready" })
  })

  it("maps claim and cursor states", () => {
    expect(withdrawalProgress(withdrawal("claimable"))).toMatchObject({ step: 3, tone: "ready" })
    expect(withdrawalProgress(withdrawal("processed"))).toMatchObject({ step: 3, tone: "complete" })
  })
})
