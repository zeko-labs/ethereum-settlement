import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { ExplorerApi } from "../lib/api";
import type { RuntimeConfig } from "../lib/runtime";
import type { Route } from "../lib/router";
import type { SearchResponse } from "../lib/types";
import { Address, Icon } from "./ui";

interface LayoutProps {
  api: ExplorerApi;
  config: RuntimeConfig;
  route: Route;
  navigate: (route: Route) => void;
  children: ReactNode;
}

const navigation: Array<{
  label: string;
  route: Route;
  pages: Route["page"][];
}> = [
  { label: "Overview", route: { page: "overview" }, pages: ["overview"] },
  { label: "Blocks", route: { page: "blocks" }, pages: ["blocks", "block"] },
  {
    label: "Transactions",
    route: { page: "transactions" },
    pages: ["transactions", "transaction", "account"],
  },
  {
    label: "Settlements",
    route: { page: "settlements" },
    pages: ["settlements", "settlement"],
  },
  {
    label: "Bridge",
    route: { page: "bridge" },
    pages: ["bridge", "deposit", "withdrawal"],
  },
];

export function Layout({
  api,
  config,
  route,
  navigate,
  children,
}: LayoutProps) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResponse | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const keyboard = (event: KeyboardEvent) => {
      if (
        event.key === "/" &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey
      ) {
        const element = event.target as HTMLElement | null;
        if (element?.tagName !== "INPUT" && element?.tagName !== "TEXTAREA") {
          event.preventDefault();
          setSearchOpen(true);
        }
      }
      if (event.key === "Escape") setSearchOpen(false);
    };
    window.addEventListener("keydown", keyboard);
    return () => window.removeEventListener("keydown", keyboard);
  }, []);

  useEffect(() => {
    if (searchOpen) input.current?.focus();
  }, [searchOpen]);

  useEffect(() => {
    if (!searchOpen || query.trim().length < 2) {
      setResults(null);
      setSearchError(null);
      return;
    }
    const controller = new AbortController();
    const timer = window.setTimeout(async () => {
      setSearching(true);
      try {
        setResults(await api.search(query.trim(), controller.signal));
        setSearchError(null);
      } catch (caught) {
        if (!controller.signal.aborted)
          setSearchError(
            caught instanceof Error ? caught.message : "Search failed",
          );
      } finally {
        if (!controller.signal.aborted) setSearching(false);
      }
    }, 220);
    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [api, query, searchOpen]);

  const go = (next: Route) => {
    setSearchOpen(false);
    setQuery("");
    navigate(next);
  };

  return (
    <div className="explorer-shell">
      <video
        className="contours contours-desktop"
        autoPlay
        loop
        muted
        playsInline
        src="/assets/zeko-contours.webm"
      />
      <video
        className="contours contours-mobile"
        autoPlay
        loop
        muted
        playsInline
        src="/assets/zeko-contours-mobile.webm"
      />
      <header className="site-header">
        <button className="brand" onClick={() => go({ page: "overview" })}>
          <img src="/assets/zeko-logo.svg" alt="Zeko" />
          <span>Explorer</span>
        </button>
        <nav className="desktop-nav" aria-label="Primary">
          {navigation.map((item) => (
            <button
              className={item.pages.includes(route.page) ? "active" : ""}
              key={item.label}
              onClick={() => go(item.route)}
            >
              {item.label}
            </button>
          ))}
        </nav>
        <div className="header-tools">
          <button
            className="search-trigger"
            onClick={() => setSearchOpen(true)}
          >
            <Icon name="search" />
            <span>Search blocks, transactions, addresses</span>
            <kbd>/</kbd>
          </button>
          <span className="network-pill">
            <span className="live-dot" />
            {config.networkName}
          </span>
          <a className="bridge-cta" href={config.bridgeUiUrl}>
            Bridge <Icon name="arrow" />
          </a>
        </div>
      </header>
      <main className="page">{children}</main>
      <nav className="mobile-nav" aria-label="Mobile navigation">
        {navigation.map((item) => (
            <button
              className={item.pages.includes(route.page) ? "active" : ""}
              key={item.label}
              onClick={() => go(item.route)}
            >
              <span>{item.label.slice(0, 1)}</span>
              {item.label}
            </button>
          ))}
      </nav>
      {searchOpen ? (
        <div
          className="search-layer"
          onMouseDown={(event) =>
            event.target === event.currentTarget && setSearchOpen(false)
          }
        >
          <div
            className="search-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Global search"
          >
            <div className="search-input">
              <Icon name="search" />
              <input
                ref={input}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Block height, hash, B62 or 0x address"
              />
              <button onClick={() => setSearchOpen(false)}>Esc</button>
            </div>
            <SearchResults
              results={results}
              searching={searching}
              error={searchError}
              navigate={go}
            />
          </div>
        </div>
      ) : null}
    </div>
  );
}

function SearchResults({
  results,
  searching,
  error,
  navigate,
}: {
  results: SearchResponse | null;
  searching: boolean;
  error: string | null;
  navigate: (route: Route) => void;
}) {
  if (searching)
    return (
      <div className="search-results">
        <p>Searching every indexed source…</p>
      </div>
    );
  if (error)
    return (
      <div className="search-results">
        <p>{error}</p>
      </div>
    );
  if (!results)
    return (
      <div className="search-results">
        <p>
          Search across Zeko L2, Ethereum settlements and the native bridge.
        </p>
      </div>
    );
  const items: Array<{ label: string; value: string; route: Route }> = [
    ...results.groups.blocks.map((item) => ({
      label: "Block",
      value: `${item.height} · ${item.stateHash}`,
      route: { page: "block" as const, identifier: item.height },
    })),
    ...results.groups.transactions.map((item) => ({
      label: "Transaction",
      value: item.hash,
      route: { page: "transaction" as const, hash: item.hash },
    })),
    ...results.groups.accounts.map((item) => ({
      label: "Account",
      value: item.publicKey,
      route: { page: "account" as const, publicKey: item.publicKey },
    })),
    ...results.groups.settlements.map((item) => ({
      label: "Settlement",
      value: `#${item.sequence} · ${item.ethereumTransactionHash}`,
      route: { page: "settlement" as const, identifier: item.sequence },
    })),
    ...results.groups.deposits.map((item) => ({
      label: "Deposit",
      value: `#${item.nonce} · ${item.ethereumTransactionHash}`,
      route: { page: "deposit" as const, nonce: item.nonce },
    })),
    ...results.groups.withdrawals.map((item) => ({
      label: "Withdrawal",
      value: `${item.settlementSequence}:${item.offset} · ${item.recipient}`,
      route: {
        page: "withdrawal" as const,
        sequence: item.settlementSequence,
        offset: String(item.offset),
      },
    })),
  ];
  if (items.length === 0)
    return (
      <div className="search-results">
        <p>No exact indexed record found.</p>
      </div>
    );
  return (
    <div className="search-results">
      {items.map((item) => (
        <button
          key={`${item.label}-${item.value}`}
          onClick={() => navigate(item.route)}
        >
          <span>{item.label}</span>
          <Address>{item.value}</Address>
          <Icon name="chevron" />
        </button>
      ))}
    </div>
  );
}
