import {
  BIP44CoinTypeNode,
  deriveBIP44AddressKey
} from "@metamask/key-tree"
import {
  InvalidParamsError,
  MethodNotSupportedError,
  UnauthorizedError,
  UserRejectedRequestError,
  type Json,
  type OnRpcRequestHandler
} from "@metamask/snaps-sdk"
import { Box, Bold, Heading, Text } from "@metamask/snaps-sdk/jsx"
import { sha256 } from "@noble/hashes/sha256"
import { base58check } from "@scure/base"
import Client from "mina-signer"

const MINA_COIN_TYPE = 12_586
const MINA_ENTROPY_PATH = ["m", "44'", "12586'"] as const
const BUILT_IN_NETWORKS = new Set([
  "mina:mainnet",
  "mina:devnet",
  "zeko:mainnet",
  "zeko:testnet",
  "testnet"
])

type MinaSnapState = {
  version: 1
  origins: Record<string, { account: number }>
  selectedNetwork: string
  chains: Record<string, { url: string; name: string }>
  credentials: Record<string, string[]>
}

const initialState = (): MinaSnapState => ({
  version: 1,
  origins: {},
  selectedNetwork: "mina:mainnet",
  chains: {
    "zeko:testnet": {
      url: "https://testnet.zeko.io/graphql",
      name: "Zeko Testnet"
    }
  },
  credentials: {}
})

const getState = async (): Promise<MinaSnapState> => {
  const stored = await snap.request({
    method: "snap_manageState",
    params: { operation: "get" }
  }) as Partial<MinaSnapState> | null
  if (
    stored?.version !== 1 ||
    typeof stored.origins !== "object" ||
    stored.origins === null ||
    typeof stored.selectedNetwork !== "string"
  ) {
    return initialState()
  }
  return {
    ...(stored as MinaSnapState),
    chains: typeof stored.chains === "object" && stored.chains !== null
      ? stored.chains
      : initialState().chains,
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

const getClient = (networkID: string, era?: "berkeley"): Client => new Client({
  network: networkID === "mina:mainnet"
    ? "mainnet"
    : networkID === "zeko:mainnet"
      ? { custom: "zeko-mainnet" }
      : "testnet",
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

const readZkappCommand = (params: unknown): {
  command: Record<string, unknown>
  onlySign: boolean
  feePayerMemo: string
  feePayerFee: unknown
  nonce: unknown
} => {
  if (typeof params !== "object" || params === null || !("transaction" in params)) {
    throw new InvalidParamsError("mina_sendTransaction requires a transaction")
  }
  const record = params as Record<string, unknown>
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
  const feePayer = typeof record.feePayer === "object" && record.feePayer !== null
    ? record.feePayer as Record<string, unknown>
    : {}
  return {
    command: parsed as Record<string, unknown>,
    onlySign: record.onlySign === true,
    feePayerMemo: typeof feePayer.memo === "string" ? feePayer.memo : "",
    feePayerFee: feePayer.fee,
    nonce: record.nonce
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
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new InvalidParamsError(`${label} must be a non-negative finite number`)
  }
  const match = value.toString().match(/^(\d+)(?:\.(\d+))?(?:e([+-]?\d+))?$/iu)
  if (!match) throw new InvalidParamsError(`${label} is invalid`)
  const whole = match[1] ?? "0"
  const fraction = match[2] ?? ""
  const exponent = Number(match[3] ?? 0)
  const digits = `${whole}${fraction}`.replace(/^0+(?=\d)/u, "")
  const decimalPlaces = fraction.length - exponent
  const nanoPlaces = decimalPlaces - 9
  if (nanoPlaces > 0) {
    const discarded = digits.slice(-nanoPlaces)
    if (/[1-9]/u.test(discarded)) {
      throw new InvalidParamsError(`${label} supports at most 9 decimal places`)
    }
    return (digits.slice(0, -nanoPlaces) || "0").replace(/^0+(?=\d)/u, "")
  }
  return `${digits}${"0".repeat(-nanoPlaces)}`.replace(/^0+(?=\d)/u, "")
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
    return { networkID: (await getState()).selectedNetwork }
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
    if (request.method === "mina_sendPayment") {
      const amount = minaToNanomina(params.amount, "Amount")
      await approveSigning(
        origin,
        "Send Mina payment",
        `${params.amount as number} MINA to ${to}; fee ${params.fee ?? 0.1} MINA`
      )
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
    await approveSigning(
      origin,
      "Send Mina delegation",
      `Delegate to ${to}; fee ${params.fee ?? 0.1} MINA`
    )
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
    const feePayerBody = typeof feePayer === "object" && feePayer !== null
      ? (feePayer as { body?: unknown }).body
      : undefined
    if (typeof feePayerBody !== "object" || feePayerBody === null) {
      throw new InvalidParamsError("The zkApp transaction has no fee payer body")
    }
    const body = feePayerBody as Record<string, unknown>
    const publicKey = body.publicKey
    const commandFee = body.fee
    const commandNonce = body.nonce
    if (typeof publicKey !== "string" ||
        (typeof commandFee !== "string" && typeof commandFee !== "number") ||
        (typeof commandNonce !== "string" && typeof commandNonce !== "number")) {
      throw new InvalidParamsError("The zkApp fee payer is invalid")
    }
    const fee = feePayerFee === undefined
      ? String(commandFee)
      : minaToNanomina(feePayerFee, "Fee")
    let nonce = String(commandNonce)
    if (nonceOverride !== undefined) {
      if (typeof nonceOverride !== "number" ||
          !Number.isSafeInteger(nonceOverride) || nonceOverride < 0) {
        throw new InvalidParamsError("Nonce must be a non-negative safe integer")
      }
      nonce = nonceOverride.toString()
    }
    const connectedPublicKey = await derivePublicKey()
    if (publicKey !== connectedPublicKey) {
      throw new InvalidParamsError("The zkApp fee payer does not match the connected account")
    }
    const accountUpdates = Array.isArray(command.accountUpdates)
      ? command.accountUpdates.length
      : 0
    await approveSigning(
      origin,
      "Sign Mina zkApp transaction",
      `${state.selectedNetwork}; fee ${fee} nanomina; nonce ${nonce}; ${accountUpdates} account update(s)`
    )
    const account = await deriveAccount()
    if (account.publicKey !== connectedPublicKey) {
      throw new Error("The derived Mina account changed during approval")
    }
    const client = getClient(state.selectedNetwork, getZkappEra(command))
    const signed = client.signTransaction({
      zkappCommand: command,
      feePayer: {
        feePayer: publicKey,
        fee,
        nonce,
        memo: feePayerMemo
      }
    } as never, account.privateKey) as unknown as { data: unknown }
    if (!client.verifyTransaction(signed as never)) {
      throw new Error("Mina signer failed to verify its zkApp signature")
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
