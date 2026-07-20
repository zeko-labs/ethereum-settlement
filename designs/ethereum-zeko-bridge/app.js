const { useEffect, useMemo, useRef, useState } = preactHooks;

const ROUTES = {
  deposit: {
    title: "Deposit to Zeko",
    source: "ethereum",
    destination: "zeko",
    sourceName: "Ethereum Sepolia",
    destinationName: "Zeko Testnet",
    sourceType: "Settlement & custody",
    destinationType: "Execution network",
    balance: 4.82,
    wallet: "0x71c8…a0e2",
    recipient: "B62qqF9s8cY4wT2JcZ7f1Vx6iMmR8YpQe4uJ1L9sK",
    cta: "Review deposit",
    reviewCta: "Confirm in Ethereum wallet",
    finalCta: "Finalize on Zeko",
    routeLabel: "ETH lock → SP1 proof → Zeko finalization",
    walletNotice: "You will first confirm the ETH lock in your Ethereum wallet. Finalization on Zeko uses your connected Zeko wallet.",
    steps: [
      {
        short: "Lock ETH",
        title: "Confirming Ethereum deposit",
        body: "Waiting for the custody transaction to reach Ethereum finality."
      },
      {
        short: "SP1 proof",
        title: "Proving the deposit batch",
        body: "The gateway is proving the ordered Ethereum deposits for Zeko."
      },
      {
        short: "Sync settlement",
        title: "Synchronizing Zeko settlement",
        body: "The proven deposit is being synchronized into Zeko's outer action state."
      },
      {
        short: "Finalize",
        title: "Ready to finalize on Zeko",
        body: "Sign the final Zeko transaction to credit native ETH to your recipient."
      }
    ]
  },
  withdrawal: {
    title: "Withdraw to Ethereum",
    source: "zeko",
    destination: "ethereum",
    sourceName: "Zeko Testnet",
    destinationName: "Ethereum Sepolia",
    sourceType: "Execution network",
    destinationType: "Settlement & custody",
    balance: 1.64,
    wallet: "B62qqF…L9sK",
    recipient: "0x71c8B0f42D6fD8bA5C12c6A83529E1D4D24Aa0e2",
    cta: "Review withdrawal",
    reviewCta: "Confirm in Zeko wallet",
    finalCta: "Claim ETH on Ethereum",
    routeLabel: "Zeko request → settlement proof → ETH claim",
    walletNotice: "You will first sign the withdrawal request on Zeko. Ethereum releases custody only after the settlement proof and withdrawal delay.",
    steps: [
      {
        short: "Request",
        title: "Submitting Zeko withdrawal",
        body: "Waiting for your withdrawal action to be included in a committed Zeko state."
      },
      {
        short: "Settle proof",
        title: "Settling the withdrawal root",
        body: "SP1 is proving the committed inner action state for Ethereum verification."
      },
      {
        short: "Safety delay",
        title: "Waiting for withdrawal delay",
        body: "Ethereum custody remains locked until the configured settlement-slot delay passes."
      },
      {
        short: "Claim ETH",
        title: "Withdrawal is claimable",
        body: "The inclusion proof is ready. Claim native ETH from the bridge escrow."
      }
    ]
  }
};

function NetworkIcon({ network, compact = false }) {
  if (network === "zeko") {
    return (
      <span className={`network-icon zeko${compact ? " compact" : ""}`} aria-hidden="true">
        <img src="assets/zeko-token.png" alt="" />
      </span>
    );
  }

  if (network === "proof") {
    return <span className="network-icon proof" aria-hidden="true">SP1</span>;
  }

  return (
    <span className={`network-icon ethereum${compact ? " compact" : ""}`} aria-hidden="true">
      <span className="eth-mark">◆</span>
    </span>
  );
}

