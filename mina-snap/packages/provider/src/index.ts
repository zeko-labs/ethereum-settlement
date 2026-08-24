export const DEFAULT_MINA_SNAP_ID = "npm:@zeko-labs/mina-snap"
export const DEFAULT_MINA_SNAP_VERSION = "0.1.0"

export type ProviderError = Error & { code: number; data?: unknown }
export type RequestArguments = { method: string; params?: unknown[] | object }
export type ChainInfo = { networkID: string; url?: string; name?: string }
export type SignedData = {
  publicKey: string
  data: string
  signature: { field: string; scalar: string }
}
export type SignedFieldsData = {
  data: Array<string | number>
  publicKey: string
  signature: string
}
export type SendTransactionArgs = {
  onlySign?: boolean
  nonce?: number
  transaction: string | object
  feePayer?: { fee?: number; memo?: string }
}
export type SendTransactionResult =
  | { hash: string; paymentId?: string }
  | { signedData: string }
export type Nullifier = {
  publicKey: { x: bigint; y: bigint }
  public: { nullifier: { x: bigint; y: bigint }; s: bigint }
  private: {
    c: bigint
    g_r: { x: bigint; y: bigint }
    h_m_pk_r: { x: bigint; y: bigint }
  }
}

export interface MetaMaskProvider {
  request(args: { method: string; params?: unknown }): Promise<unknown>
}

export const discoverMetaMaskProvider = (
  target: EventTarget & { ethereum?: unknown }
): MetaMaskProvider | undefined => {
  let discovered: MetaMaskProvider | undefined
  const onAnnouncement = (event: Event) => {
    const detail = (event as CustomEvent<{
      info?: { rdns?: string }
      provider?: unknown
    }>).detail
    if ((detail?.info?.rdns !== "io.metamask" &&
        detail?.info?.rdns !== "io.metamask.flask") ||
        typeof detail.provider !== "object" || detail.provider === null ||
        !("request" in detail.provider) ||
        typeof detail.provider.request !== "function") return
    discovered = detail.provider as MetaMaskProvider
  }
  target.addEventListener("eip6963:announceProvider", onAnnouncement)
  target.dispatchEvent(new Event("eip6963:requestProvider"))
  target.removeEventListener("eip6963:announceProvider", onAnnouncement)
  if (discovered) return discovered
  const injected = target.ethereum
  if (typeof injected === "object" && injected !== null &&
      "request" in injected && typeof injected.request === "function" &&
      "isMetaMask" in injected && injected.isMetaMask === true) {
    return injected as MetaMaskProvider
  }
  return undefined
}

export interface IMinaProvider {
  request(args: RequestArguments): Promise<unknown>
  requestAccounts(): Promise<string[] | ProviderError>
  getAccounts(): Promise<string[]>
  requestNetwork(): Promise<ChainInfo>
  sendTransaction(args: SendTransactionArgs): Promise<SendTransactionResult | ProviderError>
  sendPayment(args: {
    to: string
    amount: number
    fee?: number
    memo?: string
    nonce?: number
  }): Promise<{ hash: string; paymentId?: string } | ProviderError>
  sendStakeDelegation(args: {
    to: string
    fee?: number
    memo?: string
    nonce?: number
  }): Promise<{ hash: string; paymentId?: string } | ProviderError>
  signMessage(args: { message: string }): Promise<SignedData | ProviderError>
  verifyMessage(args: SignedData): Promise<boolean | ProviderError>
  signFields(args: { message: Array<string | number> }): Promise<SignedFieldsData | ProviderError>
  verifyFields(args: SignedFieldsData): Promise<boolean | ProviderError>
  signJsonMessage(args: {
    message: Array<{ label: string; value: string }>
  }): Promise<SignedData | ProviderError>
  verifyJsonMessage(args: SignedData): Promise<boolean | ProviderError>
  createNullifier(args: { message: Array<string | number> }): Promise<Nullifier | ProviderError>
  storePrivateCredential(args: { credential: unknown }): Promise<{ credential: string } | ProviderError>
  requestPresentation(args: { presentation: unknown }): Promise<{ presentation: string } | ProviderError>
  revokePermissions(): Promise<string[]>
  addChain(args: { url: string; name: string }): Promise<ChainInfo | ProviderError>
  switchChain(args: { networkID: string }): Promise<ChainInfo | ProviderError>
  getWalletInfo(): Promise<{ version: string; init: boolean }>
  on(event: "accountsChanged", listener: (accounts: string[]) => void): this
  on(event: "chainChanged", listener: (chain: ChainInfo) => void): this
  on(event: "networkChanged", listener: (network: string) => void): this
  removeListener(event: "accountsChanged", listener: (accounts: string[]) => void): this
  removeListener(event: "chainChanged", listener: (chain: ChainInfo) => void): this
  removeListener(event: "networkChanged", listener: (network: string) => void): this
  removeAllListeners(event?: string): this
}

