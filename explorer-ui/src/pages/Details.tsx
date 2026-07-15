import { useCallback } from "react";
import type { ReactNode } from "react";
import type { ExplorerApi } from "../lib/api";
import {
  compact,
  formatInteger,
  formatNano,
  formatTimestamp,
  formatWei,
  sentence,
  timeAgo,
} from "../lib/format";
import type { Route } from "../lib/router";
import type { RuntimeConfig } from "../lib/runtime";
import type {
  AccountRecord,
  BlockRecord,
  DepositRecord,
  SettlementRecord,
  TransactionRecord,
  WithdrawalRecord,
} from "../lib/types";
import { usePolling } from "../lib/usePolling";
import { TransactionTable } from "../components/records";
import {
  Address,
  DetailGrid,
  DetailHero,
  EmptyState,
  ErrorState,
  Field,
  LoadingRows,
  Status,
} from "../components/ui";

interface DetailProps {
  api: ExplorerApi;
  config: RuntimeConfig;
  navigate: (route: Route) => void;
}

function DetailState<T>({
  data,
  loading,
  error,
  refresh,
  children,
}: {
  data: T | null;
  loading: boolean;
  error: string | null;
  refresh: () => void;
  children: (data: T) => ReactNode;
}) {
  if (loading && !data)
    return (
      <div className="surface detail-loading">
        <LoadingRows count={5} />
      </div>
    );
  if (error && !data) return <ErrorState message={error} retry={refresh} />;
  if (!data)
    return (
      <EmptyState
        title="Record not found"
        copy="The indexed record is unavailable or no longer canonical."
      />
    );
  return <>{children(data)}</>;
}

function Back({
  label,
  route,
  navigate,
}: {
  label: string;
  route: Route;
  navigate: (route: Route) => void;
}) {
  return (
    <button className="back-link" onClick={() => navigate(route)}>
      ← {label}
    </button>
  );
}

export function BlockDetailPage({
  api,
  config,
  navigate,
  identifier,
}: DetailProps & { identifier: string }) {
  const load = useCallback(
    (signal: AbortSignal) => api.block(identifier, signal),
    [api, identifier],
  );
  const state = usePolling(load, config.pollIntervalMs, identifier);
  return (
    <section className="detail-page">
      <Back label="All blocks" route={{ page: "blocks" }} navigate={navigate} />
      <DetailState<BlockRecord> {...state}>
        {(block) => (
          <>
            <DetailHero
              eyebrow="Zeko L2 block"
              title={`Block ${formatInteger(block.height)}`}
              copy={timeAgo(block.timestamp)}
              status={block.chainStatus}
            />
            <article className="surface detail-surface">
              <DetailGrid>
                <Field label="State hash" wide>
                  <Address copy>{block.stateHash}</Address>
                </Field>
                <Field label="Parent hash" wide>
                  <button
                    className="inline-link"
                    onClick={() =>
                      navigate({ page: "block", identifier: block.parentHash })
                    }
                  >
                    <Address>{block.parentHash}</Address>
                  </button>
                </Field>
                <Field label="Global slot">{block.globalSlot ?? "—"}</Field>
                <Field label="Transaction count">
                  {block.transactionCount}
                </Field>
                <Field label="Creator" wide>
                  <button
                    className="inline-link"
                    onClick={() =>
                      navigate({ page: "account", publicKey: block.creator })
                    }
                  >
                    <Address>{block.creator}</Address>
                  </button>
                </Field>
                <Field label="Block winner" wide>
                  <button
                    className="inline-link"
                    onClick={() =>
                      block.blockWinner &&
                      navigate({
                        page: "account",
                        publicKey: block.blockWinner,
                      })
                    }
                  >
                    <Address>{block.blockWinner}</Address>
                  </button>
                </Field>
                <Field label="Ledger hash" wide>
                  <Address copy>{block.ledgerHash}</Address>
                </Field>
                <Field label="Timestamp">
                    {formatTimestamp(block.timestamp)}
                </Field>
              </DetailGrid>
              {block.transaction ? (
                <button
                  className="record-callout"
                  onClick={() =>
                    navigate({
                      page: "transaction",
                      hash: block.transaction!.hash,
                    })
                  }
                >
                  <span className="transaction-mark">▦</span>
                  <span>
                    <small>
                      Included transaction ·{" "}
                      {sentence(block.transaction.kind ?? "command")}
                    </small>
                    <strong className="mono">
                      {compact(block.transaction.hash, 16, 7)}
                    </strong>
                  </span>
                  <Status>{block.transaction.status ?? "unknown"}</Status>
                  <span>›</span>
                </button>
              ) : null}
            </article>
          </>
        )}
      </DetailState>
    </section>
  );
}

