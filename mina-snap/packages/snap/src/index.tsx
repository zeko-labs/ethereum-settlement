import {
  BIP44CoinTypeNode,
  deriveBIP44AddressKey
} from "@metamask/key-tree"
import {
  InvalidParamsError,
  MethodNotSupportedError,
  UnauthorizedError,
  UserInputEventType,
  UserRejectedRequestError,
  type Json,
  type OnHomePageHandler,
  type OnRpcRequestHandler,
  type OnUserInputHandler
} from "@metamask/snaps-sdk"
import {
  Box,
  Bold,
  Copyable,
  Divider,
  Heading,
  Row,
  Section,
  Text
} from "@metamask/snaps-sdk/jsx"
import { sha256 } from "@noble/hashes/sha256"
import { base58check } from "@scure/base"
import Client from "mina-signer"
import {
  missingEndpointMessage,
  parseWalletAccounts,
  renderWalletHome,
  renderWalletLoading,
  type WalletHomeSnapshot
} from "./home"

const MINA_COIN_TYPE = 12_586
const MINA_ENTROPY_PATH = ["m", "44'", "12586'"] as const
const MAX_UINT32 = 4_294_967_295n
const MAX_UINT64 = 18_446_744_073_709_551_615n
const MAX_ZKAPP_BYTES = 100_000
const MAX_ZKAPP_UPDATES = 32
const BUILT_IN_NETWORKS = new Set([
  "mina:mainnet",
  "mina:devnet",
  "zeko:mainnet",
  "zeko:testnet",
  "testnet"
])

const BUILT_IN_NETWORK_NAMES: Record<string, string> = {
  "mina:mainnet": "Mina Mainnet",
  "mina:devnet": "Mina Devnet",
  "zeko:mainnet": "Zeko Mainnet",
  "zeko:testnet": "Zeko Testnet",
  testnet: "Testnet"
}

type MinaSnapState = {
  version: 2
  origins: Record<string, { account: number }>
  selectedNetwork: string
  chains: Record<string, { url: string; name: string }>
  credentials: Record<string, string[]>
}

const initialState = (): MinaSnapState => ({
  version: 2,
  origins: {},
  selectedNetwork: "mina:mainnet",
  chains: {},
  credentials: {}
})

const getState = async (): Promise<MinaSnapState> => {
  const stored = await snap.request({
    method: "snap_manageState",
    params: { operation: "get" }
  }) as (Partial<Omit<MinaSnapState, "version">> & {
    version?: 1 | 2
  }) | null
  if (
    (stored?.version !== 1 && stored?.version !== 2) ||
    typeof stored.origins !== "object" ||
    stored.origins === null ||
    typeof stored.selectedNetwork !== "string"
  ) {
    return initialState()
  }
  const chains = typeof stored.chains === "object" && stored.chains !== null
    ? { ...stored.chains }
    : {}
  if (
    stored.version === 1 &&
    chains["zeko:testnet"]?.url === "https://testnet.zeko.io/graphql"
  ) {
    delete chains["zeko:testnet"]
  }
  return {
    ...(stored as MinaSnapState),
    version: 2,
    chains,
    credentials: typeof stored.credentials === "object" && stored.credentials !== null
      ? stored.credentials
      : {}
  }
}

const setState = async (state: MinaSnapState): Promise<void> => {
  await snap.request({
    method: "snap_manageState",
    params: { operation: "update", newState: state }
  })
}

const deriveAccount = async (account = 0): Promise<{
  privateKey: string
  publicKey: string
}> => {
  const entropy = await snap.request({
    method: "snap_getBip32Entropy",
    params: {
      path: [...MINA_ENTROPY_PATH],
      curve: "secp256k1"
    }
  })
  if (entropy.depth !== 2) {
    throw new Error("MetaMask returned Mina entropy at an unexpected depth")
  }
  const coinTypeNode = await BIP44CoinTypeNode.fromJSON({
    depth: 2,
    masterFingerprint: entropy.masterFingerprint,
    parentFingerprint: entropy.parentFingerprint,
    index: entropy.index,
    network: entropy.network,
    privateKey: entropy.privateKey,
    publicKey: entropy.publicKey,
    chainCode: entropy.chainCode
  }, MINA_COIN_TYPE)
  const addressNode = await deriveBIP44AddressKey(coinTypeNode, {
    account,
    change: 0,
    address_index: 0
  })
  if (!addressNode.privateKeyBytes) {
    throw new Error("MetaMask did not provide private Mina entropy")
  }

  // Auro converts the BIP-44 secp256k1 scalar into Mina's little-endian
  // private-key payload before Base58Check encoding it.
  const scalar = Uint8Array.from(addressNode.privateKeyBytes)
  const payload = new Uint8Array(34)
  try {
    scalar[0] = (scalar[0] ?? 0) & 0x3f
    scalar.reverse()
    payload.set([0x5a, 0x01])
    payload.set(scalar, 2)
    const privateKey = base58check(sha256).encode(payload)
    const publicKey = new Client({ network: "mainnet" }).derivePublicKey(privateKey)
    return { privateKey, publicKey }
  } finally {
    scalar.fill(0)
    payload.fill(0)
  }
}

const derivePublicKey = async (account = 0): Promise<string> =>
  (await deriveAccount(account)).publicKey

const readMessage = (params: unknown): string => {
  if (
    typeof params !== "object" ||
    params === null ||
    !("message" in params) ||
    typeof params.message !== "string"
  ) {
    throw new InvalidParamsError("mina_signMessage requires a string message")
  }
  return params.message
}

const readSignedMessage = (params: unknown): {
  data: string
  publicKey: string
  signature: { field: string; scalar: string }
} => {
  if (typeof params !== "object" || params === null) {
    throw new InvalidParamsError("Expected a signed Mina message")
  }
  const { data, publicKey, signature } = params as Record<string, unknown>
  if (typeof data !== "string" || typeof publicKey !== "string" ||
      typeof signature !== "object" || signature === null) {
    throw new InvalidParamsError("The signed Mina message is invalid")
  }
  const field = (signature as Record<string, unknown>).field
  const scalar = (signature as Record<string, unknown>).scalar
  if (typeof field !== "string" || typeof scalar !== "string") {
    throw new InvalidParamsError("The Mina signature is invalid")
  }
  return { data, publicKey, signature: { field, scalar } }
}