type EventName = "accountsChanged" | "chainChanged" | "networkChanged"
type EventValue = {
  accountsChanged: string[]
  chainChanged: ChainInfo
  networkChanged: string
}
type EventListener = (value: unknown) => void

export class MinaSnapProvider implements IMinaProvider {
  readonly isAuro = false
  readonly isMinaSnap = true
  readonly snapId: string
  readonly version: string
  readonly #ethereum: MetaMaskProvider
  readonly #listeners = new Map<EventName, Set<EventListener>>()
  #installPromise?: Promise<void>

  constructor(
    ethereum: MetaMaskProvider,
    options: { snapId?: string; version?: string } = {}
  ) {
    this.#ethereum = ethereum
    this.snapId = options.snapId ?? DEFAULT_MINA_SNAP_ID
    this.version = options.version ?? DEFAULT_MINA_SNAP_VERSION
  }

  async connectSnap(): Promise<void> {
    this.#installPromise ??= this.#ensureInstalled().catch((error: unknown) => {
      this.#installPromise = undefined
      throw error
    })
    return this.#installPromise
  }

  async #ensureInstalled(): Promise<void> {
    await this.#ethereum.request({ method: "wallet_getSnaps" })
    await this.#ethereum.request({
      method: "wallet_requestSnaps",
      params: {
        [this.snapId]: this.snapId.startsWith("local:")
          ? {}
          : { version: this.version }
      }
    })
  }

  async request({ method, params }: RequestArguments): Promise<unknown> {
    try {
      await this.connectSnap()
      return await this.#ethereum.request({
        method: "wallet_invokeSnap",
        params: {
          snapId: this.snapId,
          request: {
            method,
            ...(params === undefined ? {} : { params })
          }
        }
      })
    } catch (error) {
      throw toAuroError(error)
    }
  }

  async requestAccounts() {
    const accounts = await this.request({ method: "mina_requestAccounts" }) as string[]
    this.#emit("accountsChanged", accounts)
    return accounts
  }

  async getAccounts() {
    return this.request({ method: "mina_accounts" }) as Promise<string[]>
  }

  async requestNetwork() {
    return this.request({ method: "mina_requestNetwork" }) as Promise<ChainInfo>
  }

  async sendTransaction(args: SendTransactionArgs) {
    return this.request({ method: "mina_sendTransaction", params: args }) as Promise<SendTransactionResult>
  }

  async sendPayment(args: { to: string; amount: number; fee?: number; memo?: string; nonce?: number }) {
    return this.request({ method: "mina_sendPayment", params: args }) as Promise<{ hash: string; paymentId?: string }>
  }

  async sendStakeDelegation(args: { to: string; fee?: number; memo?: string; nonce?: number }) {
    return this.request({ method: "mina_sendStakeDelegation", params: args }) as Promise<{ hash: string; paymentId?: string }>
  }

  async signMessage(args: { message: string }) {
    return this.request({ method: "mina_signMessage", params: args }) as Promise<SignedData>
  }

  async verifyMessage(args: SignedData) {
    return this.request({ method: "mina_verifyMessage", params: args }) as Promise<boolean>
  }

  async signFields(args: { message: Array<string | number> }) {
    return this.request({ method: "mina_signFields", params: args }) as Promise<SignedFieldsData>
  }

  async verifyFields(args: SignedFieldsData) {
    return this.request({ method: "mina_verifyFields", params: args }) as Promise<boolean>
  }

  async signJsonMessage(args: { message: Array<{ label: string; value: string }> }) {
    return this.request({ method: "mina_sign_JsonMessage", params: args }) as Promise<SignedData>
  }

  async verifyJsonMessage(args: SignedData) {
    return this.request({ method: "mina_verify_JsonMessage", params: args }) as Promise<boolean>
  }

  async createNullifier(args: { message: Array<string | number> }) {
    const value = await this.request({ method: "mina_createNullifier", params: args })
    return reviveBigInts(value) as Nullifier
  }

  async storePrivateCredential(args: { credential: unknown }) {
    return this.request({ method: "mina_storePrivateCredential", params: args }) as Promise<{ credential: string }>
  }

  async requestPresentation(args: { presentation: unknown }) {
    return this.request({ method: "mina_requestPresentation", params: args }) as Promise<{ presentation: string }>
  }

  async revokePermissions() {
    const result = await this.request({ method: "wallet_revokePermissions" }) as string[]
    this.#emit("accountsChanged", [])
    return result
  }

  async addChain(args: { url: string; name: string }) {
    const chain = await this.request({ method: "mina_addChain", params: args }) as ChainInfo
    this.#emit("chainChanged", chain)
    this.#emit("networkChanged", chain.networkID)
    return chain
  }

  async switchChain(args: { networkID: string }) {
    const chain = await this.request({ method: "mina_switchChain", params: args }) as ChainInfo
    this.#emit("chainChanged", chain)
    this.#emit("networkChanged", chain.networkID)
    return chain
  }

  async getWalletInfo() {
    return this.request({ method: "wallet_info" }) as Promise<{ version: string; init: boolean }>
  }

  on<Event extends EventName>(
    event: Event,
    listener: (value: EventValue[Event]) => void
  ): this {
    const listeners = this.#listeners.get(event) ?? new Set()
    listeners.add(listener as EventListener)
    this.#listeners.set(event, listeners)
    return this
  }

  removeListener<Event extends EventName>(
    event: Event,
    listener: (value: EventValue[Event]) => void
  ): this {
    this.#listeners.get(event)?.delete(listener as EventListener)
    return this
  }

  removeAllListeners(event?: string): this {
    if (event) this.#listeners.delete(event as EventName)
    else this.#listeners.clear()
    return this
  }

  #emit(event: EventName, value: unknown): void {
    for (const listener of this.#listeners.get(event) ?? []) {
      listener(value)
    }
  }
}

