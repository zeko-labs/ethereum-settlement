import {
  discoverMetaMaskProvider,
  MinaSnapProvider,
  type MetaMaskProvider
} from "@zeko-labs/mina-snap-provider"
import type { Address, EIP1193Provider, Hex } from "viem"
import { getAddress } from "viem"
import type { RuntimeConfig } from "./config"
import type { MinaWalletKind } from "./storage"

export type EthereumProvider = EIP1193Provider & {
  readonly isMetaMask?: boolean
  on?: (event: "accountsChanged" | "chainChanged", listener: (value: unknown) => void) => void
  removeListener?: (event: "accountsChanged" | "chainChanged", listener: (value: unknown) => void) => void
}

export type ProviderError = { code: number; message?: string }

export type AuroSignedResult =
  | { signedData: string }
  | { hash: string }
  | ProviderError
  | Error

export type AuroProvider = {
  readonly isAuro?: boolean
  readonly isMinaSnap?: boolean
  requestAccounts: () => Promise<string[] | ProviderError>
  requestNetwork: () => Promise<{ networkID: string; url?: string; name?: string } | ProviderError>
  addChain: (input: { url: string; name: string }) => Promise<{ networkID: string } | ProviderError>
  switchChain: (input: { networkID: string }) => Promise<{ networkID: string } | ProviderError>
  sendTransaction: (input: { onlySign: true; transaction: string }) => Promise<AuroSignedResult>
  sendPayment?: (input: { to: string; amount: number; fee?: number; memo?: string; nonce?: number }) => Promise<{ hash: string; paymentId?: string } | ProviderError | Error>
  revokePermissions?: () => Promise<string[]>
  on?: {
    (event: "accountsChanged", listener: (accounts: string[]) => void): void
    (event: "chainChanged", listener: (network: { networkID: string }) => void): void
  }
  removeAllListeners?: () => void
}

declare global {
  interface Window {
    ethereum?: EthereumProvider
    mina?: AuroProvider
  }
}

export const isProviderError = (value: unknown): value is ProviderError =>
  typeof value === "object" && value !== null && "code" in value && typeof (value as ProviderError).code === "number"

export const getEthereumProvider = (): EthereumProvider => {
  if (!window.ethereum) throw new Error("No injected Ethereum wallet detected")
  return window.ethereum
}

let cachedSnapProvider: { ethereum: EthereumProvider; provider: AuroProvider } | undefined

export const getSnapProvider = (): AuroProvider => {
  const snapId = import.meta.env.VITE_MINA_SNAP_ID as string | undefined
  const ethereum = discoverMetaMaskProvider(window, snapId)
  if (!ethereum) {
    throw new Error(snapId?.startsWith("local:")
      ? "MetaMask Flask is not installed"
      : "MetaMask is not installed")
  }
  if (cachedSnapProvider?.ethereum === ethereum) return cachedSnapProvider.provider
  const provider = new MinaSnapProvider(
    ethereum,
    snapId ? { snapId } : undefined
  ) as unknown as AuroProvider
  cachedSnapProvider = { ethereum: ethereum as unknown as EthereumProvider, provider }
  return provider
}

export const defaultMinaWallet = (): MinaWalletKind => {
  const preference = import.meta.env.VITE_MINA_WALLET as string | undefined
  if (preference === "auro" || preference === "metamask-snap") return preference
  return window.mina ? "auro" : "metamask-snap"
}

export const minaWalletName = (
  wallet: MinaWalletKind,
  snapId = import.meta.env.VITE_MINA_SNAP_ID as string | undefined
): string => wallet === "auro"
  ? "Auro Wallet"
  : snapId?.startsWith("local:")
    ? "MetaMask Flask"
    : "MetaMask Snap"

export const minaWalletShortName = (
  wallet: MinaWalletKind,
  snapId = import.meta.env.VITE_MINA_SNAP_ID as string | undefined
): string => wallet === "auro"
  ? "Auro"
  : snapId?.startsWith("local:")
    ? "Flask"
    : "Snap"

export const getMinaProvider = (wallet: MinaWalletKind): AuroProvider => {
  if (wallet === "auro") {
    if (!window.mina) throw new Error("Auro Wallet is not installed")
    return window.mina
  }
  return getSnapProvider()
}

export const getAuroProvider = (): AuroProvider => getMinaProvider(defaultMinaWallet())

// Auro exposes the selected chain using its wallet-facing identifier. The
// transaction signing salt remains `testnet` and is configured separately.
export const auroPoCNetworkIds = new Set(["zeko:testnet", "testnet"])

export const isAuroPoCNetwork = (networkId: string): boolean =>
  auroPoCNetworkIds.has(networkId)

export const ensureEthereumNetwork = async (
  provider: EthereumProvider,
  expectedChainId: number
): Promise<void> => {
  const current = await provider.request({ method: "eth_chainId" })
  const currentId = Number.parseInt(String(current), 16)
  if (currentId === expectedChainId) return
  const chainId = `0x${expectedChainId.toString(16)}` as Hex
  try {
    await provider.request({
      method: "wallet_switchEthereumChain",
      params: [{ chainId }]
    })
  } catch (error) {
    if (expectedChainId !== 31_337 || !isProviderError(error) || error.code !== 4902) throw error
    await provider.request({
      method: "wallet_addEthereumChain",
      params: [{
        chainId,
        chainName: "Local Ethereum",
        nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
        // MetaMask ships a non-editable "Localhost 8545" entry with chain ID
        // 1337. Use a distinct browser-side port for this PoC's Anvil chain.
        rpcUrls: ["http://127.0.0.1:8546"]
      }]
    })
  }
  const switched = Number.parseInt(String(await provider.request({ method: "eth_chainId" })), 16)
  if (switched !== expectedChainId) throw new Error(`Ethereum wallet did not switch to chain ${expectedChainId}`)
}