const readFields = (params: unknown, property: "message" | "data"): Array<string | number> => {
  if (typeof params !== "object" || params === null || !(property in params)) {
    throw new InvalidParamsError(`Expected ${property} fields`)
  }
  const value = (params as Record<string, unknown>)[property]
  if (
    !Array.isArray(value) ||
    !value.every((field) =>
      (typeof field === "string" && /^-?[0-9]+$/u.test(field)) ||
      (typeof field === "number" && Number.isSafeInteger(field)))
  ) {
    throw new InvalidParamsError(`${property} must contain decimal field elements`)
  }
  return value as Array<string | number>
}

const signerNetwork = (networkID: string): "mainnet" | "testnet" | { custom: string } =>
  networkID === "mina:mainnet"
    ? "mainnet"
    : networkID === "zeko:mainnet"
      ? { custom: "zeko-mainnet" }
      : "testnet"

const signingDomain = (networkID: string): string => {
  const network = signerNetwork(networkID)
  return typeof network === "string" ? network : network.custom
}

const getClient = (networkID: string, era?: "berkeley"): Client => new Client({
  network: signerNetwork(networkID),
  ...(era ? { era } : {})
})

const validatePublicKey = (client: Client, publicKey: string): void => {
  try {
    client.publicKeyToRaw(publicKey)
  } catch {
    throw new InvalidParamsError("The recipient is not a valid Mina public key")
  }
}

const getZkappEra = (command: Record<string, unknown>): "berkeley" | undefined => {
  const updates = Array.isArray(command.accountUpdates) ? command.accountUpdates : []
  const lengths: number[] = []
  for (const update of updates) {
    if (typeof update !== "object" || update === null) continue
    const body = (update as { body?: unknown }).body
    if (typeof body !== "object" || body === null) continue
    const bodyRecord = body as Record<string, unknown>
    const updateValue = bodyRecord.update
    if (typeof updateValue === "object" && updateValue !== null) {
      const appState = (updateValue as { appState?: unknown }).appState
      if (Array.isArray(appState)) lengths.push(appState.length)
    }
    const preconditions = bodyRecord.preconditions
    const account = typeof preconditions === "object" && preconditions !== null
      ? (preconditions as { account?: unknown }).account
      : undefined
    const state = typeof account === "object" && account !== null
      ? (account as { state?: unknown }).state
      : undefined
    if (Array.isArray(state)) lengths.push(state.length)
  }
  if (lengths.some((length) => length !== 8 && length !== 32) ||
      (lengths.includes(8) && lengths.includes(32))) {
    throw new InvalidParamsError("The zkApp command mixes unsupported Mina protocol eras")
  }
  return lengths.includes(8) ? "berkeley" : undefined
}

const signedAccountUpdateIndexes = (
  command: Record<string, unknown>,
  publicKey: string
): number[] => {
  if (!Array.isArray(command.accountUpdates)) return []
  const indexes: number[] = []
  command.accountUpdates.forEach((update, index) => {
    if (typeof update !== "object" || update === null) return
    const body = (update as { body?: unknown }).body
    if (typeof body !== "object" || body === null) return
    const bodyRecord = body as Record<string, unknown>
    const authorizationKind = bodyRecord.authorizationKind
    if (
      bodyRecord.publicKey === publicKey &&
      typeof authorizationKind === "object" &&
      authorizationKind !== null &&
      (authorizationKind as { isSigned?: unknown }).isSigned === true
    ) {
      indexes.push(index)
    }
  })
  return indexes
}

const readZkappCommand = (params: unknown): {
  command: Record<string, unknown>
  onlySign: boolean
  feePayerMemo?: string
  feePayerFee: unknown
  nonce: unknown
} => {
  if (typeof params !== "object" || params === null || !("transaction" in params)) {
    throw new InvalidParamsError("mina_sendTransaction requires a transaction")
  }
  const record = params as Record<string, unknown>
  if (typeof record.transaction === "string" &&
      record.transaction.length > MAX_ZKAPP_BYTES) {
    throw new InvalidParamsError("The zkApp transaction exceeds 100 KB")
  }
  let parsed: unknown
  try {
    parsed = typeof record.transaction === "string"
      ? JSON.parse(record.transaction) as unknown
      : record.transaction
  } catch {
    throw new InvalidParamsError("The zkApp transaction must be valid JSON")
  }
  if (typeof parsed !== "object" || parsed === null) {
    throw new InvalidParamsError("The zkApp transaction must be a JSON object")
  }
  let serialized: string | undefined
  try {
    serialized = JSON.stringify(parsed)
  } catch {
    throw new InvalidParamsError("The zkApp transaction must contain JSON values")
  }
  if (!serialized || serialized.length > MAX_ZKAPP_BYTES) {
    throw new InvalidParamsError("The zkApp transaction exceeds 100 KB")
  }
  const feePayer = typeof record.feePayer === "object" && record.feePayer !== null
    ? record.feePayer as Record<string, unknown>
    : {}
  return {
    command: parsed as Record<string, unknown>,
    onlySign: record.onlySign === true,
    ...(typeof feePayer.memo === "string" ? { feePayerMemo: feePayer.memo } : {}),
    feePayerFee: feePayer.fee,
    nonce: record.nonce
  }
}

const readUnsigned = (
  value: unknown,
  label: string,
  maximum: bigint,
  typeName: "UInt32" | "UInt64"
): string => {
  const input = typeof value === "number" && Number.isSafeInteger(value)
    ? value.toString()
    : typeof value === "string"
      ? value
      : ""
  if (!/^\d+$/u.test(input)) {
    throw new InvalidParamsError(`${label} must be a non-negative integer`)
  }
  const normalized = input.replace(/^0+(?=\d)/u, "")
  if (normalized.length > maximum.toString().length || BigInt(normalized) > maximum) {
    throw new InvalidParamsError(`${label} exceeds Mina ${typeName}`)
  }
  return normalized
}

