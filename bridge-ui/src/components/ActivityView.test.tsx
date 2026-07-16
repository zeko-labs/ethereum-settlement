import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { describe, expect, it, vi } from "vitest"
import { ActivityView } from "./ActivityView"

const recipient = "0x0000000000000000000000000000000000000001" as const

const withdrawal = (globalActionIndex: number, settlementSequence: number) => ({
  settlementSequence,
  offset: 0,
  globalActionIndex,
  recipient,
  amount: "50000000",
  actionFieldsHash: `0x${"12".repeat(32)}` as const,
  siblings: [],
  innerActionRoot: `0x${"34".repeat(32)}` as const,
  commitSlotUpper: 10,
  claimableSlot: 15,
  currentVirtualSlot: 11,
  recipientCursor: 0,
  status: "waitingForDelay",
  nextAction: "waitForWithdrawalDelay"
})

describe("bridge activity", () => {
  it("reconciles pending and settled withdrawals by global action index", async () => {
    const onWithdrawal = vi.fn()
    const matching = {
      id: "withdrawal:matching",
      direction: "withdrawal" as const,
      amount: "0.05",
      recipient,
      transactionHash: "5Jmatching",
      createdAt: "2026-07-16T00:00:00.000Z",
      globalActionIndex: 7
    }
    const sameRecipientAndAmount = {
      ...matching,
      id: "withdrawal:other",
      transactionHash: "5Jother",
      globalActionIndex: 8
    }

    render(
      <ActivityView
        deposits={[]}
        withdrawals={[withdrawal(7, 3)]}
        operations={[sameRecipientAndAmount, matching]}
        loading={false}
        onDeposit={vi.fn()}
        onWithdrawal={onWithdrawal}
      />
    )

    expect(screen.getAllByText(/0.05 ETH/)).toHaveLength(2)
    await userEvent.click(screen.getByTestId("activity-withdrawal-7").querySelector("button")!)
    expect(onWithdrawal).toHaveBeenCalledWith(expect.objectContaining({ globalActionIndex: 7 }), matching)
    expect(screen.getByTestId("activity-withdrawal-8")).toHaveTextContent("Waiting for Zeko settlement")
  })
})