export const connectEthereum = async (config: RuntimeConfig): Promise<Address> => {
  const provider = getEthereumProvider()
  const accounts = (await provider.request({ method: "eth_requestAccounts" })) as string[]
  if (!accounts[0]) throw new Error("No Ethereum account selected")
  await ensureEthereumNetwork(provider, config.expectedEthereumChainId)
  return getAddress(accounts[0])
}

export const ensureAuroPoCNetwork = async (
  provider: AuroProvider,
  config: RuntimeConfig
): Promise<void> => {
  const current = await provider.requestNetwork()
  if (isProviderError(current)) throw new Error(current.message ?? `Mina wallet error ${current.code}`)
  if (provider.isMinaSnap) {
    if (current.url && new URL(current.url).href === new URL(config.sequencerGraphqlUrl).href) {
      if (!isAuroPoCNetwork(current.networkID)) {
        throw new Error("The configured Zeko endpoint reports an unsupported network ID")
      }
      return
    }
    const added = await provider
      .addChain({ url: config.sequencerGraphqlUrl, name: config.auroNetworkName })
      .catch((error: unknown) => error)
    if (added instanceof Error) throw added
    if (isProviderError(added)) {
      throw new Error(added.message ?? `Mina wallet error ${added.code}`)
    }
    const selected = await provider.requestNetwork()
    if (isProviderError(selected)) {
      throw new Error(selected.message ?? `Mina wallet error ${selected.code}`)
    }
    if (!isAuroPoCNetwork(selected.networkID)) {
      throw new Error("The Mina Snap did not select the configured Zeko endpoint")
    }
    return
  }
  if (isAuroPoCNetwork(current.networkID)) return

  // Zeko testnet is built into current Auro releases. Selecting it is enough
  // for onlySign: the UI constructs and submits the transaction against the
  // configured local sequencer, while Auro applies its testnet signing salt.
  const switched: { networkID: string } | ProviderError | Error = await provider
    .switchChain({ networkID: "zeko:testnet" })
    .catch((error: unknown) => error as Error)
  if (
    !switched ||
    switched instanceof Error ||
    isProviderError(switched) ||
    !isAuroPoCNetwork(switched.networkID)
  ) {
    const added = await provider
      .addChain({ url: config.sequencerGraphqlUrl, name: config.auroNetworkName })
      .catch((error: unknown) => error)
    if (added instanceof Error) throw added
    if (isProviderError(added)) {
      if (added.code === 20003) {
        throw new Error(
          `Auro blocks dapps from adding local HTTP nodes. Add ${config.sequencerGraphqlUrl} manually in Auro Settings > Networks, select it, then reconnect.`
        )
      }
      throw new Error(added.message ?? `Mina wallet error ${added.code}`)
    }
  }

  const network = await provider.requestNetwork()
  if (isProviderError(network)) throw new Error(network.message ?? `Mina wallet error ${network.code}`)
  if (!isAuroPoCNetwork(network.networkID)) {
    throw new Error("The Mina wallet did not select Zeko Testnet")
  }
}

export const connectMina = async (config: RuntimeConfig, wallet: MinaWalletKind): Promise<string> => {
  const provider = getMinaProvider(wallet)
  const accounts = await provider.requestAccounts()
  if (isProviderError(accounts)) throw new Error(accounts.message ?? `Mina wallet error ${accounts.code}`)
  if (!accounts[0]) throw new Error("No Mina account selected")
  await ensureAuroPoCNetwork(provider, config)
  return accounts[0]
}

export const connectAuro = async (config: RuntimeConfig): Promise<string> =>
  connectMina(config, defaultMinaWallet())

export const shortAddress = (address: string, head = 6, tail = 4): string =>
  address.length <= head + tail + 1 ? address : `${address.slice(0, head)}…${address.slice(-tail)}`

export const formatWalletError = (error: unknown): string => {
  if (isProviderError(error)) return error.message ?? `Wallet error ${error.code}`
  if (error instanceof Error) {
    if (/reject|denied|cancel/i.test(error.message)) return "The wallet request was rejected."
    return error.message
  }
  return String(error)
}

export const listenEthereumChanges = (
  provider: EthereumProvider,
  handlers: {
    onAccounts: (accounts: string[]) => void
    onChain: (chainId: number) => void
  }
): (() => void) => {
  const onAccounts = (value: unknown) =>
    handlers.onAccounts(Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [])
  const onChain = (value: unknown) => handlers.onChain(Number.parseInt(String(value), 16))
  provider.on?.("accountsChanged", onAccounts)
  provider.on?.("chainChanged", onChain)
  return () => {
    provider.removeListener?.("accountsChanged", onAccounts)
    provider.removeListener?.("chainChanged", onChain)
  }
}

export const listenAuroChanges = (
  provider: AuroProvider,
  handlers: {
    onAccounts: (accounts: string[]) => void
    onNetwork: (networkId: string) => void
  }
): (() => void) => {
  provider.removeAllListeners?.()
  provider.on?.("accountsChanged", handlers.onAccounts)
  provider.on?.("chainChanged", (network) => handlers.onNetwork(network.networkID))
  return () => provider.removeAllListeners?.()
}
