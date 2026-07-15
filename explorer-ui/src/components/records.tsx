import type { Route } from "../lib/router";
import type {
  BlockRecord,
  DepositRecord,
  SettlementRecord,
  TransactionRecord,
  WithdrawalRecord,
} from "../lib/types";
import {
  compact,
  formatNano,
  formatWei,
  sentence,
  timeAgo,
} from "../lib/format";
import { Icon, ShortAddress, Status } from "./ui";

export function TransactionTable({
  items,
  navigate,
}: {
  items: TransactionRecord[];
  navigate: (route: Route) => void;
}) {
  return (
    <div className="data-table transaction-table">
      <div className="table-head">
        <span>Transaction</span>
        <span>Block</span>
        <span>Type</span>
        <span>Status</span>
        <span>Age</span>
        <span />
      </div>
      {items.map((row) => (
        <button
          className="table-row"
          key={`${row.hash}-${row.blockHeight}`}
          onClick={() => navigate({ page: "transaction", hash: row.hash })}
        >
          <span className="primary-cell">
            <span className="transaction-mark">
              <Icon name="block" />
            </span>
            <span>
              <strong className="mono">{compact(row.hash, 12, 5)}</strong>
              <small>Fee payer {compact(row.feePayer, 9, 4)}</small>
            </span>
          </span>
          <strong className="gold-link">{row.blockHeight}</strong>
          <span>{sentence(row.kind)}</span>
          <Status>{row.status}</Status>
          <span className="muted">{timeAgo(row.timestamp)}</span>
          <Icon name="chevron" />
        </button>
      ))}
    </div>
  );
}

export function BlockTable({
  items,
  navigate,
}: {
  items: BlockRecord[];
  navigate: (route: Route) => void;
}) {
  return (
    <div className="data-table block-table">
      <div className="table-head">
        <span>Block</span>
        <span>Transaction</span>
        <span>Status</span>
        <span>Age</span>
        <span />
      </div>
      {items.map((row) => (
        <button
          className="table-row"
          key={row.stateHash}
          onClick={() => navigate({ page: "block", identifier: row.height })}
        >
          <span className="primary-cell">
            <span className="transaction-mark">
              <Icon name="block" />
            </span>
            <span>
              <strong>Block {row.height}</strong>
              <small className="mono">{compact(row.stateHash, 11, 5)}</small>
            </span>
          </span>
          <span>
            {row.transaction ? (
              <span className="mono">
                {compact(row.transaction.hash, 11, 5)}
              </span>
            ) : (
              "No transaction"
            )}
          </span>
          <Status>{row.chainStatus}</Status>
          <span className="muted">{timeAgo(row.timestamp)}</span>
          <Icon name="chevron" />
        </button>
      ))}
    </div>
  );
}

export function SettlementList({
  items,
  navigate,
}: {
  items: SettlementRecord[];
  navigate: (route: Route) => void;
}) {
  return (
    <div className="settlement-list">
      {items.map((row) => (
        <button
          className="settlement-row"
          key={row.id}
          onClick={() =>
            navigate({
              page: "settlement",
              identifier: row.batchSequence ?? row.id,
            })
          }
        >
          <span className="settlement-orb">
            <span>Ξ</span>
            <small>SP1</small>
          </span>
          <span className="settlement-main">
            <span className="row-title">
              <strong>
                Settlement{" "}
                {row.batchSequence ? `#${row.batchSequence}` : compact(row.id)}
              </strong>
              <Status>{row.status}</Status>
            </span>
            <span className="muted">
              Slots {row.slotLower ?? "—"}–{row.slotUpper ?? "—"} · command
              digest {compact(row.settlementCommandDigest)}
            </span>
          </span>
          <span className="settlement-effect">
            <strong>
              {row.innerActionCount
                ? `${row.innerActionCount} bridge actions`
                : "No indexed bridge actions"}
            </strong>
            <small>{timeAgo(row.createdAt)}</small>
          </span>
          <Icon name="chevron" />
        </button>
      ))}
    </div>
  );
}

export function DepositList({
  items,
  navigate,
}: {
  items: DepositRecord[];
  navigate: (route: Route) => void;
}) {
  return (
    <div className="bridge-list">
      {items.map((row) => (
        <button
          className="bridge-row"
          key={row.nonce}
          onClick={() => navigate({ page: "deposit", nonce: row.nonce })}
        >
          <span className="route-icon deposit">
            <span>Ξ</span>
            <span>→</span>
          </span>
          <span className="bridge-main">
            <span className="row-title">
              <strong>Deposit #{row.nonce}</strong>
              <Status>{row.status}</Status>
            </span>
            <span className="muted">
              Ethereum → Zeko · {compact(row.zekoRecipient, 10, 5)}
            </span>
          </span>
          <span className="amount-cell">
            <strong>{formatWei(row.ethereumAmount)}</strong>
            <small>Ethereum block {row.ethereumBlockNumber}</small>
          </span>
          <Icon name="chevron" />
        </button>
      ))}
    </div>
  );
}

export function WithdrawalList({
  items,
  navigate,
}: {
  items: WithdrawalRecord[];
  navigate: (route: Route) => void;
}) {
  return (
    <div className="bridge-list">
      {items.map((row) => (
        <button
          className="bridge-row"
          key={`${row.settlementSequence}-${row.offset}`}
          onClick={() =>
            navigate({
              page: "withdrawal",
              sequence: row.settlementSequence,
              offset: String(row.offset),
            })
          }
        >
          <span className="route-icon withdrawal">
            <span>Z</span>
            <span>→</span>
          </span>
          <span className="bridge-main">
            <span className="row-title">
              <strong>
                Withdrawal {row.settlementSequence}:{row.offset}
              </strong>
              <Status>{row.status}</Status>
            </span>
            <span className="muted">
              Zeko → Ethereum · {compact(row.recipient, 10, 5)}
            </span>
          </span>
          <span className="amount-cell">
            <strong>{formatNano(row.amount)}</strong>
            <small>Claim slot {row.claimableSlot}</small>
          </span>
          <Icon name="chevron" />
        </button>
      ))}
    </div>
  );
}
