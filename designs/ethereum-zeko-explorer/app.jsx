function ExplorerApp() {
  const [page, setPage] = useState("Overview");
  const [selected, setSelected] = useState(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [fresh, setFresh] = useState(false);
  const searchRef = useRef(null);

  const navigate = (next) => { setPage(next); setSelected(null); window.scrollTo({ top: 0, behavior: "smooth" }); };
  const refresh = () => { setFresh(true); window.setTimeout(() => setFresh(false), 650); };
  const searchMatches = query.length > 2 ? [
    { label: "Transaction", value: explorerTransactions[0].hash },
    { label: "Block", value: "18,492 · canonical" },
    { label: "Account", value: "B62qkJzX…x7Kj" }
  ] : [];

  useEffect(() => { if (searchOpen) searchRef.current?.focus(); }, [searchOpen]);

  return <div className="explorer-shell">
    <video className="contours contours-desktop" autoPlay loop muted playsInline src="assets/zeko-contours.webm"></video>
    <video className="contours contours-mobile" autoPlay loop muted playsInline src="assets/zeko-contours-mobile.webm"></video>
    <header className="site-header">
      <button className="brand" onClick={() => navigate("Overview")}><img src="assets/zeko-logo.svg" alt="Zeko" /><span>Explorer</span></button>
      <nav className="desktop-nav" aria-label="Primary">{["Overview", "Transactions", "Settlements", "Bridge"].map((item) => <button className={page === item ? "active" : ""} key={item} onClick={() => navigate(item)}>{item}</button>)}</nav>
      <div className="header-tools">
        <button className="search-trigger" onClick={() => setSearchOpen(true)}><Icon name="search" /><span>Search blocks, transactions, addresses</span><kbd>/</kbd></button>
        <span className="network-pill"><span className="live-dot"></span>Zeko Testnet</span>
        <a className="bridge-cta" href="#bridge">Bridge <Icon name="arrow" /></a>
      </div>
    </header>

    <main className="page" data-screen-label={page}>
      {page === "Overview" && <Overview onNavigate={navigate} onSelect={setSelected} fresh={fresh} onRefresh={refresh} />}
      {page === "Transactions" && <ListPage title="L2 transactions" copy="One sequenced transaction per Zeko block." tabs={["All", "zkApp", "Payment", "Delegation"]}><TransactionTable onSelect={setSelected} /></ListPage>}
      {page === "Settlements" && <ListPage title="Ethereum settlements" copy="Pickles state transitions verified through SP1 and accepted on Sepolia." tabs={["All", "Proving", "Submitted", "Confirmed", "Failed"]}><SettlementList onSelect={setSelected} /></ListPage>}
      {page === "Bridge" && <ListPage title="Bridge activity" copy="Native ETH deposits and settlement-bound withdrawals." tabs={["All", "Deposits", "Withdrawals", "Action required"]}><BridgeList onSelect={setSelected} /></ListPage>}
    </main>

    <nav className="mobile-nav" aria-label="Mobile navigation">{["Overview", "Transactions", "Settlements", "Bridge"].map((item) => <button className={page === item ? "active" : ""} key={item} onClick={() => navigate(item)}><span>{item.slice(0, 1)}</span>{item}</button>)}</nav>

    {searchOpen && <div className="search-layer" onMouseDown={(event) => event.target === event.currentTarget && setSearchOpen(false)}>
      <div className="search-dialog" role="dialog" aria-modal="true" aria-label="Global search">
        <div className="search-input"><Icon name="search" /><input ref={searchRef} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Block height, hash, B62 or 0x address" /><button onClick={() => setSearchOpen(false)}>Esc</button></div>
        <div className="search-results">{searchMatches.length ? searchMatches.map((match) => <button key={match.label} onClick={() => { setSearchOpen(false); setSelected({ kind: match.label, id: match.value, status: "Applied" }); }}><span>{match.label}</span><Address>{match.value}</Address><Icon name="chevron" /></button>) : <p>{query ? "Keep typing to search every indexed source." : "Search across Zeko L2, Ethereum settlements and the native bridge."}</p>}</div>
      </div>
    </div>}
    <DetailDrawer item={selected} onClose={() => setSelected(null)} />
  </div>;
}

function Overview({ onNavigate, onSelect, fresh, onRefresh }) {
  return <>
    <section className="overview-hero">
      <div><span className="hero-kicker">Ethereum-settled Zeko L2</span><h1>Network activity, from execution to settlement.</h1><p>Inspect every Zeko transaction, SP1-verified settlement and native bridge operation in one canonical view.</p></div>
      <div className="hero-state"><span className="live-dot"></span><div><strong>Network operational</strong><span>Last indexed 4 sec ago</span></div><button className={fresh ? "spinning" : ""} onClick={onRefresh} aria-label="Refresh data"><Icon name="refresh" /></button></div>
    </section>
    <section className="metrics-grid">
      <article><span>L2 block height</span><strong>18,492</strong><small>1 txn per block</small></article>
      <article><span>Latest settlement</span><strong>#0284</strong><small>64 blocks · confirmed</small></article>
      <article><span>Transactions</span><strong>18,491</strong><small>99.84% applied</small></article>
      <article><span>Bridge volume</span><strong>24.82 ETH</strong><small>147 deposits · 39 withdrawals</small></article>
    </section>
    <section className="overview-grid">
      <article className="surface wide-surface"><SectionHeading eyebrow="Execution" title="Latest L2 transactions" action={<button className="text-button" onClick={() => onNavigate("Transactions")}>View all <Icon name="arrow" /></button>} /><TransactionTable compact onSelect={onSelect} /></article>
      <aside className="surface settlement-card"><SectionHeading eyebrow="Settlement" title="Latest proof" /><button className="proof-card" onClick={() => onSelect({ kind: "Settlement", ...explorerSettlements[0] })}><div className="proof-head"><span className="proof-emblem">SP1</span><Status>Confirmed</Status></div><strong>Settlement #0284</strong><span>64 L2 blocks committed to Ethereum</span><div className="slot-range"><span>Slot range</span><Address>91,884 → 91,948</Address></div><div className="proof-progress"><span></span></div><small>18 / 18 confirmations</small></button><button className="text-button full" onClick={() => onNavigate("Settlements")}>All settlements <Icon name="arrow" /></button></aside>
    </section>
    <section className="surface bridge-surface" id="bridge"><SectionHeading eyebrow="Cross-chain" title="Recent bridge activity" action={<button className="text-button" onClick={() => onNavigate("Bridge")}>View all <Icon name="arrow" /></button>} /><BridgeList onSelect={onSelect} /></section>
    <footer><span>Zeko Testnet · Mina <code>testnet</code> signing domain</span><span>Ethereum settlement · Sepolia · Experimental</span></footer>
  </>;
}

function ListPage({ title, copy, tabs, children }) {
  const [tab, setTab] = useState(tabs[0]);
  return <section className="list-page"><div className="list-hero"><div><span className="hero-kicker">Zeko Explorer</span><h1>{title}</h1><p>{copy}</p></div><div className="list-meta"><span className="live-dot"></span>Live · updated 4 sec ago</div></div><div className="surface"><div className="filter-bar"><div className="filter-tabs">{tabs.map((item) => <button className={tab === item ? "active" : ""} onClick={() => setTab(item)} key={item}>{item}</button>)}</div><button className="refresh-button"><Icon name="refresh" /> Refresh</button></div>{children}<div className="pagination"><button disabled>Previous</button><span>Showing newest records</span><button>Next</button></div></div></section>;
}

ReactDOM.createRoot(document.getElementById("root")).render(<ExplorerApp />);
