import { cleanup, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { validConfig } from "./test/fixtures"

const ethereumAccount = "0x0000000000000000000000000000000000000001"
const zekoAccount = "B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2"

const mocks = vi.hoisted(() => ({
  connectEthereum: vi.fn(),
  connectAuro: vi.fn(),
  createClient: vi.fn(),
  depositNative: vi.fn(),
  finalizeDeposit: vi.fn(),
  requestWithdrawal: vi.fn(),
  listActivity: vi.fn()
}))

const client = {
  account: ethereumAccount,
  config: { bridgeAddress: "0x00000000000000000000000000000000000000b0" },
  claimNativeWithdrawal: vi.fn()
}

vi.mock("./lib/config", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./lib/config")>()),
  loadRuntimeConfig: vi.fn(async () => validConfig)
}))

vi.mock("./lib/wallets", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./lib/wallets")>()),
  connectEthereum: mocks.connectEthereum,
  connectAuro: mocks.connectAuro,
  ensureEthereumNetwork: vi.fn(async () => undefined),
  ensureAuroPoCNetwork: vi.fn(async () => undefined),
  getEthereumProvider: vi.fn(() => ({ request: vi.fn() })),
  getAuroProvider: vi.fn(() => ({}))
}))

vi.mock("./lib/bridge", () => ({
  loadBridgeModules: vi.fn(async () => ({})),
  createEthereumBridgeClient: mocks.createClient,
  depositNative: mocks.depositNative,
  finalizeDeposit: mocks.finalizeDeposit,
  requestNativeWithdrawal: mocks.requestWithdrawal,
  listWalletActivity: mocks.listActivity,
  isValidZekoAddress: vi.fn(async () => true),
  fetchEthereumBalance: vi.fn(async () => "1.25"),
  fetchZekoBalance: vi.fn(async () => "2.5"),
  ethereumTransactionUrl: vi.fn((_config, hash) => `https://sepolia.etherscan.io/tx/${hash}`),
  zekoTransactionUrl: vi.fn((_config, hash) => `https://zekoscan.io/testnet/transactions/${hash}`)
}))

import App from "./App"

const synchronizedDeposit = {
  nonce: 9,
  token: "0x0000000000000000000000000000000000000000",
  sender: ethereumAccount,
  zekoRecipient: "0x01",
  ethereumAmount: "100000000000000000",
  zekoAmount: "100000000",
  timeout: 4294967295,
  ethereumTransactionHash: `0x${"12".repeat(32)}`,
  ethereumFinalized: true,
  bridgeJobId: "job",
  bridgeJobStatus: "confirmed",
  outerActionSequence: 1,
  outerActionStateAfter: "state",
  synchronizedSettlementSequence: 2,
  status: "synchronized",
  nextAction: "finalizeDepositOnZeko"
} as const

