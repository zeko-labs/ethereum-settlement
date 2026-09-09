import { describe, expect, it } from "@jest/globals"
import {
  formatNanomina,
  parseWalletAccounts
} from "./home"

describe("Mina Snap wallet home data", () => {
  it("formats native balances exactly without floating-point rounding", () => {
    expect(formatNanomina("0")).toBe("0")
    expect(formatNanomina("998900000000")).toBe("998.9")
    expect(formatNanomina("100000001")).toBe("0.100000001")
    expect(formatNanomina("1000000000000000000000000001")).toBe(
      "1000000000000000000.000000001"
    )
  })

  it("separates MINA from custom token accounts and preserves exact token units", () => {
    const accounts = parseWalletAccounts([
      {
        publicKey: "B62qwallet",
        tokenId: "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf",
        tokenSymbol: "",
        balance: {
          total: "998900000000",
          liquid: "998900000000",
          locked: "0"
        },
        nonce: "3"
      },
      {
        publicKey: "B62qwallet",
        tokenId: "xkEAxPUqnaKw87xoxCQfykihf7iGWydBftHdcasGwjxuesUVJo",
        tokenSymbol: "USDC",
        balance: {
          total: "1234567",
          liquid: "1200000",
          locked: "34567"
        },
        nonce: "0"
      }
    ])

    expect(accounts.native).toMatchObject({
      total: "998900000000",
      liquid: "998900000000",
      locked: "0",
      nonce: "3"
    })
    expect(accounts.tokens).toEqual([{
      tokenId: "xkEAxPUqnaKw87xoxCQfykihf7iGWydBftHdcasGwjxuesUVJo",
      symbol: "USDC",
      total: "1234567",
      liquid: "1200000",
      locked: "34567"
    }])
  })

  it("rejects malformed balances instead of displaying an incorrect value", () => {
    expect(() => parseWalletAccounts([{
      publicKey: "B62qwallet",
      tokenId: "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf",
      tokenSymbol: "",
      balance: { total: "1.5", liquid: "1.5", locked: "0" },
      nonce: "0"
    }])).toThrow("invalid total balance")
  })

  it("rejects oversized token identifiers from untrusted endpoints", () => {
    expect(() => parseWalletAccounts([{
      tokenId: "x".repeat(129),
      tokenSymbol: "TOKEN",
      balance: { total: "1", liquid: "1", locked: "0" },
      nonce: "0"
    }])).toThrow("invalid token ID")
  })
})
