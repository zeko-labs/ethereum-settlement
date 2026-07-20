import { useCallback, useState } from "react";
import type { ReactNode } from "react";
import type { ExplorerApi } from "../lib/api";
import type { Route } from "../lib/router";
import type { RuntimeConfig } from "../lib/runtime";
import type {
  DepositRecord,
  Page,
  SettlementRecord,
  TransactionRecord,
  WithdrawalRecord,
  BlockRecord,
} from "../lib/types";
import { usePolling } from "../lib/usePolling";
import {
  BlockTable,
  DepositList,
  SettlementList,
  TransactionTable,
  WithdrawalList,
} from "../components/records";
import {
  EmptyState,
  ErrorState,
  LoadingRows,
  Pager,
  RefreshButton,
  Updated,
} from "../components/ui";

interface ListShellProps {
  title: string;
  copy: string;
  tabs?: string[];
  activeTab?: string;
  onTab?: (tab: string) => void;
  updatedAt: number | null;
  loading: boolean;
  refresh: () => void;
  children: ReactNode;
  pager?: ReactNode;
}

function ListShell({
  title,
  copy,
  tabs,
  activeTab,
  onTab,
  updatedAt,
  loading,
  refresh,
  children,
  pager,
}: ListShellProps) {
  return (
    <section className="list-page">
      <div className="list-hero">
        <div>
          <span className="hero-kicker">Zeko Explorer</span>
          <h1>{title}</h1>
          <p>{copy}</p>
        </div>
        <Updated updatedAt={updatedAt} />
      </div>
      <div className="surface">
        <div className="filter-bar">
          {tabs ? (
            <div className="filter-tabs">
              {tabs.map((item) => (
                <button
                  className={activeTab === item ? "active" : ""}
                  onClick={() => onTab?.(item)}
                  key={item}
                >
                  {item}
                </button>
              ))}
            </div>
          ) : (
            <span />
          )}
          <RefreshButton onClick={refresh} loading={loading} />
        </div>
        {children}
        {pager}
      </div>
    </section>
  );
}

function useCursor() {
  const [cursors, setCursors] = useState<Array<string | undefined>>([
    undefined,
  ]);
  const cursor = cursors[cursors.length - 1];
  return {
    cursor,
    hasPrevious: cursors.length > 1,
    next(value: string) {
      setCursors((current) => [...current, value]);
    },
    previous() {
      setCursors((current) =>
        current.length > 1 ? current.slice(0, -1) : current,
      );
    },
    reset() {
      setCursors([undefined]);
    },
  };
}

function PagedContent<T>({
  state,
  emptyTitle,
  emptyCopy,
  render,
  cursor,
}: {
  state: {
    data: Page<T> | null;
    loading: boolean;
    error: string | null;
    refresh: () => void;
  };
  emptyTitle: string;
  emptyCopy: string;
  render: (items: T[]) => ReactNode;
  cursor: ReturnType<typeof useCursor>;
}) {
  return (
    <>
      {state.error && !state.data ? (
        <ErrorState message={state.error} retry={state.refresh} />
      ) : state.loading && !state.data ? (
        <LoadingRows count={6} />
      ) : state.data?.items.length ? (
        render(state.data.items)
      ) : (
        <EmptyState title={emptyTitle} copy={emptyCopy} />
      )}
      <Pager
        hasPrevious={cursor.hasPrevious}
        hasNext={Boolean(state.data?.nextCursor)}
        onPrevious={cursor.previous}
        onNext={() =>
          state.data?.nextCursor && cursor.next(state.data.nextCursor)
        }
      />
    </>
  );
}

export function BlocksPage({ api, config, navigate }: PageProps) {
  const cursor = useCursor();
  const load = useCallback(
    (signal: AbortSignal) => api.blocks(cursor.cursor, signal),
    [api, cursor.cursor],
  );
  const state = usePolling(
    load,
    config.pollIntervalMs,
    cursor.cursor ?? "newest",
  );
  return (
    <ListShell
      title="L2 blocks"
      copy="Every Zeko block contains one sequenced transaction."
      updatedAt={state.updatedAt}
      loading={state.loading}
      refresh={state.refresh}
    >
      <PagedContent<BlockRecord>
        state={state}
        emptyTitle="No L2 blocks indexed"
        emptyCopy="Blocks will appear when the archive is connected."
        render={(items) => <BlockTable items={items} navigate={navigate} />}
        cursor={cursor}
      />
    </ListShell>
  );
}

const transactionTabs: Record<string, string | undefined> = {
  All: undefined,
  zkApp: "zkapp",
  Payment: "payment",
  Delegation: "delegation",
};

