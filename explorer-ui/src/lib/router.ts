export type Route =
  | { page: "overview" }
  | { page: "blocks" }
  | { page: "block"; identifier: string }
  | { page: "transactions" }
  | { page: "transaction"; hash: string }
  | { page: "account"; publicKey: string }
  | { page: "settlements" }
  | { page: "settlement"; identifier: string }
  | { page: "bridge" }
  | { page: "deposit"; nonce: string }
  | { page: "withdrawal"; sequence: string; offset: string }
  | { page: "notFound" };

export function parseRoute(pathname: string): Route {
  const parts = pathname.split("/").filter(Boolean).map(decodeURIComponent);
  if (parts.length === 0) return { page: "overview" };
  if (parts[0] === "blocks" && parts.length === 1) return { page: "blocks" };
  if (parts[0] === "blocks" && parts[1] && parts.length === 2)
    return { page: "block", identifier: parts[1] };
  if (parts[0] === "transactions" && parts.length === 1)
    return { page: "transactions" };
  if (parts[0] === "transactions" && parts[1] && parts.length === 2)
    return { page: "transaction", hash: parts[1] };
  if (parts[0] === "accounts" && parts[1] && parts.length === 2)
    return { page: "account", publicKey: parts[1] };
  if (parts[0] === "settlements" && parts.length === 1)
    return { page: "settlements" };
  if (parts[0] === "settlements" && parts[1] && parts.length === 2)
    return { page: "settlement", identifier: parts[1] };
  if (parts[0] === "bridge" && parts.length === 1) return { page: "bridge" };
  if (
    parts[0] === "bridge" &&
    parts[1] === "deposits" &&
    parts[2] &&
    parts.length === 3
  )
    return { page: "deposit", nonce: parts[2] };
  if (
    parts[0] === "bridge" &&
    parts[1] === "withdrawals" &&
    parts[2] &&
    parts[3] &&
    parts.length === 4
  )
    return { page: "withdrawal", sequence: parts[2], offset: parts[3] };
  return { page: "notFound" };
}

export function href(route: Route): string {
  switch (route.page) {
    case "overview":
      return "/";
    case "blocks":
      return "/blocks";
    case "block":
      return `/blocks/${encodeURIComponent(route.identifier)}`;
    case "transactions":
      return "/transactions";
    case "transaction":
      return `/transactions/${encodeURIComponent(route.hash)}`;
    case "account":
      return `/accounts/${encodeURIComponent(route.publicKey)}`;
    case "settlements":
      return "/settlements";
    case "settlement":
      return `/settlements/${encodeURIComponent(route.identifier)}`;
    case "bridge":
      return "/bridge";
    case "deposit":
      return `/bridge/deposits/${encodeURIComponent(route.nonce)}`;
    case "withdrawal":
      return `/bridge/withdrawals/${encodeURIComponent(route.sequence)}/${encodeURIComponent(route.offset)}`;
    case "notFound":
      return "/404";
  }
}