function BackgroundWave({ className, src, storageKey }) {
  const videoRef = useRef(null);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return undefined;

    const restorePlayback = () => {
      try {
        const savedTime = Number(window.localStorage.getItem(storageKey));
        if (Number.isFinite(savedTime) && savedTime > 0 && savedTime < video.duration) {
          video.currentTime = savedTime;
        }
      } catch {
        // Storage can be unavailable in privacy-restricted browser contexts.
      }
    };

    const savePlayback = () => {
      try {
        window.localStorage.setItem(storageKey, String(video.currentTime));
      } catch {
        // The ambient animation still works without persistence.
      }
    };

    video.addEventListener("loadedmetadata", restorePlayback);
    video.addEventListener("timeupdate", savePlayback);
    return () => {
      video.removeEventListener("loadedmetadata", restorePlayback);
      video.removeEventListener("timeupdate", savePlayback);
    };
  }, [storageKey]);

  return (
    <video ref={videoRef} className={`contour-background ${className}`} autoPlay loop muted playsInline aria-hidden="true">
      <source src={src} type="video/webm" />
    </video>
  );
}

function WalletChip({ network, onClick }) {
  const isEthereum = network === "ethereum";
  return (
    <button
      type="button"
      className={`wallet-chip ${isEthereum ? "ethereum-wallet" : "zeko-wallet"}`}
      onClick={onClick}
      aria-label={`Connected ${isEthereum ? "Ethereum" : "Zeko"} wallet`}
    >
      <NetworkIcon network={network} compact />
      <span className="wallet-copy">
        <span className="wallet-network">{isEthereum ? "Sepolia" : "Zeko Testnet"}</span>
        <span className="wallet-address">{isEthereum ? "0x71c8…a0e2" : "B62qqF…L9sK"}</span>
      </span>
    </button>
  );
}

function AppHeader({ notify }) {
  return (
    <header className="site-header">
      <a className="brand-link" href="#" onClick={(event) => event.preventDefault()} aria-label="Zeko bridge home">
        <img src="assets/zeko-logo.svg" alt="Zeko" />
      </a>
      <div className="network-health" title="Prototype network state">
        <span className="health-dot"></span>
        <span>Sepolia settlement · Experimental</span>
      </div>
      <div className="header-actions">
        <WalletChip network="ethereum" onClick={() => notify("Ethereum wallet is connected to Sepolia.")} />
        <WalletChip network="zeko" onClick={() => notify("Zeko wallet is connected to Zeko Testnet.")} />
      </div>
    </header>
  );
}

function TransferNetwork({ network, name, type, recipient, editingRecipient, setEditingRecipient, setRecipient }) {
  return (
    <div className="network-side">
      <div className="network-pill">
        <NetworkIcon network={network} />
        <span className="network-pill-copy">
          <span className="network-name">{name}</span>
          <span className="network-type">{type}</span>
        </span>
        <span className="token-label">ETH</span>
      </div>
      {editingRecipient ? (
        <div className="recipient-editor">
          <input
            className="recipient-input"
            value={recipient}
            onInput={(event) => setRecipient(event.currentTarget.value)}
            aria-label="Recipient address"
            autoFocus
          />
          <button type="button" className="edit-recipient" onClick={() => setEditingRecipient(false)}>Done</button>
        </div>
      ) : (
        <div className="recipient-row">
          <span>Recipient</span>
          <strong title={recipient}>{recipient}</strong>
          <button type="button" className="edit-recipient" onClick={() => setEditingRecipient(true)}>Edit</button>
        </div>
      )}
    </div>
  );
}

