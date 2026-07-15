import { describe, expect, it } from "vitest"
import { bridgeAmountFromEth, formatUnits, normalizeAmountInput, parseDecimalUnits } from "./amount"

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

  it("enforces the experimental deposit cap", () => {
    expect(() => bridgeAmountFromEth("0.100000001", 100_000_000_000_000_000n)).toThrow(/deposit cap/)
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