export const announceMinaProvider = (
  target: EventTarget,
  provider: IMinaProvider
): (() => void) => {
  const info = Object.freeze({
    slug: "zeko-mina-snap",
    name: "Zeko Mina Snap",
    icon: "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'%3E%3Ccircle cx='32' cy='32' r='30' fill='%235c3df5'/%3E%3Cpath d='M18 20h28L26 44h20' fill='none' stroke='white' stroke-width='7' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E",
    rdns: "io.zeko.snap"
  })
  const announce = () => {
    target.dispatchEvent(new CustomEvent("mina:announceProvider", {
      detail: Object.freeze({ info, provider })
    }))
  }
  target.addEventListener("mina:requestProvider", announce)
  announce()
  return () => target.removeEventListener("mina:requestProvider", announce)
}

const reviveBigInts = (value: unknown): unknown => {
  if (Array.isArray(value)) return value.map(reviveBigInts)
  if (typeof value !== "object" || value === null) return value
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [
    key,
    typeof item === "string" && /^-?[0-9]+$/u.test(item)
      ? BigInt(item)
      : reviveBigInts(item)
  ]))
}

const toAuroError = (value: unknown): ProviderError => {
  const source = typeof value === "object" && value !== null
    ? value as { code?: unknown; message?: unknown; data?: unknown }
    : {}
  const message = typeof source.message === "string" ? source.message : String(value)
  const sourceCode = typeof source.code === "number" ? source.code : undefined
  const legacyCodes = new Set([1001, 1002, 20001, 20002, 20003, 20004, 20005, 20006, 21001, 22001, 23001])
  let code = 21_001
  if (sourceCode !== undefined && legacyCodes.has(sourceCode)) code = sourceCode
  else if (sourceCode === 4001) code = 1002
  else if (sourceCode === 4100 || /connect.*before|disconnect/iu.test(message)) code = 1001
  else if (sourceCode === -32602) code = 20_003
  else if (sourceCode === -32601 || sourceCode === 4200) code = 20_006
  else if (/unsupported.*chain|chain.*unsupported|not supported on/iu.test(message)) code = 20_004
  return Object.assign(new Error(message), {
    code,
    ...(source.data === undefined ? {} : { data: source.data })
  }) as ProviderError
}