export function TransactionsPage({ api, config, navigate }: PageProps) {
  const cursor = useCursor();
  const [tab, setTab] = useState("All");
  const load = useCallback(
    (signal: AbortSignal) =>
      api.transactions(
        { cursor: cursor.cursor, kind: transactionTabs[tab] },
        signal,
      ),
    [api, cursor.cursor, tab],
  );
  const state = usePolling(
    load,
    config.pollIntervalMs,
    `${cursor.cursor ?? "newest"}-${tab}`,
  );
  const select = (next: string) => {
    setTab(next);
    cursor.reset();
  };
  return (
    <ListShell
      title="L2 transactions"
      copy="One sequenced transaction per Zeko block, including full zkApp account-update detail."
      tabs={Object.keys(transactionTabs)}
      activeTab={tab}
      onTab={select}
      updatedAt={state.updatedAt}
      loading={state.loading}
      refresh={state.refresh}
    >
      <PagedContent<TransactionRecord>
        state={state}
        emptyTitle="No matching transactions"
        emptyCopy="Try another transaction type or wait for archive indexing."
        render={(items) => (
          <TransactionTable items={items} navigate={navigate} />
        )}
        cursor={cursor}
      />
    </ListShell>
  );
}

const settlementTabs: Record<string, string | undefined> = {
  All: undefined,
  Proving: "proving",
  Submitted: "submitted",
  Confirmed: "confirmed",
  Failed: "failed",
};

export function SettlementsPage({ api, config, navigate }: PageProps) {
  const cursor = useCursor();
  const [tab, setTab] = useState("All");
  const load = useCallback(
    (signal: AbortSignal) =>
      api.settlements(
        { cursor: cursor.cursor, status: settlementTabs[tab] },
        signal,
      ),
    [api, cursor.cursor, tab],
  );
  const state = usePolling(
    load,
    config.pollIntervalMs,
    `${cursor.cursor ?? "newest"}-${tab}`,
  );
  const select = (next: string) => {
    setTab(next);
    cursor.reset();
  };
  return (
    <ListShell
      title="Ethereum settlements"
      copy="Pickles state transitions verified through SP1 and accepted on Sepolia."
      tabs={Object.keys(settlementTabs)}
      activeTab={tab}
      onTab={select}
      updatedAt={state.updatedAt}
      loading={state.loading}
      refresh={state.refresh}
    >
      <PagedContent<SettlementRecord>
        state={state}
        emptyTitle="No matching settlements"
        emptyCopy="Accepted checkpoint events and public proof-job progress appear here."
        render={(items) => <SettlementList items={items} navigate={navigate} />}
        cursor={cursor}
      />
    </ListShell>
  );
}

export function BridgePage({ api, config, navigate }: PageProps) {
  const [tab, setTab] = useState("All");
  const load = useCallback(
    async (signal: AbortSignal) => {
      const [deposits, withdrawals] = await Promise.all([
        api.deposits({}, signal),
        api.withdrawals(undefined, signal),
      ]);
      return { deposits, withdrawals };
    },
    [api],
  );
  const state = usePolling(load, config.pollIntervalMs, tab);
  const showDeposits = tab === "All" || tab === "Deposits";
  const showWithdrawals = tab === "All" || tab === "Withdrawals";
  const deposits = (state.data?.deposits.items ?? []).filter(
    (item) =>
      tab !== "Action required" ||
      ["synchronized", "proofFailed"].includes(item.status),
  );
  const withdrawals = (state.data?.withdrawals.items ?? []).filter(
    (item) => tab !== "Action required" || item.status === "claimable",
  );
  return (
    <ListShell
      title="Bridge activity"
      copy="Native ETH deposits and settlement-bound withdrawals. Statuses come from the authoritative gateway."
      tabs={["All", "Deposits", "Withdrawals", "Action required"]}
      activeTab={tab}
      onTab={setTab}
      updatedAt={state.updatedAt}
      loading={state.loading}
      refresh={state.refresh}
    >
      {state.error && !state.data ? (
        <ErrorState message={state.error} retry={state.refresh} />
      ) : state.loading && !state.data ? (
        <LoadingRows count={6} />
      ) : (
        <>
          {(showDeposits || tab === "Action required") && deposits.length ? (
            <DepositList items={deposits} navigate={navigate} />
          ) : null}
          {(showWithdrawals || tab === "Action required") &&
          withdrawals.length ? (
            <WithdrawalList items={withdrawals} navigate={navigate} />
          ) : null}
          {(showDeposits &&
            !deposits.length &&
            showWithdrawals &&
            !withdrawals.length) ||
          (tab === "Deposits" && !deposits.length) ||
          (tab === "Withdrawals" && !withdrawals.length) ||
          (tab === "Action required" &&
            !deposits.length &&
            !withdrawals.length) ? (
            <EmptyState
              title="No matching bridge operations"
              copy="Bridge records will appear when gateway events are indexed."
            />
          ) : null}
        </>
      )}
      <div className="bridge-accuracy">
        <strong>Bridge status accuracy</strong>
        <p>
          Deposits are only shown as synchronized when an accepted settlement
          has synchronized their outer action. The archive does not claim a
          canonical deposit-nonce-to-L2-finalization mapping.
        </p>
        <a href={config.bridgeUiUrl}>
          Open bridge <span aria-hidden="true">↗</span>
        </a>
      </div>
    </ListShell>
  );
}

interface PageProps {
  api: ExplorerApi;
  config: RuntimeConfig;
  navigate: (route: Route) => void;
}
