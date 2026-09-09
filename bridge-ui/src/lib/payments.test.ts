import { beforeEach, describe, expect, it, vi } from "vitest"
import { validConfig } from "../test/fixtures"

const mocks = vi.hoisted(() => ({
  validAddress: vi.fn(async () => true),
  ensureNetwork: vi.fn(async () => undefined),
  sendPayment: vi.fn(async () => ({ hash: "5Jnative" })),
  fetchAccount: vi.fn(),
  fundNewAccount: vi.fn(),
  transfer: vi.fn(async () => undefined),
  compile: vi.fn(async () => ({ verificationKey: "vk" })),
  prove: vi.fn(async () => ({ proved: true })),
  sign: vi.fn(),
  send: vi.fn(async () => ({ status: "pending", hash: "5Jmft", errors: [] })),
  transaction: vi.fn()
}))

vi.mock("./bridge", () => ({
  isValidZekoAddress: mocks.validAddress,
  createAuroSigner: vi.fn(() => mocks.sign)
}))

vi.mock("./wallets", () => ({
  ensureAuroPoCNetwork: mocks.ensureNetwork,
  getAuroProvider: vi.fn(),
  isProviderError: (value: unknown) => typeof value === "object" && value !== null && "code" in value
}))

vi.mock("o1js", () => ({
  AccountUpdate: { fundNewAccount: mocks.fundNewAccount },
  fetchAccount: mocks.fetchAccount,
  Mina: {
    Network: vi.fn((value) => value),
    setActiveInstance: vi.fn(),
    transaction: mocks.transaction
  },
  PublicKey: { fromBase58: vi.fn((value: string) => ({ value })) },
  UInt64: { from: vi.fn((value: bigint) => ({ value })) }
}))

vi.mock("mina-fungible-token", () => ({
  FungibleToken: class {
    static compile = mocks.compile
    address: unknown
    constructor(address: unknown) { this.address = address }
    deriveTokenId() { return "token-id" }
    transfer = mocks.transfer
  }
}))

import { sendMftPayment, sendNativePayment } from "./payments"

const sender = "B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2"
const recipient = "B62qpsAarHNrGH4NXUUGNcaEQR66ksaR1bDURSHdiXNRgHVxi9YRTUA"
const tokenOwner = "B62qm7w14uvoXCU6LCTLnZnMT41qD2prFJEpYtRdU1Ny7BvgHcxhVT8"

describe("wallet payments", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.fetchAccount
      .mockResolvedValueOnce({ account: { nonce: 1 } })
      .mockResolvedValueOnce({ account: { zkapp: {} } })
      .mockResolvedValueOnce({ account: undefined, error: { statusCode: 404, statusText: "not found" } })
    mocks.transaction.mockImplementation(async (_feePayer, callback: () => Promise<void>) => {
      await callback()
      return { prove: mocks.prove }
    })
    mocks.sign.mockResolvedValue({ send: mocks.send })
  })

  it("sends native MINA through the Auro-compatible provider", async () => {
    const provider = { requestNetwork: vi.fn(), sendPayment: mocks.sendPayment }
    await expect(sendNativePayment({
      config: validConfig,
      provider: provider as never,
      input: { recipient, amountMina: "1.25", feeMina: "0.1", memo: "hello" }
    })).resolves.toBe("5Jnative")
    expect(mocks.sendPayment).toHaveBeenCalledWith({
      to: recipient,
      amount: 1.25,
      fee: 0.1,
      memo: "hello"
    })
  })

  it("constructs, proves, signs, and submits a standard MFT transfer", async () => {
    const provider = { requestNetwork: vi.fn() }
    mocks.compile.mockRejectedValueOnce(new Error("temporary compiler failure"))
    const request = {
      config: validConfig,
      provider: provider as never,
      sender,
      input: { tokenOwner, recipient, amountBaseUnits: "125", feeNanomina: "2500" }
    }
    await expect(sendMftPayment(request)).rejects.toThrow("temporary compiler failure")
    mocks.fetchAccount
      .mockResolvedValueOnce({ account: { nonce: 1 } })
      .mockResolvedValueOnce({ account: { zkapp: {} } })
      .mockResolvedValueOnce({ account: undefined, error: { statusCode: 404, statusText: "not found" } })
    await expect(sendMftPayment(request)).resolves.toBe("5Jmft")

    expect(mocks.compile).toHaveBeenCalledTimes(2)
    expect(mocks.fundNewAccount).toHaveBeenCalledOnce()
    expect(mocks.transfer).toHaveBeenCalledWith(
      { value: sender },
      { value: recipient },
      { value: 125n }
    )
    expect(mocks.prove).toHaveBeenCalledOnce()
    expect(mocks.sign).toHaveBeenCalledWith({ proved: true })
    expect(mocks.send).toHaveBeenCalledOnce()
  })
})