describe("bridge application", () => {
  beforeEach(() => {
    mocks.connectEthereum.mockResolvedValue(ethereumAccount)
    mocks.connectAuro.mockResolvedValue(zekoAccount)
    mocks.createClient.mockResolvedValue(client)
    mocks.depositNative.mockResolvedValue({
      hash: synchronizedDeposit.ethereumTransactionHash,
      nonce: synchronizedDeposit.nonce,
      deposit: synchronizedDeposit
    })
    mocks.finalizeDeposit.mockResolvedValue("5Jfinalized")
    mocks.requestWithdrawal.mockResolvedValue("5Jwithdrawal")
    mocks.listActivity.mockResolvedValue({ deposits: [], withdrawals: [], withdrawalRequests: [] })
    localStorage.clear()
    delete window.ethereum
    delete window.mina
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it("runs the deposit review, gateway progress, and Auro finalization states", async () => {
    const user = userEvent.setup()
    render(<App />)

    expect(await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })).toBeVisible()
    expect(screen.getAllByText(/No cancellation\/refund/i).length).toBeGreaterThan(0)

    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await user.click(screen.getByRole("button", { name: /Connect Auro/i }))
    await user.type(screen.getByLabelText("Amount of native ETH to bridge"), "0.1")
    expect(screen.getByLabelText("Zeko recipient")).toHaveValue(zekoAccount)

    const review = await screen.findByRole("button", { name: /Review deposit/i })
    await waitFor(() => expect(review).toBeEnabled())
    await user.click(review)
    expect(screen.getByText(/there is no cancellation or refund path/i)).toBeVisible()
    await user.click(screen.getByRole("button", { name: "Confirm in Ethereum wallet" }))

    expect(await screen.findByRole("button", { name: "Finalize on Zeko" })).toBeEnabled()
    expect(mocks.depositNative).toHaveBeenCalledWith(
      expect.objectContaining({ recipient: zekoAccount, valueWei: 100_000_000_000_000_000n })
    )
    await user.click(screen.getByRole("button", { name: "Finalize on Zeko" }))
    expect(await screen.findByRole("heading", { name: "Deposit finalized" })).toBeVisible()
    expect(screen.getByRole("link", { name: /View transaction/i })).toHaveAttribute(
      "href",
      "https://zekoscan.io/testnet/transactions/5Jfinalized"
    )
  })

  it("shows rejected wallet signatures without claiming success", async () => {
    mocks.depositNative.mockRejectedValueOnce(new Error("User rejected the request"))
    const user = userEvent.setup()
    render(<App />)
    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await user.type(screen.getByLabelText("Amount of native ETH to bridge"), "0.1")
    await user.type(screen.getByLabelText("Zeko recipient"), zekoAccount)
    const review = screen.getByRole("button", { name: /Review deposit/i })
    await waitFor(() => expect(review).toBeEnabled())
    await user.click(review)
    await user.click(screen.getByRole("button", { name: "Confirm in Ethereum wallet" }))
    expect(await screen.findByText("The wallet request was rejected.")).toBeVisible()
    expect(screen.queryByText("Deposit finalized")).not.toBeInTheDocument()
  })

  it("submits a native withdrawal with Auro's testnet signing domain", async () => {
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await user.click(screen.getByRole("button", { name: /Connect Auro/i }))
    await user.click(screen.getByRole("button", { name: "Reverse bridge direction" }))
    await user.type(screen.getByLabelText("Amount of native ETH to bridge"), "0.05")

    expect(screen.getByLabelText("Ethereum recipient")).toHaveValue(ethereumAccount)
    const review = screen.getByRole("button", { name: /Review withdrawal/i })
    await waitFor(() => expect(review).toBeEnabled())
    await user.click(review)
    expect(screen.getByText("Auro · testnet salt")).toBeVisible()
    await user.click(screen.getByRole("button", { name: "Confirm in Auro" }))

    expect(await screen.findByRole("heading", { name: "Withdrawal in progress" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Waiting for settlement" })).toBeDisabled()
    expect(mocks.requestWithdrawal).toHaveBeenCalledWith(
      expect.objectContaining({
        sender: zekoAccount,
        recipient: ethereumAccount,
        amount: 50_000_000n,
        config: validConfig
      })
    )

    await user.click(screen.getByRole("tab", { name: "Activity" }))
    expect(await screen.findByText("0.05 ETH · Withdrawal request")).toBeVisible()
    expect(screen.getByText("Waiting for Zeko settlement")).toBeVisible()
  })

  it("restores an already-authorized Auro connection after reload", async () => {
    localStorage.setItem("zeko-eth-bridge:v1:auro-connected", "true")
    window.mina = {} as typeof window.mina
    render(<App />)

    expect(await screen.findByRole("button", { name: /Auro wallet B62qke…ABn2/ })).toBeVisible()
    expect(mocks.connectAuro).toHaveBeenCalledTimes(1)
  })

  it("recovers a pending withdrawal from gateway archive activity", async () => {
    mocks.listActivity.mockResolvedValue({
      deposits: [],
      withdrawals: [],
      withdrawalRequests: [{
        globalActionIndex: 0,
        transactionHash: "5JarchiveWithdrawal",
        blockHeight: 9,
        timestamp: "1784159326275",
        recipient: ethereumAccount,
        amount: "5000000000",
        status: "pendingSettlement",
        nextAction: "waitForSettlement"
      }]
    })
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await user.click(screen.getByRole("tab", { name: "Activity" }))
    expect(await screen.findByText("5 ETH · Withdrawal request")).toBeVisible()
    expect(screen.getByText("Waiting for Zeko settlement")).toBeVisible()
  })
})