const readCommandMemo = (value: unknown): string => {
  if (typeof value !== "string") {
    throw new InvalidParamsError("The zkApp command memo is invalid")
  }
  try {
    const payload = base58check(sha256).decode(value)
    const length = payload[2]
    if (payload.length !== 35 || payload[0] !== 20 || payload[1] !== 1 ||
        length === undefined || length > 32 ||
        payload.slice(3 + length).some((byte) => byte !== 0)) {
      throw new Error("invalid memo payload")
    }
    const bytes = payload.slice(3, 3 + length)
    const memo = new TextDecoder("utf-8", { fatal: true }).decode(bytes)
    const encoded = new TextEncoder().encode(memo)
    if (encoded.length !== bytes.length || encoded.some((byte, index) => byte !== bytes[index])) {
      throw new Error("invalid memo encoding")
    }
    return memo
  } catch {
    throw new InvalidParamsError("The zkApp command memo is invalid")
  }
}

const canonicalJson = (value: unknown): string => {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return JSON.stringify(value) as string
  }
  if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value) as string
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`
  if (typeof value === "object") {
    const entries = Object.entries(value).sort(([left], [right]) =>
      left < right ? -1 : left > right ? 1 : 0)
    return `{${entries.map(([key, item]) =>
      `${JSON.stringify(key)}:${canonicalJson(item)}`).join(",")}}`
  }
  throw new InvalidParamsError("The zkApp transaction must contain JSON values")
}

const sha256Hex = (value: string): string =>
  Array.from(sha256(new TextEncoder().encode(value)), (byte) =>
    byte.toString(16).padStart(2, "0")).join("")

const reviewValue = (value: unknown): string => {
  const canonical = canonicalJson(value)
  return canonical.length <= 180
    ? canonical
    : `SHA-256 ${sha256Hex(canonical)} (${canonical.length} characters)`
}

const reviewString = (value: string): string => value.length <= 180
  ? value
  : `SHA-256 ${sha256Hex(value)} (${value.length} characters)`

type ZkappUpdateReview = {
  index: number
  publicKey: string
  tokenId: string
  balanceChange: string
  authorization: string
  signingScope: string
  actions: string
  events: string
  callData: string
}

const readZkappUpdateReviews = (
  command: Record<string, unknown>,
  signedIndexes: number[]
): ZkappUpdateReview[] => {
  const updates = Array.isArray(command.accountUpdates) ? command.accountUpdates : []
  if (updates.length > MAX_ZKAPP_UPDATES) {
    throw new InvalidParamsError(`The Snap supports at most ${MAX_ZKAPP_UPDATES} account updates`)
  }
  return updates.map((update, index) => {
    if (typeof update !== "object" || update === null) {
      throw new InvalidParamsError(`Account update ${index + 1} is invalid`)
    }
    const body = (update as { body?: unknown }).body
    if (typeof body !== "object" || body === null) {
      throw new InvalidParamsError(`Account update ${index + 1} has no body`)
    }
    const record = body as Record<string, unknown>
    const publicKey = record.publicKey
    const tokenId = record.tokenId
    const balance = record.balanceChange
    const authorizationKind = record.authorizationKind
    if (typeof publicKey !== "string" || typeof tokenId !== "string" ||
        typeof balance !== "object" || balance === null ||
        typeof authorizationKind !== "object" || authorizationKind === null ||
        !Array.isArray(record.actions) || !Array.isArray(record.events) ||
        typeof record.callData !== "string") {
      throw new InvalidParamsError(`Account update ${index + 1} cannot be reviewed safely`)
    }
    const magnitude = (balance as Record<string, unknown>).magnitude
    const sign = (balance as Record<string, unknown>).sgn
    if ((typeof magnitude !== "string" && typeof magnitude !== "number") ||
        typeof sign !== "string") {
      throw new InvalidParamsError(`Account update ${index + 1} has an invalid balance change`)
    }
    const kind = authorizationKind as Record<string, unknown>
    const isSigned = kind.isSigned === true
    const isProved = kind.isProved === true
    if (isSigned && isProved) {
      throw new InvalidParamsError(`Account update ${index + 1} has conflicting authorization`)
    }
    return {
      index,
      publicKey,
      tokenId,
      balanceChange: `${sign} ${String(magnitude)}`,
      authorization: isSigned ? "Signature" : isProved ? "Proof" : "None",
      signingScope: signedIndexes.includes(index)
        ? "This Snap will sign this update"
        : "This Snap will not sign this update",
      actions: reviewValue(record.actions),
      events: reviewValue(record.events),
      callData: reviewString(record.callData)
    }
  })
}

const approveZkappSigning = async ({
  origin,
  onlySign,
  networkId,
  signingPublicKey,
  feePayerPublicKey,
  fee,
  nonce,
  validUntil,
  memo,
  updates,
  payloadHash
}: {
  origin: string
  onlySign: boolean
  networkId: string
  signingPublicKey: string
  feePayerPublicKey: string
  fee: string
  nonce: string
  validUntil: string | null
  memo: string
  updates: ZkappUpdateReview[]
  payloadHash: string
}): Promise<void> => {
  const networkName = BUILT_IN_NETWORK_NAMES[networkId] ?? networkId
  const approved = await snap.request({
    method: "snap_dialog",
    params: {
      type: "confirmation",
      content: (
        <Box>
          <Heading>Sign Mina zkApp transaction</Heading>
          <Text>Requesting site: <Bold>{origin}</Bold></Text>
          <Section>
            <Row label="Operation"><Text>{onlySign ? "Sign only" : "Sign and submit"}</Text></Row>
            <Row label="Network"><Text>{`${networkName} (${networkId})`}</Text></Row>
            <Row label="Signature domain"><Text>{signingDomain(networkId)}</Text></Row>
            <Row label="Signing account"><Copyable value={signingPublicKey} /></Row>
            <Row label="Fee payer"><Copyable value={feePayerPublicKey} /></Row>
            <Row label="Fee"><Text>{`${fee} nanomina`}</Text></Row>
            <Row label="Nonce"><Text>{nonce}</Text></Row>
            <Row label="Valid until"><Text>{validUntil ?? "No limit"}</Text></Row>
            <Row label="Memo"><Copyable value={memo || "(empty)"} /></Row>
            <Row label="Account updates"><Text>{String(updates.length)}</Text></Row>
          </Section>
          {updates.map((update) => (
            <Section key={String(update.index)}>
              <Heading>{`Account update ${update.index + 1}`}</Heading>
              <Row label="Target public key"><Copyable value={update.publicKey} /></Row>
              <Row label="Token ID"><Copyable value={update.tokenId} /></Row>
              <Row label="Balance change"><Text>{update.balanceChange}</Text></Row>
              <Row label="Authorization"><Text>{update.authorization}</Text></Row>
              <Row label="Signing scope"><Text>{update.signingScope}</Text></Row>
              <Row label="Actions"><Copyable value={update.actions} /></Row>
              <Row label="Events"><Copyable value={update.events} /></Row>
              <Row label="Call data"><Copyable value={update.callData} /></Row>
            </Section>
          ))}
          <Divider />
          <Text>Canonical payload SHA-256</Text>
          <Copyable value={payloadHash} />
        </Box>
      )
    }
  })
  if (!approved) {
    throw new UserRejectedRequestError("Sign Mina zkApp transaction was rejected")
  }
}

const requireConnection = async (origin: string): Promise<MinaSnapState> => {
  const state = await getState()
  if (!state.origins[origin]) {
    throw new UnauthorizedError("Connect the Mina account before signing")
  }
  return state
}

const approveSigning = async (
  origin: string,
  title: string,
  detail: string
): Promise<void> => {
  const approved = await snap.request({
    method: "snap_dialog",
    params: {
      type: "confirmation",
      content: (
        <Box>
          <Heading>{title}</Heading>
          <Text>Requesting site: <Bold>{origin}</Bold></Text>
          <Text>{detail}</Text>
        </Box>
      )
    }
  })
  if (!approved) throw new UserRejectedRequestError(`${title} was rejected`)
}

const approveTransactionSigning = async ({
  origin,
  title,
  operation,
  networkId,
  signingPublicKey,
  recipientLabel,
  recipient,
  amount,
  fee,
  nonce,
  memo
}: {
  origin: string
  title: string
  operation: string
  networkId: string
  signingPublicKey: string
  recipientLabel: "Recipient" | "Delegate"
  recipient: string
  amount?: string
  fee: string
  nonce: string
  memo: string
}): Promise<void> => {
  const networkName = BUILT_IN_NETWORK_NAMES[networkId] ?? networkId
  const approved = await snap.request({
    method: "snap_dialog",
    params: {
      type: "confirmation",
      content: (
        <Box>
          <Heading>{title}</Heading>
          <Text>Requesting site: <Bold>{origin}</Bold></Text>
          <Section>
            <Row label="Operation"><Text>{operation}</Text></Row>
            <Row label="Network"><Text>{`${networkName} (${networkId})`}</Text></Row>
            <Row label="Signature domain"><Text>{signingDomain(networkId)}</Text></Row>
            <Row label="Signing account"><Copyable value={signingPublicKey} /></Row>
            <Row label={recipientLabel}><Copyable value={recipient} /></Row>
            {amount === undefined
              ? null
              : <Row label="Amount"><Text>{`${amount} nanomina`}</Text></Row>}
            <Row label="Fee"><Text>{`${fee} nanomina`}</Text></Row>
            <Row label="Nonce"><Text>{nonce}</Text></Row>
            <Row label="Memo"><Copyable value={memo || "(empty)"} /></Row>
          </Section>
        </Box>
      )
    }
  })
  if (!approved) throw new UserRejectedRequestError(`${title} was rejected`)
}

const toJson = (value: unknown): Json => {
  if (typeof value === "bigint") return value.toString()
  if (value === null || typeof value === "string" ||
      typeof value === "number" || typeof value === "boolean") return value
  if (Array.isArray(value)) return value.map(toJson)
  if (typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, toJson(item)])
    )
  }
  throw new Error("Mina signer returned a non-JSON value")
}

const graphql = async (
  url: string,
  query: string,
  variables: Record<string, Json> = {}
): Promise<Record<string, unknown>> => {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query, variables })
  })
  if (!response.ok) throw new Error(`Mina node returned HTTP ${response.status}`)
  const result = await response.json() as {
    data?: Record<string, unknown>
    errors?: Array<{ message?: string }>
  }
  if (result.errors?.length) {
    throw new Error(result.errors[0]?.message ?? "Mina node rejected the request")
  }
  if (!result.data) throw new Error("Mina node returned no GraphQL data")
  return result.data
}

const minaToNanomina = (value: unknown, label: string): string => {
  if ((typeof value !== "number" && typeof value !== "string") ||
      (typeof value === "number" && (!Number.isFinite(value) || value < 0))) {
    throw new InvalidParamsError(`${label} must be a non-negative decimal number`)
  }
  const input = typeof value === "string" ? value.trim() : value.toString()
  if (input.length > 128) throw new InvalidParamsError(`${label} exceeds Mina UInt64`)
  const match = input.match(/^(\d+)(?:\.(\d+))?(?:e([+-]?\d+))?$/iu)
  if (!match) throw new InvalidParamsError(`${label} is invalid`)
  const whole = match[1] ?? "0"
  const fraction = match[2] ?? ""
  const exponent = BigInt(match[3] ?? 0)
  const digits = `${whole}${fraction}`.replace(/^0+(?=\d)/u, "")
  if (digits === "0") return "0"
  const decimalPlaces = BigInt(fraction.length) - exponent
  const nanoPlaces = decimalPlaces - 9n
  let nanomina: string
  if (nanoPlaces > 0n) {
    if (nanoPlaces >= BigInt(digits.length)) {
      throw new InvalidParamsError(`${label} supports at most 9 decimal places`)
    }
    const places = Number(nanoPlaces)
    const discarded = digits.slice(-places)
    if (/[1-9]/u.test(discarded)) {
      throw new InvalidParamsError(`${label} supports at most 9 decimal places`)
    }
    nanomina = (digits.slice(0, -places) || "0").replace(/^0+(?=\d)/u, "")
  } else {
    const zeroes = -nanoPlaces
    if (BigInt(digits.length) + zeroes > BigInt(MAX_UINT64.toString().length)) {
      throw new InvalidParamsError(`${label} exceeds Mina UInt64`)
    }
    nanomina = `${digits}${"0".repeat(Number(zeroes))}`.replace(/^0+(?=\d)/u, "")
  }
  if (BigInt(nanomina) > MAX_UINT64) {
    throw new InvalidParamsError(`${label} exceeds Mina UInt64`)
  }
  return nanomina
}

const getNetworkUrl = (state: MinaSnapState): string => {
  const url = state.chains[state.selectedNetwork]?.url
  if (!url) {
    throw new Error(`Add a GraphQL endpoint for ${state.selectedNetwork} before broadcasting`)
  }
  return url
}

const getNonce = async (
  state: MinaSnapState,
  publicKey: string,
  supplied: unknown
): Promise<string> => {
  if (supplied !== undefined) {
    if (typeof supplied !== "number" ||
        !Number.isSafeInteger(supplied) || supplied < 0) {
      throw new InvalidParamsError("Nonce must be a non-negative safe integer")
    }
    return supplied.toString()
  }
  const data = await graphql(
    getNetworkUrl(state),
    "query MinaAccountNonce($publicKey: PublicKey!) { account(publicKey: $publicKey) { nonce } }",
    { publicKey }
  )
  const account = data.account
  if (account === null) return "0"
  const nonce = typeof account === "object" && account !== null
    ? (account as { nonce?: unknown }).nonce
    : undefined
  if (typeof nonce !== "string" && typeof nonce !== "number") {
    throw new Error("The Mina node returned no account nonce")
  }
  return String(nonce)
}

const sendPaymentMutation = `
  mutation MinaSnapPayment(
    $fee: UInt64!, $amount: UInt64!, $to: PublicKey!, $from: PublicKey!,
    $nonce: UInt32, $memo: String, $field: String!, $scalar: String!
  ) {
    sendPayment(
      input: { fee: $fee, amount: $amount, to: $to, from: $from, memo: $memo, nonce: $nonce },
      signature: { field: $field, scalar: $scalar }
    ) { payment { hash id } }
  }
