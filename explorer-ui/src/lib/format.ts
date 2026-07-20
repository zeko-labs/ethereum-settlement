export function compact(
  value: string | null | undefined,
  lead = 8,
  tail = 5,
): string {
  if (!value) return "—";
  return value.length > lead + tail + 1
    ? `${value.slice(0, lead)}…${value.slice(-tail)}`
    : value;
}

export function formatInteger(value: string | null | undefined): string {
  if (value == null || !/^\d+$/.test(value)) return value ?? "—";
  return value.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

export function formatNano(
  value: string | null | undefined,
  symbol = "ZEKO",
): string {
  if (!value || !/^-?\d+$/.test(value))
    return value ? `${value} ${symbol}` : "—";
  const negative = value.startsWith("-");
  const digits = negative ? value.slice(1) : value;
  const padded = digits.padStart(10, "0");
  const whole = padded.slice(0, -9).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const fraction = padded.slice(-9).replace(/0+$/, "").slice(0, 6);
  return `${negative ? "−" : ""}${whole}${fraction ? `.${fraction}` : ""} ${symbol}`;
}

export function formatWei(value: string | null | undefined): string {
  if (!value || !/^\d+$/.test(value)) return value ? `${value} wei` : "—";
  const padded = value.padStart(19, "0");
  const whole = padded.slice(0, -18).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const fraction = padded.slice(-18).replace(/0+$/, "").slice(0, 6);
  return `${whole}${fraction ? `.${fraction}` : ""} ETH`;
}

export function timeAgo(
  value: string | null | undefined,
  now = Date.now(),
): string {
  if (!value) return "—";
  const timestamp = timestampMillis(value);
  if (!Number.isFinite(timestamp)) return "—";
  const difference = Math.max(0, now - timestamp);
  const seconds = Math.floor(difference / 1000);
  if (seconds < 60) return `${seconds} sec`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} hr`;
  return `${Math.floor(hours / 24)} day`;
}

export function formatTimestamp(value: string | null | undefined): string {
  if (!value) return "—";
  const timestamp = timestampMillis(value);
  return Number.isFinite(timestamp)
    ? new Date(timestamp).toLocaleString()
    : value;
}

function timestampMillis(value: string): number {
  // Mina archive timestamps are decimal Unix milliseconds; fixtures and
  // adapters may use ISO-8601. Date-scale millisecond values are below the
  // JavaScript safe-integer boundary.
  return /^\d+$/.test(value) ? Number(value) : new Date(value).getTime();
}

export function sentence(value: string): string {
  return value
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replaceAll("_", " ")
    .replace(/^./, (letter) => letter.toUpperCase());
}
