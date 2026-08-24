import { cleanup, render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, describe, expect, it, vi } from "vitest"
import { validConfig } from "../test/fixtures"

const mocks = vi.hoisted(() => ({
  native: vi.fn(async () => "5Jnative"),
  mft: vi.fn(async () => "5Jmft")
}))

vi.mock("../lib/payments", () => ({
  sendNativePayment: mocks.native,
  sendMftPayment: mocks.mft
}))

import { WalletView } from "./WalletView"

const account = "B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2"
const recipient = "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA"
const tokenOwner = "B62qm7w14uvoXCU6LCTLnZnMT41qD2prFJEpYtRdU1Ny7BvgHcxhVT8"

describe("Mina wallet payment view", () => {
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it("submits a native MINA payment through the connected wallet", async () => {
    const submitted = vi.fn()
    const user = userEvent.setup()
    render(<WalletView config={validConfig} account={account} onConnect={vi.fn()} onSubmitted={submitted} />)

    await user.type(screen.getByLabelText("Payment recipient"), recipient)
    await user.type(screen.getByLabelText("Payment amount"), "1.25")
    await user.type(screen.getByLabelText("Payment memo"), "hello")
    await user.click(screen.getByRole("button", { name: "Review MINA payment" }))

    expect(mocks.native).toHaveBeenCalledWith(expect.objectContaining({
      config: validConfig,
      input: { recipient, amountMina: "1.25", feeMina: "0.1", memo: "hello" }
    }))
    expect(submitted).toHaveBeenCalledWith("5Jnative", "MINA")
  })

  it("submits a standard MFT transfer in exact base units", async () => {
    const submitted = vi.fn()
    const user = userEvent.setup()
    render(<WalletView config={validConfig} account={account} onConnect={vi.fn()} onSubmitted={submitted} />)

    await user.click(screen.getByRole("tab", { name: "MFT token" }))
    await user.type(screen.getByLabelText("MFT token owner"), tokenOwner)
    await user.type(screen.getByLabelText("Payment recipient"), recipient)
    await user.type(screen.getByLabelText("Payment amount"), "1250000")
    await user.click(screen.getByRole("button", { name: "Review MFT payment" }))

    expect(mocks.mft).toHaveBeenCalledWith(expect.objectContaining({
      config: validConfig,
      sender: account,
      input: {
        tokenOwner,
        recipient,
        amountBaseUnits: "1250000",
        feeNanomina: "1000000000"
      }
    }))
    expect(submitted).toHaveBeenCalledWith("5Jmft", "MFT")
  })
})