function RouteSummary({ route, direction, open, onToggle, timeout }) {
  const deposit = direction === "deposit";
  return (
    <>
      <div className="route-summary">
        <div className="route-line">
          <span className="route-label">Route</span>
          <span className="route-value">
            <span className="route-node">{deposit ? "Ethereum" : "Zeko"}</span>
            <span className="route-arrow">→</span>
            <span className="route-node"><NetworkIcon network="proof" compact /> SP1</span>
            <span className="route-arrow">→</span>
            <span className="route-node">{deposit ? "Zeko" : "Ethereum"}</span>
          </span>
        </div>
        <button type="button" className="details-button" onClick={onToggle} aria-expanded={open}>
          {open ? "Hide details" : "Route details"}
        </button>
      </div>
      {open && (
        <div className="route-details">
          <div className="detail-cell">
            <span className="detail-label">Asset</span>
            <span className="detail-value">Native ETH · 1:1</span>
          </div>
          <div className="detail-cell">
            <span className="detail-label">Custody</span>
            <span className="detail-value">Ethereum bridge escrow</span>
          </div>
          <div className="detail-cell">
            <span className="detail-label">{deposit ? "Cancellation" : "Release rule"}</span>
            <span className="detail-value">{deposit ? `After ${timeout}h timeout` : "Settlement delay + proof"}</span>
          </div>
        </div>
      )}
    </>
  );
}

function BridgeForm({
  direction,
  onSwap,
  amount,
  setAmount,
  recipient,
  setRecipient,
  onReview,
  showDetails,
  setShowDetails,
  timeout
}) {
  const route = ROUTES[direction];
  const [editingRecipient, setEditingRecipient] = useState(false);
  const numericAmount = Number(amount || 0);
  const validNumber = Number.isFinite(numericAmount) && numericAmount > 0;
  const exceedsBalance = validNumber && numericAmount > route.balance;
  const recipientLooksValid = direction === "deposit"
    ? recipient.startsWith("B62") && recipient.length > 20
    : recipient.startsWith("0x") && recipient.length === 42;
  const canReview = validNumber && !exceedsBalance && recipientLooksValid;

  const updateAmount = (value) => {
    const normalized = value.replace(/[^0-9.]/g, "");
    const [whole, ...decimalParts] = normalized.split(".");
    setAmount(decimalParts.length ? `${whole}.${decimalParts.join("").slice(0, 9)}` : whole);
  };

  return (
    <section className="bridge-form" data-screen-label="Bridge form">
      <div className="form-heading">
        <h2>{route.title}</h2>
        <div className="route-kicker">
          <strong>Native ETH</strong>
          <span>·</span>
          <span>Verified settlement route</span>
        </div>
      </div>

      <div className="transfer-surface">
        <div className="transfer-panel source-panel">
          <div className="amount-side">
            <div className="field-topline">
              <span>You send</span>
              <button type="button" className="balance-action" onClick={() => setAmount(String(route.balance))}>
                Balance {route.balance.toFixed(2)} ETH · Max
              </button>
            </div>
            <div className="amount-row">
              <input
                className="amount-input"
                value={amount}
                inputMode="decimal"
                placeholder="0.00"
                onInput={(event) => updateAmount(event.currentTarget.value)}
                aria-label="Amount of native ETH to bridge"
              />
            </div>
            <span className="fiat-value">Native ETH · precision capped at 9 decimals on Zeko</span>
          </div>
          <div className="network-side">
            <div className="network-pill">
              <NetworkIcon network={route.source} />
              <span className="network-pill-copy">
                <span className="network-name">{route.sourceName}</span>
                <span className="network-type">{route.sourceType}</span>
              </span>
              <span className="token-label">ETH</span>
            </div>
            <div className="recipient-row">
              <span>Connected</span>
              <strong>{route.wallet}</strong>
            </div>
          </div>
        </div>

        <button type="button" className="swap-button" onClick={onSwap} aria-label="Reverse bridge direction">
          <span>↕</span>
        </button>

        <div className="transfer-panel destination-panel">
          <div className="amount-side">
            <div className="field-topline">
              <span>You receive</span>
              <span>Bridge fee 0 ETH</span>
            </div>
            <div className={`amount-input amount-output${validNumber ? "" : " empty"}`}>
              {validNumber ? numericAmount.toLocaleString(undefined, { maximumFractionDigits: 9 }) : "0.00"}
            </div>
            <span className="fiat-value">Network gas is paid separately in the signing wallet</span>
          </div>
          <TransferNetwork
            network={route.destination}
            name={route.destinationName}
            type={route.destinationType}
            recipient={recipient}
            editingRecipient={editingRecipient}
            setEditingRecipient={setEditingRecipient}
            setRecipient={setRecipient}
          />
        </div>
      </div>

      {exceedsBalance && <div className="validation-message"><span>!</span> Amount exceeds the connected wallet balance.</div>}
      {!editingRecipient && recipient.length > 0 && !recipientLooksValid && (
        <div className="validation-message"><span>!</span> Enter a valid {direction === "deposit" ? "Zeko" : "Ethereum"} recipient.</div>
      )}

      <RouteSummary
        route={route}
        direction={direction}
        open={showDetails}
        onToggle={() => setShowDetails(!showDetails)}
        timeout={timeout}
      />

      <div className="notice">
        <span className="notice-mark">i</span>
        <span>{route.walletNotice}</span>
      </div>

      <button type="button" className="primary-button" disabled={!canReview} onClick={onReview}>
        {route.cta}
        <span aria-hidden="true">→</span>
      </button>
    </section>
  );
}