export function TransactionDetailPage({
  api,
  config,
  navigate,
  hash,
}: DetailProps & { hash: string }) {
  const load = useCallback(
    (signal: AbortSignal) => api.transaction(hash, signal),
    [api, hash],
  );
  const state = usePolling(load, config.pollIntervalMs, hash);
  return (
    <section className="detail-page">
      <Back
        label="All transactions"
        route={{ page: "transactions" }}
        navigate={navigate}
      />
      <DetailState<TransactionRecord> {...state}>
        {(transaction) => (
          <>
            <DetailHero
              eyebrow="Zeko L2 transaction"
              title={compact(transaction.hash, 18, 8)}
              copy={`Included in block ${transaction.blockHeight} · ${timeAgo(transaction.timestamp)}`}
              status={transaction.status}
            />
            <article className="surface detail-surface">
              <DetailGrid>
                <Field label="Transaction hash" wide>
                  <Address copy>{transaction.hash}</Address>
                </Field>
                <Field label="Command type">{sentence(transaction.kind)}</Field>
                <Field label="Block">
                  <button
                    className="gold-link inline-link"
                    onClick={() =>
                      navigate({
                        page: "block",
                        identifier: transaction.blockHeight,
                      })
                    }
                  >
                    {formatInteger(transaction.blockHeight)}
                  </button>
                </Field>
                <Field label="Fee">{formatNano(transaction.fee)}</Field>
                <Field label="Nonce">{transaction.nonce}</Field>
                <Field label="Fee payer" wide>
                  <button
                    className="inline-link"
                    onClick={() =>
                      navigate({
                        page: "account",
                        publicKey: transaction.feePayer,
                      })
                    }
                  >
                    <Address>{transaction.feePayer}</Address>
                  </button>
                </Field>
                {transaction.source ? (
                  <Field label="Source" wide>
                    <button
                      className="inline-link"
                      onClick={() =>
                        navigate({
                          page: "account",
                          publicKey: transaction.source!,
                        })
                      }
                    >
                      <Address>{transaction.source}</Address>
                    </button>
                  </Field>
                ) : null}
                {transaction.receiver ? (
                  <Field label="Receiver" wide>
                    <button
                      className="inline-link"
                      onClick={() =>
                        navigate({
                          page: "account",
                          publicKey: transaction.receiver!,
                        })
                      }
                    >
                      <Address>{transaction.receiver}</Address>
                    </button>
                  </Field>
                ) : null}
                {transaction.amount ? (
                  <Field label="Amount">{formatNano(transaction.amount)}</Field>
                ) : null}
                <Field label="Memo">{transaction.memo || "—"}</Field>
                <Field label="State hash" wide>
                  <Address>{transaction.stateHash}</Address>
                </Field>
                {transaction.failureReason ? (
                  <Field label="Failure reason" wide>
                    {transaction.failureReason}
                  </Field>
                ) : null}
              </DetailGrid>
              {transaction.accountUpdates?.length ? (
                <AccountUpdates transaction={transaction} navigate={navigate} />
              ) : null}
            </article>
          </>
        )}
      </DetailState>
    </section>
  );
}

function AccountUpdates({
  transaction,
  navigate,
}: {
  transaction: TransactionRecord;
  navigate: (route: Route) => void;
}) {
  return (
    <section className="subsection">
      <div className="subsection-title">
        <span>zkApp command</span>
        <h2>{transaction.accountUpdateCount} account updates</h2>
      </div>
      <div className="account-updates">
        {transaction.accountUpdates?.map((update) => (
          <article key={update.index}>
            <span className="update-index">{update.index}</span>
            <div>
              <button
                className="inline-link"
                onClick={() =>
                  navigate({ page: "account", publicKey: update.publicKey })
                }
              >
                <Address>{update.publicKey}</Address>
              </button>
              <small>
                {sentence(update.authorizationKind)} · call depth{" "}
                {update.callDepth}
              </small>
            </div>
            <strong>{formatNano(update.balanceChange)}</strong>
          </article>
        ))}
      </div>
    </section>
  );
}

