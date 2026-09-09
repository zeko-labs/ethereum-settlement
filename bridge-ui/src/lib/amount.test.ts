import { describe, expect, it } from "vitest"
import { bridgeAmountFromEth, bridgeAmountFromToken, formatUnits, normalizeAmountInput, parseDecimalUnits } from "./amount"

describe("bridge amounts", () => {
  it("converts nine-decimal native ETH without floating point", () => {
    expect(bridgeAmountFromEth("1.000000001")).toEqual({
      zekoAmount: 1_000_000_001n,
      valueWei: 1_000_000_001_000_000_000n
    })
  })

  it("preserves the full uint64 range", () => {
    const max = "18446744073.709551615"
    expect(bridgeAmountFromEth(max).zekoAmount).toBe(2n ** 64n - 1n)
    expect(() => bridgeAmountFromEth("18446744073.709551616")).toThrow(/bridge limit/)
  })

  it("uses the registered ERC20 precision without native unit conversion", () => {
    expect(bridgeAmountFromToken("12.345678", 6)).toEqual({
      ethereumAmount: 12_345_678n,
      zekoAmount: 12_345_678n
    })
    expect(() => bridgeAmountFromToken("1.0000001", 6)).toThrow(/at most 6/)
    expect(normalizeAmountInput("1.23456789", 6)).toBe("1.234567")
  })

  it("rejects excess decimals and non-positive amounts", () => {
    expect(() => parseDecimalUnits("1.0000000001", 9)).toThrow(/at most 9/)
    expect(() => parseDecimalUnits("0", 9)).toThrow(/greater than zero/)
  })

  it("normalizes input and formats bigint values", () => {
    expect(normalizeAmountInput("1a.234567890123")).toBe("1.234567890")
    expect(formatUnits(1_234_500_000n, 9)).toBe("1.2345")
  })
})