function ReviewView({ direction, amount, recipient, timeout, onBack, onConfirm }) {
  const route = ROUTES[direction];
  return (
    <section className="review-view" data-screen-label="Review transfer">
      <div className="review-top">
        <div className="review-title">
          <h2>Review {direction === "deposit" ? "deposit" : "withdrawal"}</h2>
          <p>Confirm the route and recipient before opening your wallet.</p>
        </div>
        <div className="amount-lockup">
          <strong>{amount} ETH</strong>
          <span>Native asset</span>
        </div>
      </div>

      <div className="review-route">
        <div className="review-network">
          <NetworkIcon network={route.source} />
          <strong>{route.sourceName}</strong>
          <span>{route.sourceType}</span>
        </div>
        <span className="review-arrow">→</span>
        <div className="review-network">
          <NetworkIcon network={route.destination} />
          <strong>{route.destinationName}</strong>
          <span>{route.destinationType}</span>
        </div>
      </div>

      <div className="summary-list">
        <div className="summary-row"><span>Recipient</span><strong title={recipient}>{recipient}</strong></div>
        <div className="summary-row"><span>Amount received</span><strong>{amount} ETH</strong></div>
        <div className="summary-row"><span>Bridge fee</span><strong>0 ETH</strong></div>
        <div className="summary-row"><span>Network gas</span><strong>Estimated in wallet</strong></div>
        <div className="summary-row">
          <span>{direction === "deposit" ? "Cancellation timeout" : "Withdrawal release"}</span>
          <strong>{direction === "deposit" ? `${timeout} hours` : "After settlement delay"}</strong>
        </div>
      </div>

      <div className="proof-note">
        <NetworkIcon network="proof" />
        <span><strong>Proof-bound settlement.</strong> The bridge transition is accepted only after SP1 verifies the Zeko proof and Ethereum verifies the committed public values.</span>
      </div>

      <div className="button-row">
        <button type="button" className="secondary-button" onClick={onBack}>Back</button>
        <button type="button" className="primary-button" onClick={onConfirm}>{route.reviewCta}</button>
      </div>
    </section>
  );
}

