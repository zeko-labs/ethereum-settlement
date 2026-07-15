import { useCallback } from "react";
import type { ExplorerApi } from "../lib/api";
import { formatInteger, formatWei } from "../lib/format";
import type { Route } from "../lib/router";
import type { RuntimeConfig } from "../lib/runtime";
import { usePolling } from "../lib/usePolling";
import {
  DepositList,
  SettlementList,
  TransactionTable,
  WithdrawalList,
} from "../components/records";
import {
  EmptyState,
  ErrorState,
  Icon,
  LoadingRows,
  RefreshButton,
  SectionHeading,
  Updated,
} from "../components/ui";

export function Overview({
  api,
  config,
  navigate,
}: {
  api: ExplorerApi;
  config: RuntimeConfig;
  navigate: (route: Route) => void;
}) {
  const load = useCallback(
    async (signal: AbortSignal) => {
      const [summary, transactions, settlements, deposits, withdrawals] =
        await Promise.all([
          api.summary(signal),
          api.transactions({}, signal).catch((error) => {
            if (signal.aborted) throw error;
            return { items: [], nextCursor: null };
          }),
          api.settlements({}, signal),
          api.deposits({}, signal),
          api.withdrawals(undefined, signal),
        ]);
      return { summary, transactions, settlements, deposits, withdrawals };
    },
    [api],
  );
  const state = usePolling(load, config.pollIntervalMs);

  return (
    <>
      <section className="overview-hero">
        <div>
          <span className="hero-kicker">Ethereum-settled Zeko L2</span>
          <h1>Network activity, from execution to settlement.</h1>
          <p>
            Inspect every Zeko transaction, SP1-verified settlement and native
            bridge operation in one canonical view.
          </p>
        </div>
        <div className="hero-state">
          <span className="live-dot" />
          <div>
            <strong>
              {state.error
                ? "Indexer connection interrupted"
                : "Network operational"}
            </strong>
            <Updated updatedAt={state.updatedAt} />
          </div>
          <RefreshButton
            onClick={state.refresh}
            loading={state.loading}
            label={false}
          />
        </div>
      </section>
      {state.error && !state.data ? (
        <ErrorState message={state.error} retry={state.refresh} />
      ) : null}
      <section className="metrics-grid">
        <article>
          <span>L2 block height</span>
          <strong>
            {state.data?.summary.l2?.blockHeight
              ? formatInteger(state.data.summary.l2.blockHeight)
              : "—"}
          </strong>
          <small>One transaction per block</small>
        </article>
        <article>
          <span>Latest settlement</span>
          <strong>
            {state.data?.summary.settlement.latestSequence
              ? `#${state.data.summary.settlement.latestSequence}`
              : "—"}
          </strong>
          <small>Accepted on Sepolia</small>
        </article>
        <article>
          <span>Transactions</span>
          <strong>
            {formatInteger(state.data?.summary.l2?.transactionCount)}
          </strong>
          <small>Canonical and pending archive history</small>
        </article>
        <article>
          <span>Bridge volume</span>
          <strong>
            {formatWei(state.data?.summary.bridge.depositedAmount)}
          </strong>
          <small>
            {formatInteger(state.data?.summary.bridge.depositCount)} deposits ·{" "}
            {formatInteger(state.data?.summary.bridge.withdrawalCount)}{" "}
            withdrawals
          </small>
        </article>
      </section>
      <section className="overview-grid">
        <article className="surface wide-surface">
          <SectionHeading
            eyebrow="Execution"
            title="Latest L2 transactions"
            action={
              <button
                className="text-button"
                onClick={() => navigate({ page: "transactions" })}
              >
                View all <Icon name="arrow" />
              </button>
            }
          />
          {state.loading && !state.data ? (
            <LoadingRows />
          ) : state.data?.transactions.items.length ? (
            <TransactionTable
              items={state.data.transactions.items.slice(0, 4)}
              navigate={navigate}
            />
          ) : (
            <EmptyState
              title={
                state.data?.summary.sources.archive
                  ? "No L2 transactions indexed"
                  : "L2 archive is initializing"
              }
              copy={
                state.data?.summary.sources.archive
                  ? "Transactions will appear after the archive observes its first block."
                  : "Settlement and bridge indexing remain live while the sequencer prepares its archive schema."
              }
            />
          )}
        </article>
        <aside className="surface settlement-card">
          <SectionHeading eyebrow="Settlement" title="Latest proof" />
          {state.loading && !state.data ? (
            <LoadingRows count={1} />
          ) : state.data?.settlements.items[0] ? (
            <LatestSettlement
              settlement={state.data.settlements.items[0]}
              navigate={navigate}
            />
          ) : (
            <EmptyState
              title="No settlement yet"
              copy="Accepted Ethereum checkpoints will appear here."
            />
          )}
          <button
            className="text-button full"
            onClick={() => navigate({ page: "settlements" })}
          >
            All settlements <Icon name="arrow" />
          </button>
        </aside>
      </section>
      <section className="surface bridge-surface">
        <SectionHeading
          eyebrow="Cross-chain"
          title="Recent bridge activity"
          action={
            <button
              className="text-button"
              onClick={() => navigate({ page: "bridge" })}
            >
              View all <Icon name="arrow" />
            </button>
          }
        />
        {state.loading && !state.data ? (
          <LoadingRows />
        ) : (
          <>
            {state.data?.deposits.items.slice(0, 2).length ? (
              <DepositList
                items={state.data.deposits.items.slice(0, 2)}
                navigate={navigate}
              />
            ) : null}
            {state.data?.withdrawals.items.slice(0, 2).length ? (
              <WithdrawalList
                items={state.data.withdrawals.items.slice(0, 2)}
                navigate={navigate}
              />
            ) : null}
            {!state.data?.deposits.items.length &&
            !state.data?.withdrawals.items.length ? (
              <EmptyState
                title="No bridge activity indexed"
                copy="Native deposits and settlement-bound withdrawals will appear here."
              />
            ) : null}
          </>
        )}
      </section>
      <footer>
        <span>
          Zeko Testnet · Mina <code>testnet</code> signing domain
        </span>
        <span>Ethereum settlement · Sepolia · Experimental</span>
      </footer>
    </>
  );
}

function LatestSettlement({
  settlement,
  navigate,
}: {
  settlement: Awaited<ReturnType<ExplorerApi["settlements"]>>["items"][number];
  navigate: (route: Route) => void;
}) {
  return (
    <button
      className="proof-card"
      onClick={() =>
        navigate({
          page: "settlement",
          identifier: settlement.batchSequence ?? settlement.id,
        })
      }
    >
      <div className="proof-head">
        <span className="proof-emblem">SP1</span>
        <span className={`status status-${settlement.status.toLowerCase()}`}>
          <span className="status-dot" />
          {settlement.status}
        </span>
      </div>
      <strong>
        Settlement{" "}
        {settlement.batchSequence
          ? `#${settlement.batchSequence}`
          : settlement.id}
      </strong>
      <span>Zeko state transition committed to Ethereum</span>
      <div className="slot-range">
        <span>Slot range</span>
        <code>
          {settlement.slotLower ?? "—"} → {settlement.slotUpper ?? "—"}
        </code>
      </div>
      <div className="proof-progress">
        <span />
      </div>
      <small>
        {settlement.confirmations
          ? `${settlement.confirmations} confirmations`
          : "Canonical event indexed"}
      </small>
    </button>
  );
}
