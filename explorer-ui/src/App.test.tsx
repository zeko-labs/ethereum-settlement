import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ExplorerApplication } from "./App";
import { responseFor, runtimeConfig } from "./test/fixtures";

afterEach(() => {
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("Zeko explorer", () => {
  it("renders execution, settlement, and both bridge directions from public API data", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async (request: RequestInfo | URL) =>
          new Response(JSON.stringify(responseFor(String(request))), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
      ),
    );
    render(<ExplorerApplication config={runtimeConfig} />);
    expect(await screen.findByText("18,492")).toBeInTheDocument();
    expect(screen.getByText("Next commit")).toBeInTheDocument();
    expect(screen.getByText("Every 15m")).toBeInTheDocument();
    expect(screen.getByText("Settlement #284")).toBeInTheDocument();
    expect(screen.getByText("Deposit #147")).toBeInTheDocument();
    expect(screen.getByText("Withdrawal 284:3")).toBeInTheDocument();
  });

  it("opens a transaction detail route through visible navigation", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async (request: RequestInfo | URL) =>
          new Response(JSON.stringify(responseFor(String(request))), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
      ),
    );
    const user = userEvent.setup();
    render(<ExplorerApplication config={runtimeConfig} />);
    await user.click(await screen.findByText(/5JtY7ZkLwcDa/));
    await waitFor(() =>
      expect(window.location.pathname).toContain("/transactions/"),
    );
    expect(await screen.findByText("Transaction hash")).toBeInTheDocument();
    expect(screen.getByText("18446744073709551615")).toBeInTheDocument();
    expect(screen.getByText("Native withdrawal request")).toBeInTheDocument();
    expect(screen.getByText("5 ZEKO")).toBeInTheDocument();
    expect(screen.getByText("Pending Settlement")).toBeInTheDocument();
  });
});