function ProgressView({ direction, amount, step, onClaim, notify }) {
  const route = ROUTES[direction];
  const current = route.steps[step];
  const finalStep = step === route.steps.length - 1;

  return (
    <section className="progress-view" data-screen-label="Transfer progress">
      <div className="progress-top">
        <div className="progress-title">
          <h2>{direction === "deposit" ? "Deposit in progress" : "Withdrawal in progress"}</h2>
          <p>Your funds remain tracked through each proof and settlement state.</p>
        </div>
        <div className="amount-lockup">
          <strong>{amount} ETH</strong>
          <span>{route.sourceName} → {route.destinationName}</span>
        </div>
      </div>

      <div className="progress-track" aria-label={`Step ${step + 1} of ${route.steps.length}`}>
        {route.steps.map((item, index) => {
          const done = index < step;
          const isCurrent = index === step;
          return (
            <div key={item.short} className={`progress-step${done ? " done" : ""}${isCurrent ? " current" : ""}`}>
              <span className="progress-node">{done ? "✓" : index + 1}</span>
              <span className="progress-label">{item.short}</span>
            </div>
          );
        })}
      </div>

      <div className="current-status">
        <span className="status-orbit">◌</span>
        <div className="status-copy">
          <strong>{current.title}</strong>
          <p>{current.body}</p>
        </div>
        <span className="status-time">{finalStep ? "Action required" : "Auto-refreshing"}</span>
      </div>

      <div className="summary-list">
        <div className="summary-row"><span>Operation</span><strong>{direction === "deposit" ? "Deposit #128" : "Withdrawal 47:03"}</strong></div>
        <div className="summary-row"><span>Settlement</span><strong>Ethereum Sepolia</strong></div>
        <div className="summary-row"><span>Proof system</span><strong>SP1 · Zeko state transition</strong></div>
      </div>

      <div className="progress-actions">
        <button type="button" className="text-action" onClick={() => notify("Explorer links are disabled in this design prototype.")}>View operation details</button>
        <button type="button" className="primary-button claim-button" disabled={!finalStep} onClick={onClaim}>
          {finalStep ? route.finalCta : "Waiting for settlement"}
        </button>
      </div>
    </section>
  );
}

function CompleteView({ direction, amount, recipient, onNewTransfer, onActivity }) {
  const route = ROUTES[direction];
  return (
    <section className="complete-view" data-screen-label="Transfer complete">
      <div className="complete-mark">✓</div>
      <div>
        <h2>{direction === "deposit" ? "Deposit finalized" : "Withdrawal claimed"}</h2>
        <p>{amount} ETH is now available on {route.destinationName}.</p>
      </div>
      <div className="complete-amount">{amount} ETH</div>
      <div className="complete-details">Settlement and bridge proofs verified · Recipient {recipient.slice(0, 10)}…</div>
      <div className="complete-actions">
        <button type="button" className="secondary-button" onClick={onActivity}>View activity</button>
        <button type="button" className="primary-button" onClick={onNewTransfer}>New transfer</button>
      </div>
    </section>
  );
}

function ActivityRouteIcon({ direction }) {
  const source = direction === "deposit" ? "ethereum" : "zeko";
  const destination = direction === "deposit" ? "zeko" : "ethereum";
  return (
    <span className="activity-route-icon" aria-hidden="true">
      <NetworkIcon network={source} />
      <NetworkIcon network={destination} />
    </span>
  );
}

function ActivityView({ latest }) {
  const rows = useMemo(() => {
    const base = [
      {
        id: "wd-47-03",
        direction: "withdrawal",
        amount: "0.420",
        title: "Withdrawal to Ethereum",
        status: "Ready to claim",
        kind: "ready",
        detail: "Settlement 47 · action 03",
        time: "18 min ago"
      },
      {
        id: "dp-121",
        direction: "deposit",
        amount: "1.100",
        title: "Deposit to Zeko",
        status: "Completed",
        kind: "complete",
        detail: "Ethereum deposit #121",
        time: "Yesterday"
      }
    ];
    return latest ? [latest, ...base] : base;
  }, [latest]);

  return (
    <section className="activity-view" data-screen-label="Bridge activity">
      <div className="activity-heading">
        <div>
          <h2>Bridge activity</h2>
          <p>Transfers are recovered from Ethereum and Zeko state after reconnecting.</p>
        </div>
        <span className="prototype-badge">Prototype data</span>
      </div>
      <div className="activity-list">
        {rows.map((row) => (
          <div className="activity-row" key={row.id}>
            <ActivityRouteIcon direction={row.direction} />
            <div className="activity-main">
              <div className="activity-primary">
                <span>{row.amount} ETH</span>
                <span>·</span>
                <span>{row.title}</span>
                <span className={`status-badge ${row.kind}`}>{row.status}</span>
              </div>
              <div className="activity-secondary">{row.detail}</div>
            </div>
            <div className="activity-time">{row.time}</div>
          </div>
        ))}
      </div>
    </section>
  );
}

