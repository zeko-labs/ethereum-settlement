export interface RuntimeConfig {
  schemaVersion: 1;
  gatewayUrl: string;
  bridgeUiUrl: string;
  ethereumExplorerUrl: string;
  networkName: string;
  pollIntervalMs: number;
}

function requiredUrl(value: unknown, field: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${field} must be a non-empty URL`);
  }
  const url = new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error(`${field} must use http or https`);
  }
  return value.replace(/\/$/, "");
}

export function parseRuntimeConfig(value: unknown): RuntimeConfig {
  if (!value || typeof value !== "object")
    throw new Error("runtime config must be an object");
  const raw = value as Record<string, unknown>;
  if (raw.schemaVersion !== 1)
    throw new Error("unsupported runtime config schemaVersion");
  if (typeof raw.networkName !== "string" || raw.networkName.trim() === "") {
    throw new Error("networkName must be a non-empty string");
  }
  if (
    !Number.isInteger(raw.pollIntervalMs) ||
    Number(raw.pollIntervalMs) < 1_000 ||
    Number(raw.pollIntervalMs) > 60_000
  ) {
    throw new Error("pollIntervalMs must be an integer between 1000 and 60000");
  }
  return {
    schemaVersion: 1,
    gatewayUrl: requiredUrl(raw.gatewayUrl, "gatewayUrl"),
    bridgeUiUrl: requiredUrl(raw.bridgeUiUrl, "bridgeUiUrl"),
    ethereumExplorerUrl: requiredUrl(
      raw.ethereumExplorerUrl,
      "ethereumExplorerUrl",
    ),
    networkName: raw.networkName,
    pollIntervalMs: Number(raw.pollIntervalMs),
  };
}

export async function loadRuntimeConfig(
  signal?: AbortSignal,
): Promise<RuntimeConfig> {
  const response = await fetch("/runtime-config.json", {
    cache: "no-store",
    signal,
  });
  if (!response.ok)
    throw new Error(`runtime config request failed (${response.status})`);
  return parseRuntimeConfig(await response.json());
}
