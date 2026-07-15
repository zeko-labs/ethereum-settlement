import { describe, expect, it } from "vitest";
import { href, parseRoute } from "./router";

describe("history routes", () => {
  it("round trips detail routes without a routing dependency", () => {
    const routes = [
      { page: "block" as const, identifier: "18492" },
      { page: "transaction" as const, hash: "5Jt abc" },
      { page: "account" as const, publicKey: "B62q/key" },
      { page: "settlement" as const, identifier: "284" },
      { page: "deposit" as const, nonce: "147" },
      { page: "withdrawal" as const, sequence: "284", offset: "3" },
    ];
    for (const route of routes) expect(parseRoute(href(route))).toEqual(route);
  });
});