function SettingsModal({ timeout, setTimeoutHours, expert, setExpert, onClose }) {
  useEffect(() => {
    const onKeyDown = (event) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <div className="settings-overlay" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div className="settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <div className="modal-header">
          <h2 id="settings-title">Bridge settings</h2>
          <button type="button" className="close-button" onClick={onClose} aria-label="Close settings">×</button>
        </div>
        <div className="modal-body">
          <div className="setting-group">
            <span className="setting-label">Ethereum settlement network</span>
            <select className="select-field" defaultValue="sepolia" aria-label="Ethereum settlement network">
              <option value="sepolia">Ethereum Sepolia · chain 11155111</option>
              <option value="anvil">Local Anvil · chain 31337</option>
            </select>
          </div>
          <div className="setting-group">
            <span className="setting-label">Deposit cancellation timeout</span>
            <p className="setting-help">A deposit can be cancelled only if the timeout wins before a synchronized commit accepts it.</p>
            <div className="timeout-options">
              {[12, 24, 36].map((hours) => (
                <button
                  type="button"
                  key={hours}
                  className={`timeout-option${timeout === hours ? " active" : ""}`}
                  onClick={() => setTimeoutHours(hours)}
                >
                  {hours} hours
                </button>
              ))}
            </div>
          </div>
          <div className="toggle-row">
            <div>
              <div className="setting-label">Show proof details</div>
              <p className="setting-help">Expose the settlement route on the bridge form.</p>
            </div>
            <button
              type="button"
              className={`switch${expert ? " active" : ""}`}
              onClick={() => setExpert(!expert)}
              aria-pressed={expert}
              aria-label="Show proof details"
            ></button>
          </div>
        </div>
      </div>
    </div>
  );
}

function App() {
  const [tab, setTab] = useState("bridge");
  const [direction, setDirection] = useState("deposit");
  const [amount, setAmount] = useState("");
  const [recipient, setRecipient] = useState(ROUTES.deposit.recipient);
  const [screen, setScreen] = useState("form");
  const [progressStep, setProgressStep] = useState(0);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [showDetails, setShowDetails] = useState(true);
  const [timeout, setTimeoutHours] = useState(24);
  const [toast, setToast] = useState("");
  const [latestActivity, setLatestActivity] = useState(null);

  const route = ROUTES[direction];

  useEffect(() => {
    if (!toast) return undefined;
    const timer = window.setTimeout(() => setToast(""), 2600);
    return () => window.clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    if (screen !== "progress" || progressStep >= route.steps.length - 1) return undefined;
    const timer = window.setTimeout(() => setProgressStep((current) => Math.min(current + 1, route.steps.length - 1)), 1900);
    return () => window.clearTimeout(timer);
  }, [screen, progressStep, route.steps.length]);

  const notify = (message) => {
    setToast("");
    window.setTimeout(() => setToast(message), 0);
  };

  const swapDirection = () => {
    const next = direction === "deposit" ? "withdrawal" : "deposit";
    setDirection(next);
    setRecipient(ROUTES[next].recipient);
    setAmount("");
    setScreen("form");
    setProgressStep(0);
  };

  const completeTransfer = () => {
    setScreen("complete");
    setLatestActivity({
      id: `latest-${Date.now()}`,
      direction,
      amount: Number(amount).toFixed(3),
      title: direction === "deposit" ? "Deposit to Zeko" : "Withdrawal to Ethereum",
      status: "Completed",
      kind: "complete",
      detail: direction === "deposit" ? "Ethereum deposit #128" : "Settlement 47 · action 03",
      time: "Just now"
    });
  };

  const startNewTransfer = () => {
    setAmount("");
    setProgressStep(0);
    setScreen("form");
  };

  let bridgeContent;
  if (screen === "review") {
    bridgeContent = (
      <ReviewView
        direction={direction}
        amount={amount}
        recipient={recipient}
        timeout={timeout}
        onBack={() => setScreen("form")}
        onConfirm={() => {
          setProgressStep(0);
          setScreen("progress");
          notify(`${direction === "deposit" ? "Ethereum" : "Zeko"} wallet confirmation accepted in prototype.`);
        }}
      />
    );
  } else if (screen === "progress") {
    bridgeContent = (
      <ProgressView
        direction={direction}
        amount={amount}
        step={progressStep}
        onClaim={completeTransfer}
        notify={notify}
      />
    );
  } else if (screen === "complete") {
    bridgeContent = (
      <CompleteView
        direction={direction}
        amount={amount}
        recipient={recipient}
        onNewTransfer={startNewTransfer}
        onActivity={() => setTab("activity")}
      />
    );
  } else {
    bridgeContent = (
      <BridgeForm
        direction={direction}
        onSwap={swapDirection}
        amount={amount}
        setAmount={setAmount}
        recipient={recipient}
        setRecipient={setRecipient}
        onReview={() => setScreen("review")}
        showDetails={showDetails}
        setShowDetails={setShowDetails}
        timeout={timeout}
      />
    );
  }

  return (
    <div className="app-shell">
      <BackgroundWave className="contour-desktop" src="assets/zeko-contours.webm" storageKey="zeko-wave-desktop-time" />
      <BackgroundWave className="contour-mobile" src="assets/zeko-contours-mobile.webm" storageKey="zeko-wave-mobile-time" />
      <div className="background-wash"></div>

      <AppHeader notify={notify} />

      <main className="main-content">
        <div className="hero">
          <p className="eyebrow">Ethereum settlement</p>
          <h1>Ethereum ↔ Zeko Bridge</h1>
          <p className="hero-copy">Move native ETH between Ethereum custody and Zeko execution, with every transition proven through SP1 and anchored to settlement state.</p>
        </div>

        <div className="bridge-card">
          <div className="card-header">
            <div className="tabs" role="tablist" aria-label="Bridge navigation">
              <button type="button" className={`tab${tab === "bridge" ? " active" : ""}`} onClick={() => setTab("bridge")} role="tab" aria-selected={tab === "bridge"}>Bridge</button>
              <button type="button" className={`tab${tab === "activity" ? " active" : ""}`} onClick={() => setTab("activity")} role="tab" aria-selected={tab === "activity"}>Activity</button>
            </div>
            <button type="button" className="icon-button" onClick={() => setSettingsOpen(true)} aria-label="Open bridge settings" title="Bridge settings">
              <span className="settings-glyph">⚙︎</span>
            </button>
          </div>

          <div className="card-body">
            {tab === "activity" ? <ActivityView latest={latestActivity} /> : bridgeContent}
          </div>

          <div className="card-footnote">
            <span className="footnote-proof">SP1</span>
            <span>verifies the Zeko state transition</span>
            <span>·</span>
            <span>Ethereum verifies settlement</span>
          </div>
        </div>

        <footer className="page-footer">
          <span><span className="health-dot"></span> Gateway online</span>
          <span>Bridge contract 0x7A3…29F</span>
          <span>Proof of concept</span>
        </footer>
      </main>

      {settingsOpen && (
        <SettingsModal
          timeout={timeout}
          setTimeoutHours={setTimeoutHours}
          expert={showDetails}
          setExpert={setShowDetails}
          onClose={() => setSettingsOpen(false)}
        />
      )}

      {toast && <div className="toast" role="status"><span className="toast-dot"></span>{toast}</div>}
    </div>
  );
}

preact.render(<App />, document.getElementById("root"));