export function AccountDetailPage({
  api,
  config,
  navigate,
  publicKey,
}: DetailProps & { publicKey: string }) {
  const load = useCallback(
    (signal: AbortSignal) => api.account(publicKey, signal),
    [api, publicKey],
  );
  const state = usePolling(load, config.pollIntervalMs, publicKey);
  return (
    <section className="detail-page">
      <Back
        label="Transactions"
        route={{ page: "transactions" }}
        navigate={navigate}
      />
      <DetailState<AccountRecord> {...state}>
        {(account) => (
          <>
            <DetailHero
              eyebrow="Zeko account"
              title={compact(account.publicKey, 18, 8)}
              copy={`Last observed in block ${account.lastUpdatedBlock}`}
            />
            <article className="surface detail-surface">
              <DetailGrid>
                <Field label="Public key" wide>
                  <Address copy>{account.publicKey}</Address>
                </Field>
                <Field label="Balance">{formatNano(account.balance)}</Field>
                <Field label="Nonce">{account.nonce}</Field>
                <Field label="Token ID" wide>
                  <Address>{account.tokenId}</Address>
                </Field>
                <Field label="Delegate" wide>
                  {account.delegate ? (
                    <button
                      className="inline-link"
                      onClick={() =>
                        navigate({
                          page: "account",
                          publicKey: account.delegate!,
                        })
                      }
                    >
                      <Address>{account.delegate}</Address>
                    </button>
                  ) : (
                    "Not delegated"
                  )}
                </Field>
                <Field label="Last state hash" wide>
                  <Address>{account.lastUpdatedStateHash}</Address>
                </Field>
              </DetailGrid>
              <section className="subsection">
                <div className="subsection-title">
                  <span>Archive history</span>
                  <h2>Recent transactions</h2>
                </div>
                {account.transactions.length ? (
                  <TransactionTable
                    items={account.transactions}
                    navigate={navigate}
                  />
                ) : (
                  <EmptyState
                    title="No recent transactions"
                    copy="No indexed commands reference this account."
                  />
                )}
              </section>
            </article>
          </>
        )}
      </DetailState>
    </section>
  );
}

export function SettlementDetailPage({
  api,
  config,
  navigate,
  identifier,
}: DetailProps & { identifier: string }) {
  const load = useCallback(
    (signal: AbortSignal) => api.settlement(identifier, signal),
    [api, identifier],
  );
  const state = usePolling(load, config.pollIntervalMs, identifier);
  return (
    <section className="detail-page">
      <Back
        label="All settlements"
        route={{ page: "settlements" }}
        navigate={navigate}
      />
      <DetailState<SettlementRecord> {...state}>
        {(settlement) => (
          <>
            <DetailHero
              eyebrow="Ethereum settlement"
              title={
                settlement.batchSequence
                  ? `Settlement #${settlement.batchSequence}`
                  : compact(settlement.id)
              }
              copy={`Observed ${timeAgo(settlement.createdAt)} ago on Sepolia`}
              status={settlement.status}
            />
            <article className="surface detail-surface">
              <div className="proof-route">
                <span className="proof-node">Zeko L2</span>
                <span>→</span>
                <span className="proof-node accented">SP1 verified</span>
                <span>→</span>
                <span className="proof-node">Ethereum</span>
              </div>
              <DetailGrid>
                <Field label="Ethereum transaction" wide>
                  {settlement.ethereumTransactionHash ? (
                    <a
                      className="external-link"
                      href={`${config.ethereumExplorerUrl}/tx/${settlement.ethereumTransactionHash}`}
                      target="_blank"
                      rel="noreferrer"
                    >
                      <Address>{settlement.ethereumTransactionHash}</Address> ↗
                    </a>
                  ) : (
                    "Pending submission"
                  )}
                </Field>
                <Field label="Slot range">
                  {settlement.slotLower ?? "—"} → {settlement.slotUpper ?? "—"}
                </Field>
                <Field label="Confirmations">
                  {settlement.confirmations ?? "Canonical event"}
                </Field>
                <Field label="Settlement command digest" wide>
                  <Address copy>{settlement.settlementCommandDigest}</Address>
                </Field>
                <Field label="Ledger hash" wide>
                  <Address copy>{settlement.ledgerHash}</Address>
                </Field>
                <Field label="Outer action state" wide>
                  <Address copy>{settlement.outerActionState}</Address>
                </Field>
                <Field label="Outer action length">
                  {settlement.outerActionStateLength ?? "—"}
                </Field>
                <Field label="Inner action length">
                  {settlement.innerActionStateLength ?? "—"}
                </Field>
                <Field label="Inner action state" wide>
                  <Address copy>{settlement.innerActionState}</Address>
                </Field>
                <Field label="Inner action root" wide>
                  <Address copy>{settlement.innerActionRoot}</Address>
                </Field>
                <Field label="Bridge action start">
                  {settlement.innerActionStartIndex ?? "—"}
                </Field>
                <Field label="Bridge action count">
                  {settlement.innerActionCount ?? "—"}
                </Field>
                <Field label="Claimable slot">
                  {settlement.claimableSlot ?? "—"}
                </Field>
                <Field label="SP1 cycles">
                  {formatInteger(settlement.cycleCount)}
                </Field>
                <Field label="Ethereum gas">
                  {formatInteger(settlement.ethereumGasUsed)}
                </Field>
              </DetailGrid>
            </article>
          </>
        )}
      </DetailState>
    </section>
  );
}

