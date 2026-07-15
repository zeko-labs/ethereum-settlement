import type { RuntimeConfig } from "./runtime";
import type {
  AccountRecord,
  BlockRecord,
  DepositRecord,
  Page,
  SearchResponse,
  SettlementRecord,
  Summary,
  TransactionRecord,
  WithdrawalRecord,
} from "./types";

export class ExplorerApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

export class ExplorerApi {
  constructor(private readonly config: RuntimeConfig) {}

  private async get<T>(path: string, signal?: AbortSignal): Promise<T> {
    const response = await fetch(
      `${this.config.gatewayUrl}/v1/explorer${path}`,
      {
        headers: { Accept: "application/json" },
        cache: "no-store",
        signal,
      },
    );
    if (!response.ok) {
      const body = (await response.json().catch(() => null)) as {
        error?: string;
      } | null;
      throw new ExplorerApiError(
        response.status,
        body?.error ?? `request failed (${response.status})`,
      );
    }
    return response.json() as Promise<T>;
  }

  summary(signal?: AbortSignal) {
    return this.get<Summary>("/summary", signal);
  }
  blocks(cursor?: string, signal?: AbortSignal) {
    return this.get<Page<BlockRecord>>(`/blocks${query({ cursor })}`, signal);
  }
  block(identifier: string, signal?: AbortSignal) {
    return this.get<BlockRecord>(
      `/blocks/${encodeURIComponent(identifier)}`,
      signal,
    );
  }
  transactions(
    filters: Record<string, string | undefined> = {},
    signal?: AbortSignal,
  ) {
    return this.get<Page<TransactionRecord>>(
      `/transactions${query(filters)}`,
      signal,
    );
  }
  transaction(hash: string, signal?: AbortSignal) {
    return this.get<TransactionRecord>(
      `/transactions/${encodeURIComponent(hash)}`,
      signal,
    );
  }
  account(publicKey: string, signal?: AbortSignal) {
    return this.get<AccountRecord>(
      `/accounts/${encodeURIComponent(publicKey)}`,
      signal,
    );
  }
  settlements(
    filters: Record<string, string | undefined> = {},
    signal?: AbortSignal,
  ) {
    return this.get<Page<SettlementRecord>>(
      `/settlements${query(filters)}`,
      signal,
    );
  }
  settlement(identifier: string, signal?: AbortSignal) {
    return this.get<SettlementRecord>(
      `/settlements/${encodeURIComponent(identifier)}`,
      signal,
    );
  }
  deposits(
    filters: Record<string, string | undefined> = {},
    signal?: AbortSignal,
  ) {
    return this.get<Page<DepositRecord>>(`/deposits${query(filters)}`, signal);
  }
  deposit(nonce: string, signal?: AbortSignal) {
    return this.get<DepositRecord>(
      `/deposits/${encodeURIComponent(nonce)}`,
      signal,
    );
  }
  withdrawals(cursor?: string, signal?: AbortSignal) {
    return this.get<Page<WithdrawalRecord>>(
      `/withdrawals${query({ cursor })}`,
      signal,
    );
  }
  withdrawal(sequence: string, offset: string, signal?: AbortSignal) {
    return this.get<WithdrawalRecord>(
      `/withdrawals/${encodeURIComponent(sequence)}/${encodeURIComponent(offset)}`,
      signal,
    );
  }
  search(value: string, signal?: AbortSignal) {
    return this.get<SearchResponse>(`/search${query({ q: value })}`, signal);
  }
}

function query(values: Record<string, string | undefined>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values))
    if (value) params.set(key, value);
  const suffix = params.toString();
  return suffix ? `?${suffix}` : "";
}
