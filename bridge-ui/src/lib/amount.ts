const UINT64_MAX = 2n ** 64n - 1n

export class AmountError extends Error {}

export const parseDecimalUnits = (value: string, decimals: number): bigint => {
  const normalized = value.trim()
  if (!/^(?:0|[1-9][0-9]*)(?:\.[0-9]+)?$/.test(normalized)) {
    throw new AmountError("Enter a valid decimal amount")
  }
  const [whole, fraction = ""] = normalized.split(".")
  if (fraction.length > decimals) {
    throw new AmountError(`Use at most ${decimals} decimal places`)
  }
  const units = BigInt(whole) * 10n ** BigInt(decimals) + BigInt(fraction.padEnd(decimals, "0") || "0")
  if (units === 0n) throw new AmountError("Amount must be greater than zero")
  return units
}

export const bridgeAmountFromEth = (value: string) => {
  const zekoAmount = parseDecimalUnits(value, 9)
  if (zekoAmount > UINT64_MAX) throw new AmountError("Amount exceeds the bridge limit")
  const valueWei = zekoAmount * 1_000_000_000n
  return { valueWei, zekoAmount }
}

export const formatUnits = (units: bigint, decimals: number, maxDecimals = decimals): string => {
  const negative = units < 0n
  const absolute = negative ? -units : units
  const factor = 10n ** BigInt(decimals)
  const whole = absolute / factor
  const fraction = (absolute % factor).toString().padStart(decimals, "0").slice(0, maxDecimals)
  const trimmed = fraction.replace(/0+$/, "")
  return `${negative ? "-" : ""}${whole}${trimmed ? `.${trimmed}` : ""}`
}

export const normalizeAmountInput = (value: string): string => {
  const cleaned = value.replace(/[^0-9.]/g, "")
  const [whole = "", ...fractions] = cleaned.split(".")
  const fraction = fractions.join("").slice(0, 9)
  return fractions.length > 0 ? `${whole}.${fraction}` : whole
}
