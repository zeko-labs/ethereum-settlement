import { useEffect, useMemo, useState } from "react";
import { ExplorerApi } from "./lib/api";
import { href, parseRoute, type Route } from "./lib/router";
import { loadRuntimeConfig, type RuntimeConfig } from "./lib/runtime";
import { Layout } from "./components/Layout";
import { Overview } from "./pages/Overview";
import {
  BlocksPage,
  BridgePage,
  SettlementsPage,
  TransactionsPage,
} from "./pages/Lists";
import {
  AccountDetailPage,
  BlockDetailPage,
  DepositDetailPage,
  SettlementDetailPage,
  TransactionDetailPage,
  WithdrawalDetailPage,
} from "./pages/Details";

export default function App() {
  const [config, setConfig] = useState<RuntimeConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    loadRuntimeConfig(controller.signal)
      .then(setConfig)
      .catch((caught) => {
        if (!controller.signal.aborted)
          setError(
            caught instanceof Error
              ? caught.message
              : "Unable to load runtime configuration",
          );
      });
    return () => controller.abort();
  }, [generation]);

  if (error)
    return (
      <div className="boot-state" role="alert">
        <img src="/assets/zeko-logo.svg" alt="Zeko" />
        <strong>Explorer configuration error</strong>
        <p>{error}</p>
        <button
          onClick={() => {
            setError(null);
            setGeneration((value) => value + 1);
          }}
        >
          Retry
        </button>
      </div>
    );
  if (!config)
    return (
      <div className="boot-state">
        <img src="/assets/zeko-logo.svg" alt="Zeko" />
        <span className="boot-spinner" />
        <p>Connecting to Zeko indexers…</p>
      </div>
    );
  return <ExplorerApplication config={config} />;
}

export function ExplorerApplication({ config }: { config: RuntimeConfig }) {
  const [route, setRoute] = useState<Route>(() =>
    parseRoute(window.location.pathname),
  );
  const api = useMemo(() => new ExplorerApi(config), [config]);

  useEffect(() => {
    const pop = () => setRoute(parseRoute(window.location.pathname));
    window.addEventListener("popstate", pop);
    return () => window.removeEventListener("popstate", pop);
  }, []);

  const navigate = (next: Route) => {
    const path = href(next);
    if (window.location.pathname !== path)
      window.history.pushState({}, "", path);
    setRoute(next);
    window.scrollTo({ top: 0, behavior: "smooth" });
  };

  return (
    <Layout api={api} config={config} route={route} navigate={navigate}>
      {renderRoute(route, api, config, navigate)}
    </Layout>
  );
}

function renderRoute(
  route: Route,
  api: ExplorerApi,
  config: RuntimeConfig,
  navigate: (route: Route) => void,
) {
  const props = { api, config, navigate };
  switch (route.page) {
    case "overview":
      return <Overview {...props} />;
    case "blocks":
      return <BlocksPage {...props} />;
    case "block":
      return <BlockDetailPage {...props} identifier={route.identifier} />;
    case "transactions":
      return <TransactionsPage {...props} />;
    case "transaction":
      return <TransactionDetailPage {...props} hash={route.hash} />;
    case "account":
      return <AccountDetailPage {...props} publicKey={route.publicKey} />;
    case "settlements":
      return <SettlementsPage {...props} />;
    case "settlement":
      return <SettlementDetailPage {...props} identifier={route.identifier} />;
    case "bridge":
      return <BridgePage {...props} />;
    case "deposit":
      return <DepositDetailPage {...props} nonce={route.nonce} />;
    case "withdrawal":
      return (
        <WithdrawalDetailPage
          {...props}
          sequence={route.sequence}
          offset={route.offset}
        />
      );
    case "notFound":
      return (
        <section className="not-found">
          <span>404</span>
          <h1>That explorer record is out of range.</h1>
          <p>Check the identifier or return to the live network overview.</p>
          <button onClick={() => navigate({ page: "overview" })}>
            Return to overview
          </button>
        </section>
      );
  }
}
