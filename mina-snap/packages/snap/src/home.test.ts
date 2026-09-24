import { describe, expect, it } from "@jest/globals"
import {
  formatNanomina,
  parseWalletAccounts,
  renderWalletHome
} from "./home"

describe("Zeko Wallet home data", () => {
  it("formats native balances exactly without floating-point rounding", () => {
    expect(formatNanomina("0")).toBe("0")
    expect(formatNanomina("998900000000")).toBe("998.9")
    expect(formatNanomina("100000001")).toBe("0.100000001")
    expect(formatNanomina("1000000000000000000000000001")).toBe(
      "1000000000000000000.000000001"
    )
  })

  it("separates native balances from custom token accounts and preserves exact token units", () => {
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

  it.each([
    ["zeko:testnet", "ETH"],
    ["testnet", "ETH"],
    ["zeko:mainnet", "MINA"],
    ["mina:devnet", "MINA"],
    ["mina:mainnet", "MINA"]
  ])("labels balances and payment fields correctly on %s without rescaling", (networkId, symbol) => {
    const page = JSON.stringify(renderWalletHome({
      publicKey: "B62qwallet",
      networkId,
      networkName: "Selected network",
      ...(symbol === "ETH" ? { nativeCurrency: { symbol: "ETH" as const, decimals: 9 as const } } : {}),
      endpoint: "https://node.example/graphql",
      native: { total: "1234567891", liquid: "1234567890", locked: "1", nonce: "2" },
      tokens: []
    }))
    expect(page).toContain("Zeko Wallet")
    expect(page).toContain(`1.234567891 ${symbol}`)
    expect(page).toContain(`1.23456789 ${symbol}`)
    expect(page).toContain(`Amount (${symbol})`)
    expect(page).toContain(`Fee (${symbol})`)
    expect(page).not.toContain(symbol === "ETH" ? "MINA" : "ETH")
  })

  it.each(["zeko:testnet", "testnet"])("keeps ambiguous %s balances neutral without approved currency metadata", (networkId) => {
    const page = JSON.stringify(renderWalletHome({
      publicKey: "B62qwallet",
      networkId,
      networkName: "Zeko Ethereum name alone is not currency metadata",
      endpoint: "https://testnet.zeko.io/graphql",
      native: { total: "1234567891", liquid: "1234567890", locked: "1", nonce: "2" },
      tokens: []
    }))
    expect(page).toContain("1.234567891 native units")
    expect(page).toContain("Amount (native units)")
    expect(page).not.toContain("MINA")
    expect(page).not.toContain(" ETH")
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