export function DepositDetailPage({
  api,
  config,
  navigate,
  nonce,
}: DetailProps & { nonce: string }) {
  const load = useCallback(
    (signal: AbortSignal) => api.deposit(nonce, signal),
    [api, nonce],
  );
  const state = usePolling(load, config.pollIntervalMs, nonce);
  return (
    <section className="detail-page">
      <Back
        label="Bridge activity"
        route={{ page: "bridge" }}
        navigate={navigate}
      />
      <DetailState<DepositRecord> {...state}>
        {(deposit) => (
          <>
            <DetailHero
              eyebrow="Ethereum → Zeko"
              title={`Deposit #${deposit.nonce}`}
              copy={`Ethereum block ${deposit.ethereumBlockNumber}`}
              status={deposit.status}
            />
            <article className="surface detail-surface">
              <div className="proof-route">
                <span className="proof-node">Ethereum lock</span>
                <span>→</span>
                <span className="proof-node accented">Outer action</span>
                <span>→</span>
                <span className="proof-node">Zeko sync</span>
              </div>
              <DetailGrid>
                <Field label="Amount">
                  {formatWei(deposit.ethereumAmount)}
                </Field>
                <Field label="Credited amount">
                  {formatNano(deposit.zekoAmount)}
                </Field>
                <Field label="Ethereum sender" wide>
                  <Address copy>{deposit.sender}</Address>
                </Field>
                <Field label="Zeko recipient" wide>
                  <button
                    className="inline-link"
                    onClick={() =>
                      navigate({
                        page: "account",
                        publicKey: deposit.zekoRecipient,
                      })
                    }
                  >
                    <Address>{deposit.zekoRecipient}</Address>
                  </button>
                </Field>
                <Field label="Ethereum transaction" wide>
                  <a
                    className="external-link"
                    href={`${config.ethereumExplorerUrl}/tx/${deposit.ethereumTransactionHash}`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    <Address>{deposit.ethereumTransactionHash}</Address> ↗
                  </a>
                </Field>
                <Field label="Ethereum finality">
                  {deposit.ethereumFinalized ? "Finalized" : "Confirming"}
                </Field>
                <Field label="Deposit timeout">{deposit.timeout}</Field>
                <Field label="Outer action sequence">
                  {deposit.outerActionSequence ?? "Not proven"}
                </Field>
                <Field label="Synchronized settlement">
                  {deposit.synchronizedSettlementSequence ? (
                    <button
                      className="gold-link inline-link"
                      onClick={() =>
                        navigate({
                          page: "settlement",
                          identifier: deposit.synchronizedSettlementSequence!,
                        })
                      }
                    >
                      #{deposit.synchronizedSettlementSequence}
                    </button>
                  ) : (
                    "Not synchronized"
                  )}
                </Field>
                <Field label="Next action">
                  {sentence(deposit.nextAction ?? "none")}
                </Field>
              </DetailGrid>
              {deposit.accuracyNote ? (
                <div className="accuracy-note">
                  <strong>Finality note</strong>
                  <p>{deposit.accuracyNote}</p>
                </div>
              ) : null}
              <a className="explorer-link" href={config.bridgeUiUrl}>
                Open bridge ↗
              </a>
            </article>
          </>
        )}
      </DetailState>
    </section>
  );
}

export function WithdrawalDetailPage({
  api,
  config,
  navigate,
  sequence,
  offset,
}: DetailProps & { sequence: string; offset: string }) {
  const load = useCallback(
    (signal: AbortSignal) => api.withdrawal(sequence, offset, signal),
    [api, sequence, offset],
  );
  const state = usePolling(
    load,
    config.pollIntervalMs,
    `${sequence}-${offset}`,
  );
  return (
    <section className="detail-page">
      <Back
        label="Bridge activity"
        route={{ page: "bridge" }}
        navigate={navigate}
      />
      <DetailState<WithdrawalRecord> {...state}>
        {(withdrawal) => (
          <>
            <DetailHero
              eyebrow="Zeko → Ethereum"
              title={`Withdrawal ${withdrawal.settlementSequence}:${withdrawal.offset}`}
              copy={`Global inner action ${withdrawal.globalActionIndex}`}
              status={withdrawal.status}
            />
            <article className="surface detail-surface">
              <div className="proof-route">
                <span className="proof-node">Zeko action</span>
                <span>→</span>
                <span className="proof-node accented">Settlement root</span>
                <span>→</span>
                <span className="proof-node">Ethereum claim</span>
              </div>
              <DetailGrid>
                <Field label="Amount">{formatNano(withdrawal.amount)}</Field>
                <Field label="Settlement">
                  <button
                    className="gold-link inline-link"
                    onClick={() =>
                      navigate({
                        page: "settlement",
                        identifier: withdrawal.settlementSequence,
                      })
                    }
                  >
                    #{withdrawal.settlementSequence}
                  </button>
                </Field>
                <Field label="Ethereum recipient" wide>
                  <Address copy>{withdrawal.recipient}</Address>
                </Field>
                <Field label="Action fields hash" wide>
                  <Address copy>{withdrawal.actionFieldsHash}</Address>
                </Field>
                <Field label="Inner action root" wide>
                  <Address copy>{withdrawal.innerActionRoot}</Address>
                </Field>
                <Field label="Commit slot upper">
                  {withdrawal.commitSlotUpper}
                </Field>
                <Field label="Claimable slot">{withdrawal.claimableSlot}</Field>
                <Field label="Current virtual slot">
                  {withdrawal.currentVirtualSlot}
                </Field>
                <Field label="Recipient cursor">
                  {withdrawal.recipientCursor}
                </Field>
                <Field label="Next action">
                  {sentence(withdrawal.nextAction)}
                </Field>
                {withdrawal.claimEthereumTransactionHash ? (
                  <Field label="Claim transaction" wide>
                    <a
                      className="external-link"
                      href={`${config.ethereumExplorerUrl}/tx/${withdrawal.claimEthereumTransactionHash}`}
                      target="_blank"
                      rel="noreferrer"
                    >
                      <Address>
                        {withdrawal.claimEthereumTransactionHash}
                      </Address>{" "}
                      ↗
                    </a>
                  </Field>
                ) : null}
              </DetailGrid>
              <details className="merkle-details">
                <summary>
                  Merkle inclusion path · {withdrawal.siblings.length} siblings
                </summary>
                {withdrawal.siblings.map((sibling, index) => (
                  <div key={`${sibling}-${index}`}>
                    <span>{index}</span>
                    <Address>{sibling}</Address>
                  </div>
                ))}
              </details>
              <a className="explorer-link" href={config.bridgeUiUrl}>
                Open bridge ↗
              </a>
            </article>
          </>
        )}
      </DetailState>
    </section>
  );
}
