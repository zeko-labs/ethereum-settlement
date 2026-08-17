import {
  Banner,
  Bold,
  Box,
  Button,
  Copyable,
  Divider,
  Heading,
  Row,
  Section,
  Spinner,
  Text
} from "@metamask/snaps-sdk/jsx"

export const MINA_TOKEN_ID =
  "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf"

export type MinaBalance = {
  total: string
  liquid: string
  locked: string
}

export type NativeAccount = MinaBalance & {
  nonce: string
}

export type TokenAccount = MinaBalance & {
  tokenId: string
  symbol: string
}

export type WalletAccounts = {
  native: NativeAccount | null
  tokens: TokenAccount[]
}

export type WalletHomeSnapshot = {
  publicKey: string
  networkId: string
  networkName: string
  endpoint?: string
  native?: NativeAccount | null
  tokens: TokenAccount[]
  error?: string
}

const readUnsignedInteger = (value: unknown, label: string): string => {
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
    return String(value)
  }
  if (typeof value !== "string" || !/^\d+$/u.test(value)) {
    throw new Error(`Mina node returned an invalid ${label}`)
  }
  return value.replace(/^0+(?=\d)/u, "")
}

const readBalance = (value: unknown): MinaBalance => {
  if (typeof value !== "object" || value === null) {
    throw new Error("Mina node returned an invalid balance")
  }
  const balance = value as Record<string, unknown>
  const total = readUnsignedInteger(balance.total, "total balance")
  return {
    total,
    liquid: balance.liquid === undefined || balance.liquid === null
      ? total
      : readUnsignedInteger(balance.liquid, "liquid balance"),
    locked: balance.locked === undefined || balance.locked === null
      ? "0"
      : readUnsignedInteger(balance.locked, "locked balance")
  }
}

export const parseWalletAccounts = (value: unknown): WalletAccounts => {
  if (!Array.isArray(value)) {
    throw new Error("Mina node returned an invalid accounts list")
  }

  let native: NativeAccount | null = null
  const tokens: TokenAccount[] = []
  for (const valueAccount of value) {
    if (typeof valueAccount !== "object" || valueAccount === null) {
      throw new Error("Mina node returned an invalid account")
    }
    const account = valueAccount as Record<string, unknown>
    if (typeof account.tokenId !== "string" || !account.tokenId ||
        account.tokenId.length > 128) {
      throw new Error("Mina node returned an invalid token ID")
    }
    const balance = readBalance(account.balance)
    if (account.tokenId === MINA_TOKEN_ID) {
      native = {
        ...balance,
        nonce: readUnsignedInteger(account.nonce, "account nonce")
      }
      continue
    }
    const symbol = typeof account.tokenSymbol === "string"
      ? account.tokenSymbol.trim().slice(0, 12)
      : ""
    tokens.push({
      ...balance,
      tokenId: account.tokenId,
      symbol: symbol || "Token"
    })
  }
  return {
    native,
    tokens: tokens.sort((left, right) =>
      left.symbol.localeCompare(right.symbol) || left.tokenId.localeCompare(right.tokenId))
  }
}

export const formatNanomina = (value: string): string => {
  const nanomina = BigInt(readUnsignedInteger(value, "MINA balance"))
  const whole = nanomina / 1_000_000_000n
  const fraction = (nanomina % 1_000_000_000n)
    .toString()
    .padStart(9, "0")
    .replace(/0+$/u, "")
  return fraction ? `${whole}.${fraction}` : whole.toString()
}

const endpointLabel = (endpoint: string | undefined): string | undefined => {
  if (!endpoint) return undefined
  try {
    const parsed = new URL(endpoint)
    return parsed.port ? `${parsed.hostname}:${parsed.port}` : parsed.hostname
  } catch {
    return "configured endpoint"
  }
}

export const missingEndpointMessage = (networkId: string): string =>
  `No GraphQL endpoint is configured for ${networkId}. Connect a Mina dapp and add or switch to a configured network.`

export const renderWalletHome = (snapshot: WalletHomeSnapshot) => {
  const nativeBalance = snapshot.native === undefined
    ? "Unavailable"
    : `${formatNanomina(snapshot.native?.total ?? "0")} MINA`
  const availableBalance = snapshot.native === undefined
    ? "Unavailable"
    : `${formatNanomina(snapshot.native?.liquid ?? "0")} MINA`
  const lockedBalance = snapshot.native?.locked ?? "0"
  const visibleTokens = snapshot.tokens.slice(0, 20)
  const endpoint = endpointLabel(snapshot.endpoint)

  return (
    <Box>
      <Heading>Zeko Mina Wallet</Heading>
      <Section>
        <Row label="Network">
          <Text>{snapshot.networkName}</Text>
        </Row>
        <Row label="Address">
          <Copyable value={snapshot.publicKey} />
        </Row>
        {endpoint ? (
          <Row label="Data source">
            <Text>{endpoint}</Text>
          </Row>
        ) : null}
      </Section>

      {snapshot.error ? (
        <Banner title="Balance unavailable" severity="warning">
          <Text>{snapshot.error}</Text>
        </Banner>
      ) : null}

      <Heading>Balance</Heading>
      <Section>
        <Row label="MINA">
          <Text><Bold>{nativeBalance}</Bold></Text>
        </Row>
        <Row label="Available">
          <Text>{availableBalance}</Text>
        </Row>
        {lockedBalance !== "0" ? (
          <Row label="Locked">
            <Text>{formatNanomina(lockedBalance)} MINA</Text>
          </Row>
        ) : null}
        <Row label="Nonce">
          <Text>{snapshot.native === undefined ? "Unavailable" : snapshot.native?.nonce ?? "0"}</Text>
        </Row>
      </Section>

      <Heading>Tokens</Heading>
      {snapshot.native === undefined ? (
        <Text>Token balances are unavailable until the network endpoint responds.</Text>
      ) : visibleTokens.length === 0 ? (
        <Text>No custom token accounts found.</Text>
      ) : (
        <Box>
          {visibleTokens.map((token, index) => (
            <Section key={`${token.tokenId}-${index}`}>
              <Row label={token.symbol}>
                <Text><Bold>{token.total} base units</Bold></Text>
              </Row>
              <Row label="Available">
                <Text>{token.liquid} base units</Text>
              </Row>
              {token.locked !== "0" ? (
                <Row label="Locked">
                  <Text>{token.locked} base units</Text>
                </Row>
              ) : null}
              <Row label="Token ID">
                <Copyable value={token.tokenId} />
              </Row>
            </Section>
          ))}
          {snapshot.tokens.length > visibleTokens.length ? (
            <Text>{String(snapshot.tokens.length - visibleTokens.length)} more token accounts are not shown.</Text>
          ) : null}
          <Text>Custom-token amounts use exact base units because token decimals are not part of Mina account data.</Text>
        </Box>
      )}

      <Divider />
      <Button name="refresh-balances">Refresh balances</Button>
    </Box>
  )
}

export const renderWalletLoading = (snapshot: Pick<
WalletHomeSnapshot,
"publicKey" | "networkName"
>) => (
  <Box>
    <Heading>Zeko Mina Wallet</Heading>
    <Section>
      <Row label="Network">
        <Text>{snapshot.networkName}</Text>
      </Row>
      <Row label="Address">
        <Copyable value={snapshot.publicKey} />
      </Row>
    </Section>
    <Spinner />
    <Text>Refreshing balances and token accounts…</Text>
    <Button name="refresh-balances" loading>Refresh balances</Button>
  </Box>
)