`

const sendDelegationMutation = `
  mutation MinaSnapDelegation(
    $fee: UInt64!, $to: PublicKey!, $from: PublicKey!, $nonce: UInt32,
    $memo: String, $field: String, $scalar: String
  ) {
    sendDelegation(
      input: { fee: $fee, to: $to, from: $from, memo: $memo, nonce: $nonce },
      signature: { field: $field, scalar: $scalar }
    ) { delegation { hash id } }
  }
`

const walletAccountsQuery = `
  query MinaSnapWallet($publicKey: PublicKey!) {
    accounts(publicKey: $publicKey) {
      tokenId
      tokenSymbol
      balance { total liquid locked }
      nonce
    }
  }
`

type WalletIdentity = Pick<
  WalletHomeSnapshot,
  "publicKey" | "networkId" | "networkName" | "endpoint"
>

const loadWalletIdentity = async (): Promise<WalletIdentity> => {
  const [publicKey, state] = await Promise.all([derivePublicKey(), getState()])
  const endpoint = state.chains[state.selectedNetwork]?.url
  const networkName = state.chains[state.selectedNetwork]?.name ??
    BUILT_IN_NETWORK_NAMES[state.selectedNetwork] ?? state.selectedNetwork
  return {
    publicKey,
    networkId: state.selectedNetwork,
    networkName,
    ...(endpoint ? { endpoint } : {})
  }
}

const loadWalletHome = async (identity?: WalletIdentity): Promise<WalletHomeSnapshot> => {
  const base = identity ?? await loadWalletIdentity()
  const endpoint = base.endpoint
  if (!endpoint) {
    return {
      ...base,
      tokens: [],
      error: missingEndpointMessage(base.networkId)
    }
  }
  try {
    const data = await graphql(endpoint, walletAccountsQuery, {
      publicKey: base.publicKey
    })
    const accounts = parseWalletAccounts(data.accounts)
    return { ...base, ...accounts }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    return {
      ...base,
      tokens: [],
      error: `Could not load balances: ${message.slice(0, 240)}`
    }
  }
}

type PaymentResult = { hash: string; paymentId?: string }

const sendMinaPayment = async (
  state: MinaSnapState,
  params: Record<string, unknown>,
  requester: string
): Promise<PaymentResult> => {
  const to = params.to
  if (typeof to !== "string") throw new InvalidParamsError("The recipient is invalid")
  const publicKey = await derivePublicKey()
  const client = getClient(state.selectedNetwork)
  validatePublicKey(client, to)
  const nonce = await getNonce(state, publicKey, params.nonce)
  const feeInput = params.fee ?? 0.1
  const amountInput = params.amount
  const fee = minaToNanomina(feeInput, "Fee")
  const amount = minaToNanomina(amountInput, "Amount")
  if (BigInt(amount) === 0n) throw new InvalidParamsError("Amount must be greater than zero")
  const memo = typeof params.memo === "string" ? params.memo : ""
  await approveTransactionSigning({
    origin: requester,
    title: "Send Mina payment",
    operation: "Payment (sign and submit)",
    networkId: state.selectedNetwork,
    signingPublicKey: publicKey,
    recipientLabel: "Recipient",
    recipient: to,
    amount,
    fee,
    nonce,
    memo
  })
  const account = await deriveAccount()
  if (account.publicKey !== publicKey) {
    throw new Error("The derived Mina account changed during approval")
  }
  const signed = client.signPayment({
    to,
    from: account.publicKey,
    amount,
    fee,
    nonce,
    memo
  }, account.privateKey)
  if (!client.verifyTransaction(signed)) {
    throw new Error("Mina signer failed to verify its payment signature")
  }
  const data = await graphql(getNetworkUrl(state), sendPaymentMutation, {
    ...(signed.data as unknown as Record<string, Json>),
    field: signed.signature.field,
    scalar: signed.signature.scalar
  })
  const payment = typeof data.sendPayment === "object" && data.sendPayment !== null
    ? (data.sendPayment as { payment?: unknown }).payment
    : undefined
  const result = typeof payment === "object" && payment !== null
    ? payment as Record<string, unknown>
    : {}
  if (typeof result.hash !== "string") throw new Error("Mina node returned no payment hash")
  return {
    hash: result.hash,
    ...(typeof result.id === "string" ? { paymentId: result.id } : {})
  }
}

export const onHomePage: OnHomePageHandler = async () => ({
  content: renderWalletHome(await loadWalletHome())
})

export const onUserInput: OnUserInputHandler = async ({ id, event }) => {
  if (event.type === UserInputEventType.ButtonClickEvent &&
      event.name === "refresh-balances") {
    const identity = await loadWalletIdentity()
    await snap.request({
      method: "snap_updateInterface",
      params: { id, ui: renderWalletLoading(identity) }
    })
    await snap.request({
      method: "snap_updateInterface",
      params: { id, ui: renderWalletHome(await loadWalletHome(identity)) }
    })
    return
  }
  if (event.type !== UserInputEventType.FormSubmitEvent || event.name !== "send-mina") return
  const identity = await loadWalletIdentity()
  try {
    const result = await sendMinaPayment(await getState(), {
      to: event.value.recipient,
      amount: event.value.amount,
      fee: event.value.fee || "0.1",
      memo: event.value.memo || ""
    }, "MetaMask wallet home")
    const snapshot = await loadWalletHome(identity)
    await snap.request({
      method: "snap_updateInterface",
      params: {
        id,
        ui: renderWalletHome({
          ...snapshot,
          transaction: {
            severity: "success",
            title: "Payment submitted",
            message: `Transaction ${result.hash}`
          }
        })
      }
    })
  } catch (error) {
    const snapshot = await loadWalletHome(identity)
    await snap.request({
      method: "snap_updateInterface",
      params: {
        id,
        ui: renderWalletHome({
          ...snapshot,
          transaction: {
            severity: "warning",
            title: "Payment not sent",
            message: error instanceof Error ? error.message.slice(0, 240) : String(error).slice(0, 240)
          }
        })
      }
    })
  }
}

export const onRpcRequest: OnRpcRequestHandler = async ({ origin, request }) => {
  if (request.method === "mina_requestAccounts") {
    const publicKey = await derivePublicKey()
    const state = await getState()
    if (!state.origins[origin]) {
      const approved = await snap.request({
        method: "snap_dialog",
        params: {
          type: "confirmation",
          content: (
            <Box>
              <Heading>Connect Mina account</Heading>
              <Text>Allow <Bold>{origin}</Bold> to view this Mina address?</Text>
              <Text>{publicKey}</Text>
            </Box>
          )
        }
      })
      if (!approved) {
        throw new UserRejectedRequestError("Mina account connection was rejected")
      }
      state.origins[origin] = { account: 0 }
      await setState(state)
    }
    return [publicKey]
  }
  if (request.method === "mina_accounts") {
    const state = await getState()
    if (!state.origins[origin]) return []
    return [await derivePublicKey(state.origins[origin].account)]
  }
  if (request.method === "mina_requestNetwork") {
    const state = await getState()
    const chain = state.chains[state.selectedNetwork]
    return {
      networkID: state.selectedNetwork,
      ...(chain ? chain : {})
    }
  }
  if (request.method === "wallet_info") {
    return { version: "0.1.0", init: true }
  }
  if (request.method === "wallet_revokePermissions") {
    const state = await getState()
    delete state.origins[origin]
    await setState(state)
    return []
  }
  if (request.method === "mina_switchChain") {
    const state = await requireConnection(origin)
    const networkID = typeof request.params === "object" && request.params !== null &&
      "networkID" in request.params && typeof request.params.networkID === "string"
      ? request.params.networkID
      : ""
    if (!BUILT_IN_NETWORKS.has(networkID) && !state.chains[networkID]) {
      throw new Error(`Unsupported Mina chain: ${networkID}`)
    }
    if (state.selectedNetwork !== networkID) {
      await approveSigning(origin, "Switch Mina network", networkID)
      state.selectedNetwork = networkID
      await setState(state)
    }
    return { networkID }
  }
  if (request.method === "mina_addChain") {
    const state = await requireConnection(origin)
    if (typeof request.params !== "object" || request.params === null) {
      throw new InvalidParamsError("mina_addChain requires a URL and name")
    }
    const { url, name } = request.params as Record<string, unknown>
    if (typeof url !== "string" || typeof name !== "string" || !name.trim()) {
      throw new InvalidParamsError("mina_addChain requires a URL and name")
    }
    let parsedUrl: URL
    try {
      parsedUrl = new URL(decodeURIComponent(url))
    } catch {
      throw new InvalidParamsError("The Mina GraphQL URL is invalid")
    }
    if (parsedUrl.protocol !== "https:" && parsedUrl.protocol !== "http:") {
      throw new InvalidParamsError("Mina GraphQL URLs must use HTTP or HTTPS")
    }
    await approveSigning(
      origin,
      "Contact Mina network",
      `${name.trim()} at ${parsedUrl.href}`
    )
    const data = await graphql(parsedUrl.href, "query MinaNetworkId { networkID }")
    if (typeof data.networkID !== "string" || !data.networkID) {
      throw new Error("The GraphQL endpoint returned no Mina networkID")
    }
    await approveSigning(
      origin,
      "Add Mina network",
      `${name.trim()} reports network ID ${data.networkID}`
    )
    state.chains[data.networkID] = { url: parsedUrl.href, name: name.trim() }
    state.selectedNetwork = data.networkID
    await setState(state)
    return { networkID: data.networkID }
  }
  if (request.method === "mina_signMessage") {
    const state = await requireConnection(origin)
    const message = readMessage(request.params)
    await approveSigning(origin, "Sign Mina message", message)
    const account = await deriveAccount()
    const client = getClient(state.selectedNetwork)
    const signed = client.signMessage(
      message,
      account.privateKey
    )
    if (!client.verifyMessage(signed)) {
      throw new Error("Mina signer failed to verify its message signature")
    }
    return signed
  }
  if (request.method === "mina_verifyMessage" ||
      request.method === "mina_verify_JsonMessage") {
    return getClient((await getState()).selectedNetwork).verifyMessage(
      readSignedMessage(request.params)
    )
  }
  if (request.method === "mina_sign_JsonMessage") {
    const state = await requireConnection(origin)
    if (typeof request.params !== "object" || request.params === null ||
        !("message" in request.params) || !Array.isArray(request.params.message)) {
      throw new InvalidParamsError("mina_sign_JsonMessage requires a message array")
    }
    const message = JSON.stringify(request.params.message)
    await approveSigning(origin, "Sign Mina JSON message", message)
    const account = await deriveAccount()
    const client = getClient(state.selectedNetwork)
    const signed = client.signMessage(message, account.privateKey)
    if (!client.verifyMessage(signed)) {
      throw new Error("Mina signer failed to verify its JSON-message signature")
    }
    return signed
  }
  if (request.method === "mina_signFields") {
    const state = await requireConnection(origin)
    const fields = readFields(request.params, "message")
    await approveSigning(origin, "Sign Mina fields", fields.join(", "))
    const account = await deriveAccount()
    const signed = getClient(state.selectedNetwork).signFields(
      fields.map(BigInt),
      account.privateKey
    )
    if (!getClient(state.selectedNetwork).verifyFields(signed)) {
      throw new Error("Mina signer failed to verify its field signature")
    }
    return {
      data: fields,
      publicKey: signed.publicKey,
      signature: signed.signature
    } as Json
  }
  if (request.method === "mina_sendPayment" ||
      request.method === "mina_sendStakeDelegation") {
    const state = await requireConnection(origin)
    if (typeof request.params !== "object" || request.params === null) {
      throw new InvalidParamsError(`${request.method} requires transaction parameters`)
    }
    const params = request.params as Record<string, unknown>
    if (request.method === "mina_sendPayment") {
      return sendMinaPayment(state, params, origin)
    }
    const to = params.to
    if (typeof to !== "string") throw new InvalidParamsError("The recipient is invalid")
    if (request.method === "mina_sendStakeDelegation" &&
        state.selectedNetwork.startsWith("zeko")) {
      throw new Error("Delegation is not supported on Zeko")
    }
    const publicKey = await derivePublicKey()
    const client = getClient(state.selectedNetwork)
    validatePublicKey(client, to)
    const nonce = await getNonce(state, publicKey, params.nonce)
    const fee = minaToNanomina(params.fee ?? 0.1, "Fee")
    const memo = typeof params.memo === "string" ? params.memo : ""
    await approveTransactionSigning({
      origin,
      title: "Send Mina delegation",
      operation: "Stake delegation (sign and submit)",
      networkId: state.selectedNetwork,
      signingPublicKey: publicKey,
      recipientLabel: "Delegate",
      recipient: to,
      fee,
      nonce,
      memo
    })
    const account = await deriveAccount()
    if (account.publicKey !== publicKey) {
      throw new Error("The derived Mina account changed during approval")
    }
    const signed = client.signStakeDelegation({
      to,
      from: account.publicKey,
      fee,
      nonce,
      memo
    }, account.privateKey)
    if (!client.verifyTransaction(signed)) {
      throw new Error("Mina signer failed to verify its delegation signature")
    }
    const data = await graphql(getNetworkUrl(state), sendDelegationMutation, {
      ...(signed.data as unknown as Record<string, Json>),
      field: signed.signature.field,
      scalar: signed.signature.scalar
    })
    const delegation = typeof data.sendDelegation === "object" && data.sendDelegation !== null
      ? (data.sendDelegation as { delegation?: unknown }).delegation
      : undefined
    const result = typeof delegation === "object" && delegation !== null
      ? delegation as Record<string, unknown>
      : {}
    if (typeof result.hash !== "string") throw new Error("Mina node returned no delegation hash")
    return {
      hash: result.hash,
      ...(typeof result.id === "string" ? { paymentId: result.id } : {})
    }
  }
  if (request.method === "mina_sendTransaction") {
    const state = await requireConnection(origin)
    const {
      command,
      onlySign,
      feePayerMemo,
      feePayerFee,
      nonce: nonceOverride
    } = readZkappCommand(request.params)
    const feePayer = command.feePayer
    const feePayerRecord = typeof feePayer === "object" && feePayer !== null
      ? feePayer as Record<string, unknown>
      : undefined
    const feePayerBody = feePayerRecord?.body
    if (typeof feePayerBody !== "object" || feePayerBody === null) {
      throw new InvalidParamsError("The zkApp transaction has no fee payer body")
    }
    const body = feePayerBody as Record<string, unknown>
    const publicKey = body.publicKey
    const commandFee = body.fee
    const commandNonce = body.nonce
    const feePayerAuthorization = feePayerRecord?.authorization
    if (typeof publicKey !== "string" ||
        (typeof commandFee !== "string" && typeof commandFee !== "number") ||
        (typeof commandNonce !== "string" && typeof commandNonce !== "number") ||
        typeof feePayerAuthorization !== "string") {
      throw new InvalidParamsError("The zkApp fee payer is invalid")
    }
    const fee = feePayerFee === undefined
      ? readUnsigned(commandFee, "Fee", MAX_UINT64, "UInt64")
      : minaToNanomina(feePayerFee, "Fee")
    let nonce = readUnsigned(commandNonce, "Nonce", MAX_UINT32, "UInt32")
    if (nonceOverride !== undefined) {
      if (typeof nonceOverride !== "number" ||
          !Number.isSafeInteger(nonceOverride) || nonceOverride < 0) {
        throw new InvalidParamsError("Nonce must be a non-negative safe integer")
      }
      nonce = readUnsigned(nonceOverride, "Nonce", MAX_UINT32, "UInt32")
    }
    const validUntil = body.validUntil === null || body.validUntil === undefined
      ? null
      : readUnsigned(body.validUntil, "Valid until", MAX_UINT32, "UInt32")
    const memo = feePayerMemo ?? readCommandMemo(command.memo)
    if (new TextEncoder().encode(memo).length > 32) {
      throw new InvalidParamsError("The zkApp memo exceeds 32 bytes")
    }
    const connectedPublicKey = await derivePublicKey()
    const signsFeePayer = publicKey === connectedPublicKey
    const signedUpdateIndexes = signedAccountUpdateIndexes(command, connectedPublicKey)
    if (!signsFeePayer && signedUpdateIndexes.length === 0) {
      throw new InvalidParamsError(
        "The zkApp transaction does not request a signature from the connected account"
      )
    }
    if (!signsFeePayer && !onlySign) {
      throw new InvalidParamsError(
        "A zkApp with a different fee payer can only be partially signed"
      )
    }
    if (!signsFeePayer &&
        (feePayerFee !== undefined || feePayerMemo !== undefined || nonceOverride !== undefined)) {
      throw new InvalidParamsError("Fee-payer overrides require the connected fee-payer account")
    }
    const signingPayload = {
      zkappCommand: command,
      feePayer: {
        feePayer: publicKey,
        fee,
        nonce,
        validUntil,
        memo
      }
    }
    const updates = readZkappUpdateReviews(command, signedUpdateIndexes)
    const approvalPayload = {
      onlySign,
      networkId: state.selectedNetwork,
      signatureDomain: signingDomain(state.selectedNetwork),
      signingPublicKey: connectedPublicKey,
      transaction: signingPayload
    }
    await approveZkappSigning({
      origin,
      onlySign,
      networkId: state.selectedNetwork,
      signingPublicKey: connectedPublicKey,
      feePayerPublicKey: publicKey,
      fee,
      nonce,
      validUntil,
      memo,
      updates,
      payloadHash: sha256Hex(canonicalJson(approvalPayload))
    })
    const account = await deriveAccount()
    if (account.publicKey !== connectedPublicKey) {
      throw new Error("The derived Mina account changed during approval")
    }
    const client = getClient(state.selectedNetwork, getZkappEra(command))
    const signed = client.signTransaction(signingPayload as never, account.privateKey) as unknown as {
      data: { zkappCommand?: unknown }
    }
    if (signsFeePayer && !client.verifyTransaction(signed as never)) {
      throw new Error("Mina signer failed to verify its zkApp signature")
    }
    if (!signsFeePayer) {
      const signedCommand = signed.data.zkappCommand
      const signedFeePayer = typeof signedCommand === "object" && signedCommand !== null
        ? (signedCommand as { feePayer?: unknown }).feePayer
        : undefined
      if (typeof signedFeePayer !== "object" || signedFeePayer === null) {
        throw new Error("Mina signer returned no fee payer")
      }
      const mutableFeePayer = signedFeePayer as { authorization: string }
      mutableFeePayer.authorization = feePayerAuthorization
      const signedUpdates = typeof signedCommand === "object" && signedCommand !== null &&
        Array.isArray((signedCommand as { accountUpdates?: unknown }).accountUpdates)
        ? (signedCommand as { accountUpdates: unknown[] }).accountUpdates
        : []
      const missingSignature = signedUpdateIndexes.some((index) => {
        const update = signedUpdates[index]
        const authorization = typeof update === "object" && update !== null
          ? (update as { authorization?: unknown }).authorization
          : undefined
        const signature = typeof authorization === "object" && authorization !== null
          ? (authorization as { signature?: unknown }).signature
          : undefined
        return typeof signature !== "string" || signature.length === 0
      })
      if (missingSignature) {
        throw new Error("Mina signer did not sign every matching zkApp account update")
      }
    }
    if (onlySign) return { signedData: JSON.stringify(signed.data) }
    const signedData = signed.data as { zkappCommand?: unknown }
    const data = await graphql(
      getNetworkUrl(state),
      `mutation MinaSnapZkapp($zkappCommandInput: ZkappCommandInput!) {
        sendZkapp(input: { zkappCommand: $zkappCommandInput }) { zkapp { hash id } }
      }`,
      { zkappCommandInput: toJson(signedData.zkappCommand) }
    )
    const zkapp = typeof data.sendZkapp === "object" && data.sendZkapp !== null
      ? (data.sendZkapp as { zkapp?: unknown }).zkapp
      : undefined
    const result = typeof zkapp === "object" && zkapp !== null
      ? zkapp as Record<string, unknown>
      : {}
    if (typeof result.hash !== "string") throw new Error("Mina node returned no zkApp hash")
    return {
      hash: result.hash,
      ...(typeof result.id === "string" ? { paymentId: result.id } : {})
    }
  }
  if (request.method === "mina_verifyFields") {
    if (typeof request.params !== "object" || request.params === null) {
      throw new InvalidParamsError("mina_verifyFields requires signed fields")
    }
    const data = readFields(request.params, "data")
    const { publicKey, signature } = request.params as Record<string, unknown>
    if (typeof publicKey !== "string" || typeof signature !== "string") {
      throw new InvalidParamsError("mina_verifyFields requires a public key and signature")
    }
    return getClient((await getState()).selectedNetwork).verifyFields({
      data: data.map(BigInt),
      publicKey,
      signature
    })
  }
  if (request.method === "mina_createNullifier") {
    const state = await requireConnection(origin)
    const fields = readFields(request.params, "message")
    await approveSigning(origin, "Create Mina nullifier", fields.join(", "))
    const account = await deriveAccount()
    return toJson(getClient(state.selectedNetwork).createNullifier(
      fields.map(BigInt),
      account.privateKey
    ))
  }
  if (request.method === "mina_storePrivateCredential") {
    const state = await requireConnection(origin)
    if (typeof request.params !== "object" || request.params === null ||
        !("credential" in request.params)) {
      throw new InvalidParamsError("mina_storePrivateCredential requires a credential")
    }
    const credential = JSON.stringify(request.params.credential)
    if (credential.length > 100_000) {
      throw new InvalidParamsError("The credential exceeds the 100 KB Snap storage limit")
    }
    await approveSigning(origin, "Store private credential", credential)
    const publicKey = await derivePublicKey()
    const credentials = state.credentials[publicKey] ?? []
    if (!credentials.includes(credential)) credentials.push(credential)
    state.credentials[publicKey] = credentials
    await setState(state)
    return { credential }
  }
  if (request.method === "mina_requestPresentation") {
    throw new MethodNotSupportedError(
      "Private-credential presentations require o1js/mina-attestations and are not part of this signing Snap"
    )
  }
  throw new MethodNotSupportedError(`Unsupported method: ${request.method}`)
}
