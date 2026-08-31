import { act, cleanup, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { validConfig } from "./test/fixtures"

const ethereumAccount = "0x0000000000000000000000000000000000000001"
const ethereumAccountB = "0x0000000000000000000000000000000000000002"
const zekoAccount = "B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2"
const zekoAccountB = "B62qjHYvRcQSTZnvjCZcHQ8B8pZxh8LZBJaq63zcChBrcT5FQqqAPq8"

const mocks = vi.hoisted(() => ({
  connectEthereum: vi.fn(),
  connectMina: vi.fn(),
  revokeMinaPermissions: vi.fn(),
  getMinaProvider: vi.fn(),
  createClient: vi.fn(),
  depositNative: vi.fn(),
  finalizeDeposit: vi.fn(),
  requestWithdrawal: vi.fn(),
  listActivity: vi.fn(),
  fetchZekoBalance: vi.fn()
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
  connectMina: mocks.connectMina,
  defaultMinaWallet: vi.fn(() => "metamask-snap"),
  ensureEthereumNetwork: vi.fn(async () => undefined),
  ensureAuroPoCNetwork: vi.fn(async () => undefined),
  getEthereumProvider: vi.fn(() => ({ request: vi.fn() })),
  getMinaProvider: mocks.getMinaProvider
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
  fetchZekoBalance: mocks.fetchZekoBalance,
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
    mocks.connectMina.mockResolvedValue(zekoAccount)
    mocks.revokeMinaPermissions.mockResolvedValue([])
    mocks.getMinaProvider.mockReturnValue({
      isMinaSnap: true,
      revokePermissions: mocks.revokeMinaPermissions
    })
    mocks.createClient.mockResolvedValue(client)
    mocks.depositNative.mockResolvedValue({
      hash: synchronizedDeposit.ethereumTransactionHash,
      nonce: synchronizedDeposit.nonce,
      deposit: synchronizedDeposit
    })
    mocks.finalizeDeposit.mockResolvedValue("5Jfinalized")
    mocks.requestWithdrawal.mockResolvedValue("5Jwithdrawal")
    mocks.listActivity.mockResolvedValue({ deposits: [], withdrawals: [], withdrawalRequests: [] })
    mocks.fetchZekoBalance.mockResolvedValue("2.5")
    localStorage.clear()
    delete window.ethereum
    delete window.mina
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it("runs the deposit review, gateway progress, and Mina-wallet finalization states", async () => {
    const user = userEvent.setup()
    render(<App />)

    expect(await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })).toBeVisible()
    expect(screen.getAllByText(/No cancellation\/refund/i).length).toBeGreaterThan(0)

    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(await screen.findByRole("button", { name: /MetaMask (Snap|Flask)/i }))
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

  it("submits a native withdrawal with the Mina wallet testnet signing domain", async () => {
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(await screen.findByRole("button", { name: /MetaMask (Snap|Flask)/i }))
    await user.click(screen.getByRole("button", { name: "Reverse bridge direction" }))
    await user.type(screen.getByLabelText("Amount of native ETH to bridge"), "0.05")

    expect(screen.getByLabelText("Ethereum recipient")).toHaveValue(ethereumAccount)
    const review = screen.getByRole("button", { name: /Review withdrawal/i })
    await waitFor(() => expect(review).toBeEnabled())
    await user.click(review)
    expect(screen.getByText("Mina wallet · testnet salt")).toBeVisible()
    await user.click(screen.getByRole("button", { name: "Confirm in Mina wallet" }))

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

  it("restores an already-authorized Mina wallet connection after reload", async () => {
    localStorage.setItem("zeko-eth-bridge:v1:auro-connected", "true")
    window.mina = {} as typeof window.mina
    render(<App />)

    expect(await screen.findByRole("button", { name: /Mina wallet B62qke…ABn2/ })).toBeVisible()
    expect(mocks.connectMina).toHaveBeenCalledWith(validConfig, "auro")
  })

  it("serializes a provider switch behind reload reconnection", async () => {
    let resolveReload!: (account: string) => void
    mocks.connectMina
      .mockReturnValueOnce(new Promise((resolve) => {
        resolveReload = resolve
    }))
      .mockResolvedValueOnce(zekoAccountB)
    localStorage.setItem("zeko-eth-bridge:v1:mina-wallet", "metamask-snap")
    localStorage.setItem("zeko-eth-bridge:v1:mina-connected", "metamask-snap")
    const user = userEvent.setup()
    render(<App />)

    await waitFor(() => expect(mocks.connectMina).toHaveBeenCalledWith(validConfig, "metamask-snap"))
    await user.click(screen.getByRole("button", { name: "Open bridge settings" }))
    const auro = screen.getByRole("radio", { name: /Auro Wallet/i })
    expect(auro).toBeDisabled()
    await user.click(auro)
    expect(mocks.connectMina).toHaveBeenCalledTimes(1)

    await act(async () => resolveReload(zekoAccount))
    expect(await screen.findByRole("button", { name: /Mina wallet B62qke…ABn2/ })).toBeVisible()
    expect(auro).toBeEnabled()

    await user.click(auro)
    await waitFor(() => expect(mocks.revokeMinaPermissions).toHaveBeenCalledOnce())
    await user.click(screen.getByRole("button", { name: "Close settings" }))
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(screen.getByRole("button", { name: /Auro Wallet/i }))
    expect(await screen.findByRole("button", { name: /Mina wallet B62qjH…APq8/ })).toBeVisible()
    expect(mocks.connectMina).toHaveBeenCalledTimes(2)
  })

  it("commits an authorized Mina session before balance loading finishes", async () => {
    let resolveBalance!: (balance: string) => void
    mocks.fetchZekoBalance.mockReturnValueOnce(new Promise((resolve) => {
      resolveBalance = resolve
    }))
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(screen.getByRole("button", { name: /MetaMask (Snap|Flask)/i }))

    expect(await screen.findByRole("button", { name: /Mina wallet B62qke…ABn2/ })).toBeVisible()
    expect(localStorage.getItem("zeko-eth-bridge:v1:mina-connected")).toBe("metamask-snap")

    await act(async () => resolveBalance("2.5"))
    expect(await screen.findByText("2.5 ETH")).toBeVisible()
  })

  it("switches between the MetaMask Snap and Auro at runtime", async () => {
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(await screen.findByRole("button", { name: /MetaMask (Snap|Flask)/i }))
    expect(mocks.connectMina).toHaveBeenLastCalledWith(validConfig, "metamask-snap")
    expect(screen.getByText("Zeko Testnet · Snap")).toBeVisible()

    await user.click(screen.getByRole("button", { name: "Open bridge settings" }))
    const snap = screen.getByRole("radio", { name: /MetaMask Snap/i })
    const auro = screen.getByRole("radio", { name: /Auro Wallet/i })
    expect(snap).toHaveAttribute("aria-checked", "true")
    expect(auro).toHaveAttribute("aria-checked", "false")
    await user.click(auro)
    await waitFor(() => expect(mocks.revokeMinaPermissions).toHaveBeenCalledOnce())
    expect(auro).toHaveAttribute("aria-checked", "true")
    expect(localStorage.getItem("zeko-eth-bridge:v1:mina-wallet")).toBe("auro")
    expect(screen.getByRole("button", { name: /Connect Mina wallet/i })).toBeVisible()

    await user.click(screen.getByRole("button", { name: "Close settings" }))
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(await screen.findByRole("button", { name: /Auro Wallet/i }))
    expect(mocks.connectMina).toHaveBeenLastCalledWith(validConfig, "auro")
    expect(mocks.getMinaProvider).toHaveBeenCalledWith("auro")
    expect(screen.getByText("Zeko Testnet · Auro")).toBeVisible()
  })

  it("asks which Mina wallet to use before connecting", async () => {
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))

    expect(screen.getByRole("dialog", { name: "Connect Mina wallet" })).toBeVisible()
    expect(screen.getByRole("button", { name: /MetaMask (Snap|Flask)/i })).toBeVisible()
    expect(screen.getByRole("button", { name: /Auro Wallet/i })).toBeVisible()
    expect(mocks.connectMina).not.toHaveBeenCalled()

    await user.click(screen.getByRole("button", { name: /Auro Wallet/i }))
    await waitFor(() => expect(mocks.connectMina).toHaveBeenCalledWith(validConfig, "auro"))
    expect(mocks.connectMina).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole("dialog", { name: "Connect Mina wallet" })).not.toBeInTheDocument()
    expect(screen.getByText("Zeko Testnet · Auro")).toBeVisible()
  })

  it("disconnects the connected Mina wallet", async () => {
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(screen.getByRole("button", { name: /MetaMask (Snap|Flask)/i }))
    const connected = await screen.findByRole("button", { name: /Mina wallet B62qke…ABn2/ })
    await user.click(connected)

    expect(screen.getByRole("dialog", { name: "Mina wallet" })).toBeVisible()
    await user.click(screen.getByRole("button", { name: "Disconnect Mina wallet" }))

    await waitFor(() => expect(mocks.revokeMinaPermissions).toHaveBeenCalledOnce())
    expect(screen.getByRole("button", { name: /Connect Mina wallet/i })).toBeVisible()
    expect(localStorage.getItem("zeko-eth-bridge:v1:auro-connected")).toBeNull()
    expect(localStorage.getItem("zeko-eth-bridge:v1:mina-connected")).toBeNull()
  })

  it("disconnects Auro locally without calling the Snap permission API", async () => {
    let onAccountsChanged: ((accounts: string[]) => void) | undefined
    const provider = {
      isAuro: true,
      on: vi.fn((event: string, listener: (accounts: string[]) => void) => {
        if (event === "accountsChanged") onAccountsChanged = listener
      }),
      removeAllListeners: vi.fn()
    }
    mocks.getMinaProvider.mockReturnValue(provider)
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(screen.getByRole("button", { name: /Auro Wallet/i }))
    const connected = await screen.findByRole("button", { name: /Mina wallet B62qke…ABn2/ })
    await user.click(connected)
    await user.click(screen.getByRole("button", { name: "Disconnect Mina wallet" }))

    expect(mocks.revokeMinaPermissions).not.toHaveBeenCalled()
    expect(screen.getByRole("button", { name: /Connect Mina wallet/i })).toBeVisible()
    expect(localStorage.getItem("zeko-eth-bridge:v1:auro-connected")).toBeNull()
    expect(localStorage.getItem("zeko-eth-bridge:v1:mina-connected")).toBeNull()

    await act(async () => onAccountsChanged?.([zekoAccount]))
    expect(screen.getByRole("button", { name: /Connect Mina wallet/i })).toBeVisible()
  })

  it("ignores account events until Mina network setup succeeds", async () => {
    let onAccountsChanged: ((accounts: string[]) => void) | undefined
    mocks.getMinaProvider.mockReturnValue({
      isMinaSnap: true,
      on: vi.fn((event: string, listener: (accounts: string[]) => void) => {
        if (event === "accountsChanged") onAccountsChanged = listener
      }),
      removeAllListeners: vi.fn()
    })
    mocks.connectMina.mockImplementationOnce(async () => {
      onAccountsChanged?.([zekoAccount])
      throw new Error("Mina network setup failed")
    })
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect Mina wallet/i }))
    await user.click(screen.getByRole("button", { name: /MetaMask (Snap|Flask)/i }))

    expect(await screen.findAllByText("Mina network setup failed")).not.toHaveLength(0)
    expect(screen.getByRole("button", { name: /Connect Mina wallet/i })).toBeVisible()
    expect(localStorage.getItem("zeko-eth-bridge:v1:auro-connected")).toBeNull()
    expect(localStorage.getItem("zeko-eth-bridge:v1:mina-connected")).toBeNull()
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

  it("does not starve a slow activity response with overlapping refreshes", async () => {
    let resolveActivity!: (value: Awaited<ReturnType<typeof mocks.listActivity>>) => void
    mocks.listActivity.mockReturnValue(new Promise((resolve) => {
      resolveActivity = resolve
    }))
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await waitFor(() => expect(mocks.listActivity).toHaveBeenCalledTimes(1))
    await user.click(screen.getByRole("tab", { name: "Activity" }))
    expect(mocks.listActivity).toHaveBeenCalledTimes(1)

    resolveActivity({
      deposits: [],
      withdrawals: [],
      withdrawalRequests: [{
        globalActionIndex: 4,
        transactionHash: "5JslowArchiveWithdrawal",
        blockHeight: 9,
        timestamp: "1784159326275",
        recipient: ethereumAccount,
        amount: "50000000",
        status: "pendingSettlement",
        nextAction: "waitForSettlement"
      }]
    })
    expect(await screen.findByTestId("activity-withdrawal-4")).toBeVisible()
  })

  it("replaces activity with the latest destination-wallet snapshot", async () => {
    const request = {
      globalActionIndex: 4,
      transactionHash: "5JwalletAWithdrawal",
      blockHeight: 9,
      timestamp: "1784159326275",
      recipient: ethereumAccount,
      amount: "50000000",
      status: "pendingSettlement" as const,
      nextAction: "waitForSettlement" as const
    }
    mocks.listActivity.mockResolvedValue({
      deposits: [],
      withdrawals: [],
      withdrawalRequests: [request]
    })
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await user.click(screen.getByRole("tab", { name: "Activity" }))
    expect(await screen.findByTestId("activity-withdrawal-4")).toBeVisible()

    mocks.listActivity.mockResolvedValue({ deposits: [], withdrawals: [], withdrawalRequests: [] })
    await user.click(screen.getByRole("tab", { name: "Bridge" }))
    await user.click(screen.getByRole("tab", { name: "Activity" }))
    await waitFor(() => expect(screen.queryByTestId("activity-withdrawal-4")).not.toBeInTheDocument())
  })

  it("ignores an activity response from the previously selected Ethereum account", async () => {
    let onAccountsChanged: ((accounts: string[]) => void) | undefined
    window.ethereum = {
      request: vi.fn(async ({ method }: { method: string }) => {
        if (method === "eth_accounts") return []
        if (method === "eth_chainId") return "0xaa36a7"
        return null
      }),
      on: vi.fn((event: string, listener: (value: unknown) => void) => {
        if (event === "accountsChanged") onAccountsChanged = listener as (accounts: string[]) => void
      }),
      removeListener: vi.fn()
    } as typeof window.ethereum
    mocks.createClient.mockImplementation(async ({ account }: { account: string }) => ({
      ...client,
      account
    }))
    let resolveOldActivity!: (value: Awaited<ReturnType<typeof mocks.listActivity>>) => void
    mocks.listActivity
      .mockReturnValueOnce(new Promise((resolve) => {
        resolveOldActivity = resolve
      }))
      .mockResolvedValue({ deposits: [], withdrawals: [], withdrawalRequests: [] })
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await waitFor(() => expect(mocks.listActivity).toHaveBeenCalledTimes(1))
    expect(onAccountsChanged).toBeDefined()
    await act(async () => onAccountsChanged?.([ethereumAccountB]))
    await waitFor(() => expect(mocks.listActivity.mock.calls.length).toBeGreaterThanOrEqual(2))

    resolveOldActivity({
      deposits: [],
      withdrawals: [],
      withdrawalRequests: [{
        globalActionIndex: 9,
        transactionHash: "5JoldWalletWithdrawal",
        blockHeight: 11,
        timestamp: "1784159326275",
        recipient: ethereumAccount,
        amount: "50000000",
        status: "pendingSettlement",
        nextAction: "waitForSettlement"
      }]
    })
    await user.click(screen.getByRole("tab", { name: "Activity" }))
    await waitFor(() => expect(screen.queryByTestId("activity-withdrawal-9")).not.toBeInTheDocument())
  })

  it("shows archive read failures only where activity is displayed", async () => {
    mocks.listActivity.mockRejectedValue(new Error("could not read pending withdrawals from the Zeko archive"))
    const user = userEvent.setup()
    render(<App />)

    await screen.findByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })
    await user.click(screen.getByRole("button", { name: /Connect wallet/i }))
    await waitFor(() => expect(mocks.listActivity).toHaveBeenCalled())
    expect(screen.queryByText(/could not read pending withdrawals/i)).not.toBeInTheDocument()

    await user.click(screen.getByRole("tab", { name: "Activity" }))
    expect(await screen.findByText(/could not read pending withdrawals/i)).toBeVisible()
  })
})
