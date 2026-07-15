const { useEffect, useRef, useState } = React;

function Icon({ name }) {
  const icons = {
    search: "⌕", refresh: "↻", arrow: "↗", chevron: "›", block: "▦", proof: "SP1"
  };
  return <span className={`glyph glyph-${name}`} aria-hidden="true">{icons[name]}</span>;
}

function Status({ children }) {
  const tone = children.toLowerCase().replaceAll(" ", "-");
  return <span className={`status status-${tone}`}><span className="status-dot"></span>{children}</span>;
}

function Address({ children }) {
  return <span className="mono address">{children}</span>;
}

function SectionHeading({ eyebrow, title, action }) {
  return <div className="section-heading"><div><span className="section-eyebrow">{eyebrow}</span><h2>{title}</h2></div>{action}</div>;
}

function TransactionTable({ compact = false, onSelect }) {
  return <div className="data-table transaction-table">
    <div className="table-head"><span>Transaction</span><span>Block</span><span>Type</span><span>Status</span><span>Age</span><span></span></div>
    {explorerTransactions.slice(0, compact ? 4 : undefined).map((row) => <button className="table-row" key={row.hash} onClick={() => onSelect({ kind: "Transaction", ...row })}>
      <span className="primary-cell"><span className="transaction-mark"><Icon name="block" /></span><span><strong className="mono">{row.hash.slice(0, 12)}…{row.hash.slice(-5)}</strong><small>From {row.from}</small></span></span>
      <strong className="gold-link">{row.block}</strong><span>{row.type}</span><Status>{row.status}</Status><span className="muted">{row.age}</span><Icon name="chevron" />
    </button>)}
  </div>;
}

function SettlementList({ onSelect }) {
  return <div className="settlement-list">{explorerSettlements.map((row) => <button className="settlement-row" key={row.id} onClick={() => onSelect({ kind: "Settlement", ...row })}>
    <span className="settlement-orb"><span>Ξ</span><small>SP1</small></span>
    <span className="settlement-main"><span className="row-title"><strong>Settlement {row.id}</strong><Status>{row.status}</Status></span><span className="muted">Slots {row.slot} · {row.blocks} L2 blocks</span></span>
    <span className="settlement-effect"><strong>{row.bridge}</strong><small>{row.age}</small></span><Icon name="chevron" />
  </button>)}</div>;
}

function BridgeList({ onSelect }) {
  return <div className="bridge-list">{explorerBridge.map((row) => <button className="bridge-row" key={row.id} onClick={() => onSelect({ kind: "Bridge operation", ...row })}>
    <span className={`route-icon ${row.kind}`}><span>{row.kind === "deposit" ? "Ξ" : "Z"}</span><span>→</span></span>
    <span className="bridge-main"><span className="row-title"><strong>{row.id}</strong><Status>{row.status}</Status></span><span className="muted">{row.route} · {row.account}</span></span>
    <span className="amount-cell"><strong>{row.amount}</strong><small>{row.age}</small></span><Icon name="chevron" />
  </button>)}</div>;
}

function DetailDrawer({ item, onClose }) {
  useEffect(() => {
    if (!item) return undefined;
    const close = (event) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [item, onClose]);
  if (!item) return null;
  const transaction = item.kind === "Transaction";
  return <div className="drawer-layer" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
    <aside className="detail-drawer" role="dialog" aria-modal="true" aria-label={`${item.kind} details`}>
      <div className="drawer-top"><div><span className="section-eyebrow">{item.kind}</span><h2>{transaction ? `${item.hash.slice(0, 14)}…` : item.id}</h2></div><button className="close-button" onClick={onClose} aria-label="Close details">×</button></div>
      <div className="drawer-status"><Status>{item.status}</Status><span>Canonical network record</span></div>
      <div className="detail-grid">
        <div><span>Included</span><strong>{transaction ? `Block ${item.block}` : item.age}</strong></div>
        <div><span>{transaction ? "Command" : "Lifecycle"}</span><strong>{transaction ? item.type : item.route || item.slot}</strong></div>
        <div className="wide"><span>Identifier</span><Address>{transaction ? item.hash : item.tx || item.account}</Address></div>
        {!transaction && item.amount && <div><span>Amount</span><strong>{item.amount}</strong></div>}
        {!transaction && item.bridge && <div className="wide"><span>Bridge effects</span><strong>{item.bridge}</strong></div>}
      </div>
      <div className="proof-route"><span className="proof-node">Zeko L2</span><span>→</span><span className="proof-node accented">SP1 verified</span><span>→</span><span className="proof-node">Ethereum</span></div>
      <button className="explorer-link">Open full detail page <Icon name="arrow" /></button>
    </aside>
  </div>;
}

Object.assign(window, { Icon, Status, Address, SectionHeading, TransactionTable, SettlementList, BridgeList, DetailDrawer });
