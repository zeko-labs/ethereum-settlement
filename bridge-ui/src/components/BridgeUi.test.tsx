import { render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import type { DepositStatus } from "@zeko-labs/eth-bridge-sdk"
import { DepositProgress, WalletChip } from "./BridgeUi"

const synchronizedDeposit: DepositStatus = {
  nonce: 9,
  token: "0x0000000000000000000000000000000000000000",
  sender: "0x0000000000000000000000000000000000000001",
  zekoRecipient: "0x01",
  ethereumAmount: "100000000000000000",
  zekoAmount: "100000000",
  timeout: 100,
  ethereumTransactionHash: "0x0123456789abcdef",
  ethereumFinalized: true,
  bridgeJobId: "job",
  bridgeJobStatus: "confirmed",
  outerActionSequence: 1,
  outerActionStateAfter: "state",
  synchronizedSettlementSequence: 2,
  status: "synchronized",
  nextAction: "finalizeDepositOnZeko"
}

describe("bridge UI components", () => {
  it("shows wallet identity and balance", () => {
    render(<WalletChip network="ethereum" account="0x1234567890abcdef" balance="1.25" onClick={() => undefined} />)
    expect(screen.getByText("0x1234…cdef")).toBeVisible()
    expect(screen.getByText("1.25 ETH")).toBeVisible()
  })

  it("enables finalization only for a synchronized deposit", () => {
    const finalize = vi.fn()
    render(
      <DepositProgress
        deposit={synchronizedDeposit}
        ethereumTransactionUrl="https://sepolia.etherscan.io/tx/0x01"
        onFinalize={finalize}
        busy={false}
      />
    )
    expect(screen.getByRole("button", { name: "Finalize on Zeko" })).toBeEnabled()
    expect(screen.getByText("Gateway status is authoritative and refreshes automatically.")).toBeVisible()
  })
})
