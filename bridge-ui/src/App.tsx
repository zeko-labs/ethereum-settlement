import { activityAsset, recoverWithdrawalOperation } from "./lib/withdrawalRecovery"
import type { DepositStatus, EthereumAssetRegistrySnapshot, EthereumBridgeClient, TokenWithdrawalProof, WithdrawalProof } from "@zeko-labs/eth-bridge-sdk"
import type { Address } from "viem"
import { getAddress, isAddress } from "viem"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { ActivityView } from "./components/ActivityView"
import { BackgroundWave, DepositProgress, type Direction, Notice, WalletChip, WithdrawalProgress } from "./components/BridgeUi"
import { BridgeForm } from "./components/BridgeForm"
import { CompleteView } from "./components/CompleteView"
import { MinaWalletModal } from "./components/MinaWalletModal"
import { ReviewView } from "./components/ReviewView"
import { SettingsModal } from "./components/SettingsModal"
import { WalletView } from "./components/WalletView"
import { bridgeAmountFromEth, bridgeAmountFromToken, formatUnits } from "./lib/amount"
import {
  discoverBridgeAssets,
  ensureTokenAllowance,
  fetchAssetRegistrySnapshot,
  fetchTokenBalance,
  NATIVE_ASSET,
  type BridgeAsset
} from "./lib/assets"
import {
  createEthereumBridgeClient,
  depositNative,
  depositToken,
  ethereumTransactionUrl,
  fetchEthereumBalance,
  fetchZekoBalance,
  fetchZekoTokenBalance,
  finalizeDeposit,
  finalizeTokenDeposit,
  isValidZekoAddress,
  listWalletActivity,
  listTokenWithdrawals,
  listTokenWithdrawalRequests,
  loadBridgeModules,
  requestNativeWithdrawal,
  requestTokenWithdrawal,
  zekoTransactionUrl
} from "./lib/bridge"
import { ethereumNetworkName, loadRuntimeConfig, type RuntimeConfig } from "./lib/config"
import {
  operationStorageKey,
  readOperations,
  readMinaWallet,
  rememberMinaWallet,
  rememberMinaWalletConnection,
  type MinaWalletKind,
  type PendingOperation,
  upsertOperation,
  wasMinaWalletConnected
} from "./lib/storage"
import {
  connectMina,
  connectEthereum,
  defaultMinaWallet,
  ensureAuroPoCNetwork,
  ensureEthereumNetwork,
  formatWalletError,
  getEthereumProvider,
  getMinaProvider,
  isAuroPoCNetwork,
  minaWalletName,
  shortAddress
} from "./lib/wallets"

type Screen = "form" | "review" | "deposit-progress" | "withdrawal-progress" | "complete"
type Completion = { direction: Direction; amount: string; symbol: string; hash: string; url: string }
type MinaSessionPresentation = {
  kind: "interactive" | "reload"
  fillRecipient: boolean
}
type MinaSessionAttempt = { cancel: () => void }

const missingWalletMessage = (wallet: "ethereum" | "auro", ethereum = "Sepolia") =>
  wallet === "ethereum"
    ? `Connect an Ethereum wallet on ${ethereum} before continuing.`
    : "Connect a Mina wallet to Zeko Testnet before continuing."

export default function App() {
  const [config, setConfig] = useState<RuntimeConfig>()
  const [configError, setConfigError] = useState("")
  const [ethereumAccount, setEthereumAccount] = useState<Address>()
  const [zekoAccount, setZekoAccount] = useState<string>()
  const [ethereumBalance, setEthereumBalance] = useState<string>()
  const [zekoBalance, setZekoBalance] = useState<string>()
  const [assets, setAssets] = useState<BridgeAsset[]>([NATIVE_ASSET])
  const [asset, setAsset] = useState<BridgeAsset>(NATIVE_ASSET)
  const [assetSnapshot, setAssetSnapshot] = useState<EthereumAssetRegistrySnapshot>()
  const [assetsLoading, setAssetsLoading] = useState(false)
  const [assetWarning, setAssetWarning] = useState("")
  const [client, setClient] = useState<EthereumBridgeClient>()
  const [zekoClient, setZekoClient] = useState<EthereumBridgeClient>()
  const [bridgeAddress, setBridgeAddress] = useState("")
  const [direction, setDirection] = useState<Direction>("deposit")
  const [tab, setTab] = useState<"bridge" | "wallet" | "activity">("bridge")
  const [screen, setScreen] = useState<Screen>("form")
  const [amount, setAmount] = useState("")
  const [recipient, setRecipient] = useState("")
  const [validation, setValidation] = useState("")
  const [recipientValid, setRecipientValid] = useState(false)
  const [showDetails, setShowDetails] = useState(true)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [minaWalletOpen, setMinaWalletOpen] = useState(false)
  const [minaWallet, setMinaWallet] = useState<MinaWalletKind>(() =>
    readMinaWallet(defaultMinaWallet())
  )
  const [busy, setBusy] = useState(false)
  const [minaConnectionBusy, setMinaConnectionBusy] = useState(false)
  const [activityLoading, setActivityLoading] = useState(false)
  const [activityError, setActivityError] = useState("")
  const [deposits, setDeposits] = useState<DepositStatus[]>([])
  const [withdrawals, setWithdrawals] = useState<Array<WithdrawalProof | TokenWithdrawalProof>>([])
  const [operations, setOperations] = useState<PendingOperation[]>([])
  const [selectedDeposit, setSelectedDeposit] = useState<DepositStatus>()
  const [selectedWithdrawal, setSelectedWithdrawal] = useState<WithdrawalProof | TokenWithdrawalProof>()
  const [selectedOperation, setSelectedOperation] = useState<PendingOperation>()
  const [completion, setCompletion] = useState<Completion>()
  const [toast, setToast] = useState("")
  const [actionError, setActionError] = useState("")
  const ethereum = config ? ethereumNetworkName(config.expectedEthereumChainId) : "Sepolia"
  const activityRequest = useRef(0)
  const activityRefreshes = useRef(new Set<string>())
  const selectedOperationRef = useRef<PendingOperation>()
  const ethereumConnectionRequest = useRef(0)
  const minaConnectionRequest = useRef(0)
  const minaConnectionPending = useRef(false)
  const activeMinaSession = useRef<MinaWalletKind>()
  const minaWalletRef = useRef(minaWallet)

  useEffect(() => {
    let active = true
    void loadRuntimeConfig()
      .then((loaded) => active && setConfig(loaded))
      .catch((error: unknown) => active && setConfigError(error instanceof Error ? error.message : String(error)))
    return () => {
      active = false
    }
  }, [])

  const setEthereumConnection = useCallback(
    async (account: Address) => {
      if (!config) throw new Error("Runtime configuration is not loaded")
      const request = ++ethereumConnectionRequest.current
      const provider = getEthereumProvider()
      await ensureEthereumNetwork(provider, config.expectedEthereumChainId)
      const connectedClient = await createEthereumBridgeClient({ config, provider, account })
      if (request !== ethereumConnectionRequest.current) return connectedClient
      activityRequest.current += 1
      setEthereumAccount(account)
      setClient(connectedClient)
      setZekoClient(undefined)
      setAssets([NATIVE_ASSET])
      setAsset((current) => current.kind === "native" ? current : NATIVE_ASSET)
      setAssetSnapshot(undefined)
      setAssetWarning("")
      setBridgeAddress(connectedClient.config.bridgeAddress)
      setEthereumBalance(await fetchEthereumBalance(provider, account).catch(() => "0"))
      setAssetsLoading(true)
      try {
        const snapshot = await fetchAssetRegistrySnapshot(config.actionsApiUrl)
        if (request !== ethereumConnectionRequest.current) return connectedClient
        if (!snapshot) {
          setAssetWarning("No canonical ERC-20 registry is published yet; native ETH remains available.")
          return connectedClient
        }
        const authenticatedClient = await createEthereumBridgeClient({
          config,
          provider,
          account,
          withZeko: true,
          assetSnapshot: snapshot
        })
        const discovered = await discoverBridgeAssets(authenticatedClient, provider)
        if (request !== ethereumConnectionRequest.current) return connectedClient
        setAssetSnapshot(snapshot)
        setZekoClient(authenticatedClient)
        setAssets(discovered.assets)
        setAssetWarning(discovered.warnings.length > 0
          ? `${discovered.warnings.length} registry asset${discovered.warnings.length === 1 ? " was" : "s were"} hidden because validation failed.`
          : "")
      } catch (error) {
        if (request === ethereumConnectionRequest.current) {
          setAssetWarning(`ERC-20 assets unavailable: ${error instanceof Error ? error.message : String(error)}`)
        }
      } finally {
        if (request === ethereumConnectionRequest.current) setAssetsLoading(false)
      }
      return connectedClient
    },
    [config]
  )

  const connectEthereumWallet = useCallback(async () => {
    if (!config) return
    setBusy(true)
    setActionError("")
    try {
      const account = await connectEthereum(config)
      await setEthereumConnection(account)
      if (direction === "withdrawal") setRecipient(account)
      setToast(`Ethereum wallet connected to ${ethereum}.`)
    } catch (error) {
      setActionError(formatWalletError(error))
    } finally {
      setBusy(false)
    }
  }, [config, direction, ethereum, setEthereumConnection])

  const establishMinaSession = useCallback((
    wallet: MinaWalletKind,
    presentation: MinaSessionPresentation
  ): MinaSessionAttempt | undefined => {
    if (!config || minaConnectionPending.current) return
    const request = ++minaConnectionRequest.current
    const interactive = presentation.kind === "interactive"
    minaConnectionPending.current = true
    activeMinaSession.current = undefined
    minaWalletRef.current = wallet
    rememberMinaWallet(wallet)
    if (interactive) rememberMinaWalletConnection(wallet, false)
    setMinaWallet(wallet)
    setMinaConnectionBusy(true)
    if (interactive) setActionError("")

    const releasePendingConnection = () => {
      if (request === minaConnectionRequest.current && minaConnectionPending.current) {
        minaConnectionPending.current = false
        setMinaConnectionBusy(false)
      }
    }

    void connectMina(config, wallet)
      .then((account) => {
        if (request !== minaConnectionRequest.current || minaWalletRef.current !== wallet) return
        rememberMinaWallet(wallet)
        rememberMinaWalletConnection(wallet, true)
        activeMinaSession.current = wallet
        activityRequest.current += 1
        setZekoAccount(account)
        setZekoClient(undefined)
        setZekoBalance(undefined)
        setRecipient((current) => presentation.fillRecipient && !current ? account : current)
        if (interactive) {
          setMinaWalletOpen(false)
          setToast(`${minaWalletName(wallet)} connected to Zeko Testnet.`)
        }
        releasePendingConnection()
        void fetchZekoBalance(config.sequencerGraphqlUrl, account)
          .then((balance) => {
            if (request === minaConnectionRequest.current &&
                activeMinaSession.current === wallet) setZekoBalance(balance)
          })
          .catch(() => {
            if (request === minaConnectionRequest.current &&
                activeMinaSession.current === wallet) setZekoBalance("0")
          })
      })
      .catch((error: unknown) => {
        if (request === minaConnectionRequest.current) {
          rememberMinaWalletConnection(wallet, false)
          if (interactive) setActionError(formatWalletError(error))
        }
      })
      .finally(releasePendingConnection)

    return {
      cancel: () => {
        if (request === minaConnectionRequest.current) {
          minaConnectionRequest.current += 1
          minaConnectionPending.current = false
        }
      }
    }
  }, [config])

  const connectMinaWallet = useCallback((wallet: MinaWalletKind) => {
    establishMinaSession(wallet, {
      kind: "interactive",
      fillRecipient: direction === "deposit"
    })
  }, [direction, establishMinaSession])

  useEffect(() => {
    minaWalletRef.current = minaWallet
  }, [minaWallet])

  useEffect(() => {
    if (!ethereumAccount) return
    const provider = getEthereumProvider()
    const balance = asset.kind === "native"
      ? fetchEthereumBalance(provider, ethereumAccount)
      : fetchTokenBalance(provider, ethereumAccount, asset)
    void balance.then(setEthereumBalance).catch(() => setEthereumBalance("0"))
  }, [asset, ethereumAccount])

  useEffect(() => {
    if (!config || !zekoAccount) return
    const balance = asset.kind === "native"
      ? fetchZekoBalance(config.sequencerGraphqlUrl, zekoAccount)
      : fetchZekoTokenBalance(config.sequencerGraphqlUrl, zekoAccount, asset.tokenIdL2, asset.zekoDecimals)
    void balance.then(setZekoBalance).catch(() => setZekoBalance("0"))
  }, [asset, config, zekoAccount])

  useEffect(() => {
    if (!config) return
    const wallet = minaWalletRef.current
    if (!wasMinaWalletConnected(wallet)) return
    const attempt = establishMinaSession(wallet, {
      kind: "reload",
      fillRecipient: true
    })
    return attempt?.cancel
  }, [config, establishMinaSession])

  useEffect(() => {
    if (!config) return
    let active = true
    if (window.ethereum) {
      void window.ethereum
        .request({ method: "eth_accounts" })
        .then(async (accounts) => {
          const account = Array.isArray(accounts) && typeof accounts[0] === "string" ? accounts[0] : undefined
          const chain = Number.parseInt(String(await window.ethereum?.request({ method: "eth_chainId" })), 16)
          if (active && account && chain === config.expectedEthereumChainId) {
            await setEthereumConnection(getAddress(account))
          }
        })
        .catch(() => undefined)
    }
    return () => {
      active = false
    }
  }, [config, setEthereumConnection])

  useEffect(() => {
    if (!config) return
    const ethereum = window.ethereum
    let minaProvider
    try {
      minaProvider = getMinaProvider(minaWallet)
    } catch {
      minaProvider = undefined
    }
    const onEthereumAccounts = (value: unknown) => {
      const account = Array.isArray(value) && typeof value[0] === "string" ? value[0] : undefined
      activityRequest.current += 1
      setClient(undefined)
      setZekoClient(undefined)
      setActivityLoading(false)
      if (!account || !isAddress(account)) {
        ethereumConnectionRequest.current += 1
        setEthereumAccount(undefined)
        setEthereumBalance(undefined)
        return
      }
      void setEthereumConnection(getAddress(account)).catch((error: unknown) => {
        setActionError(formatWalletError(error))
      })
    }
    const onEthereumChain = (value: unknown) => {
      const chain = Number.parseInt(String(value), 16)
      setClient(undefined)
      setZekoClient(undefined)
      if (chain !== config.expectedEthereumChainId) {
        setActionError(`Ethereum wallet is on chain ${chain}; ${ethereum} ${config.expectedEthereumChainId} is required.`)
        return
      }
      setActionError("")
      void ethereum?.request({ method: "eth_accounts" }).then(onEthereumAccounts).catch(() => undefined)
    }
    const onAuroAccounts = (accounts: string[]) => {
      if (activeMinaSession.current !== minaWallet || minaWalletRef.current !== minaWallet) return
      const request = ++minaConnectionRequest.current
      activityRequest.current += 1
      setZekoClient(undefined)
      setZekoAccount(accounts[0])
      if (accounts[0]) {
        rememberMinaWallet(minaWallet)
        rememberMinaWalletConnection(minaWallet, true)
        void fetchZekoBalance(config.sequencerGraphqlUrl, accounts[0])
          .then((balance) => {
            if (request === minaConnectionRequest.current &&
                activeMinaSession.current === minaWallet) setZekoBalance(balance)
          })
          .catch(() => {
            if (request === minaConnectionRequest.current &&
                activeMinaSession.current === minaWallet) setZekoBalance("0")
          })
      } else {
        activeMinaSession.current = undefined
        rememberMinaWalletConnection(minaWallet, false)
        setZekoBalance(undefined)
      }
    }
    const onAuroChain = (network: { networkID: string }) => {
      if (activeMinaSession.current !== minaWallet || minaWalletRef.current !== minaWallet) return
      setZekoClient(undefined)
      if (!isAuroPoCNetwork(network.networkID)) {
        setActionError("The Mina wallet must use Zeko Testnet for this PoC's temporary testnet signing domain.")
      }
    }
    ethereum?.on?.("accountsChanged", onEthereumAccounts)
    ethereum?.on?.("chainChanged", onEthereumChain)
    minaProvider?.on?.("accountsChanged", onAuroAccounts)
    minaProvider?.on?.("chainChanged", onAuroChain)
    return () => {
      ethereum?.removeListener?.("accountsChanged", onEthereumAccounts)
      ethereum?.removeListener?.("chainChanged", onEthereumChain)
      minaProvider?.removeAllListeners?.()
    }
  }, [config, ethereum, minaWallet, setEthereumConnection])

  const clearMinaConnection = useCallback(() => {
    activityRequest.current += 1
    setZekoAccount(undefined)
    setZekoBalance(undefined)
    setZekoClient(undefined)
    setActivityLoading(false)
    setScreen("form")
    setCompletion(undefined)
    setRecipient((current) => direction === "deposit" && current === zekoAccount ? "" : current)
  }, [direction, zekoAccount])

  const selectMinaWallet = async (wallet: MinaWalletKind) => {
    if (minaConnectionPending.current || wallet === minaWallet) return
    const previousWallet = minaWallet
    const shouldRevoke = previousWallet === "metamask-snap" &&
      (activeMinaSession.current === previousWallet ||
        Boolean(zekoAccount) ||
        wasMinaWalletConnected(previousWallet))
    const request = ++minaConnectionRequest.current
    activeMinaSession.current = undefined
    rememberMinaWalletConnection(previousWallet, false)
    setBusy(true)
    let revokeError = ""
    if (shouldRevoke) {
      try {
        await getMinaProvider(previousWallet).revokePermissions?.()
      } catch (error) {
        revokeError = formatWalletError(error)
      }
    }
    if (request !== minaConnectionRequest.current) return
    minaWalletRef.current = wallet
    rememberMinaWallet(wallet)
    setMinaWallet(wallet)
    clearMinaConnection()
    setBusy(false)
    setActionError(revokeError
      ? `The bridge disconnected locally, but the wallet permission could not be revoked: ${revokeError}`
      : "")
    setToast(`${minaWalletName(wallet)} selected. Connect it to continue.`)
  }

  const disconnectMinaWallet = useCallback(async () => {
    if (minaConnectionPending.current) return
    ++minaConnectionRequest.current
    activeMinaSession.current = undefined
    rememberMinaWalletConnection(minaWallet, false)
    setBusy(true)
    setActionError("")
    let revokeError = ""
    try {
      const provider = getMinaProvider(minaWallet)
      if (minaWallet === "metamask-snap") await provider.revokePermissions?.()
    } catch (error) {
      revokeError = formatWalletError(error)
    } finally {
      clearMinaConnection()
      setMinaWalletOpen(false)
      setBusy(false)
      setToast("Mina wallet disconnected from the bridge.")
      if (revokeError) setActionError(`The bridge disconnected locally, but the wallet permission could not be revoked: ${revokeError}`)
    }
  }, [clearMinaConnection, minaWallet])

  const minaWalletBusy = busy || minaConnectionBusy

  useEffect(() => {
    if (!recipient) {
      setRecipientValid(false)
      return
    }
    if (direction === "withdrawal") {
      setRecipientValid(isAddress(recipient))
      return
    }
    let active = true
    const timer = window.setTimeout(() => {
      void isValidZekoAddress(recipient).then((valid) => active && setRecipientValid(valid))
    }, 120)
    return () => {
      active = false
      window.clearTimeout(timer)
    }
  }, [direction, recipient])

  const amountResult = useMemo(() => {
    if (!amount || !config) return undefined
    try {
      return asset.kind === "native"
        ? bridgeAmountFromEth(amount)
        : bridgeAmountFromToken(amount, asset.zekoDecimals)
    } catch {
      return undefined
    }
  }, [amount, asset, config])

  const formValid = Boolean(
    amountResult &&
      recipientValid &&
      (direction === "deposit" ? ethereumAccount : zekoAccount)
  )

  const validateForm = async (): Promise<boolean> => {
    if (!config) return false
    try {
      if (asset.kind === "native") bridgeAmountFromEth(amount)
      else bridgeAmountFromToken(amount, asset.zekoDecimals)
    } catch (error) {
      setValidation(error instanceof Error ? error.message : String(error))
      return false
    }
    const validRecipient = direction === "deposit" ? await isValidZekoAddress(recipient) : isAddress(recipient)
    if (!validRecipient) {
      setValidation(`Enter a valid ${direction === "deposit" ? "B62 Zeko" : "Ethereum"} recipient.`)
      return false
    }
    if (direction === "deposit" && !ethereumAccount) {
      setValidation(missingWalletMessage("ethereum", ethereum))
      return false
    }
    if (direction === "withdrawal" && !zekoAccount) {
      setValidation(missingWalletMessage("auro"))
      return false
    }
    setValidation("")
    return true
  }

  const ensureClient = async (): Promise<EthereumBridgeClient> => {
    if (client) return client
    if (!config) throw new Error("Runtime configuration is not loaded")
    const account = ethereumAccount ?? (await connectEthereum(config))
    return setEthereumConnection(account)
  }

  const ensureFullClient = async (): Promise<EthereumBridgeClient> => {
    if (zekoClient) return zekoClient
    if (!config) throw new Error("Runtime configuration is not loaded")
    const base = await ensureClient()
    const full = await createEthereumBridgeClient({
      config,
      provider: getEthereumProvider(),
      account: base.account,
      withZeko: true,
      assetSnapshot
    })
    setZekoClient(full)
    return full
  }

  const operationKey = (operationRecipient: string, submittingBridgeAddress?: string) => {
    const effectiveBridgeAddress = submittingBridgeAddress
      || bridgeAddress
      || client?.config.bridgeAddress
      || zekoClient?.config.bridgeAddress
    if (!config || !effectiveBridgeAddress) throw new Error("Bridge configuration is not loaded")
    return operationStorageKey(config.expectedEthereumChainId, effectiveBridgeAddress, operationRecipient)
  }

  const rememberOperation = (operation: PendingOperation, submittingBridgeAddress?: string) => {
    const key = operationKey(operation.recipient, submittingBridgeAddress)
    upsertOperation(key, operation)
    setOperations((current) => [operation, ...current.filter((row) => row.id !== operation.id)])
  }

  const refreshActivity = useCallback(async () => {
    if (!client || (!zekoAccount && !ethereumAccount)) return
    const refreshKey = `${client.account}:${ethereumAccount ?? ""}:${zekoAccount ?? ""}`
    if (activityRefreshes.current.has(refreshKey)) return
    activityRefreshes.current.add(refreshKey)
    const request = ++activityRequest.current
    setActivityLoading(true)
    try {
      let activityClient = client
      if (zekoAccount && config && ethereumAccount) {
        activityClient = zekoClient ?? await createEthereumBridgeClient({
          config,
          provider: getEthereumProvider(),
          account: ethereumAccount,
          withZeko: true
        })
        if (!zekoClient) setZekoClient(activityClient)
      }
      const result = await listWalletActivity({
        client: activityClient,
        zekoRecipient: zekoAccount,
        ethereumRecipient: ethereumAccount
      })
      const recoveryErrors: string[] = []
      const tokenWithdrawals: TokenWithdrawalProof[] = []
      const discoveredOperations: PendingOperation[] = result.withdrawalRequests.map((row) =>
        recoverWithdrawalOperation(row, assets, archiveTimestamp(row.timestamp)))
      if (ethereumAccount) {
        const responses = await Promise.allSettled([
          listTokenWithdrawals({ gatewayUrl: config?.gatewayUrl ?? "", recipient: ethereumAccount }),
          listTokenWithdrawalRequests({ gatewayUrl: config?.gatewayUrl ?? "", recipient: ethereumAccount })
        ])
        const [proofs, requests] = responses
        if (proofs.status === "fulfilled") tokenWithdrawals.push(...proofs.value)
        if (requests.status === "fulfilled") {
          for (const row of requests.value) {
            try {
              discoveredOperations.push(recoverWithdrawalOperation(row, assets, archiveTimestamp(row.timestamp)))
            } catch (error) {
              recoveryErrors.push(error instanceof Error ? error.message : String(error))
            }
          }
        }
        for (const response of responses) {
          if (response.status === "rejected") recoveryErrors.push(`Token activity recovery failed: ${String(response.reason)}`)
        }
      }
      const allWithdrawals: Array<WithdrawalProof | TokenWithdrawalProof> = [
        ...result.withdrawals,
        ...tokenWithdrawals
      ]
      if (request !== activityRequest.current) return
      setActivityError(recoveryErrors.join("; "))
      setDeposits(result.deposits)
      setWithdrawals(allWithdrawals)
      const recovered = currentActivityOperations(config, bridgeAddress, zekoAccount, ethereumAccount)
      const discoveredByTransaction = new Map(
        discoveredOperations.map((operation) => [operation.transactionHash, operation])
      )
      const enrichedRecovered = recovered.map((operation) => {
        const discovered = discoveredByTransaction.get(operation.transactionHash)
        if (!discovered) return operation
        const enriched = { ...operation, amount: discovered.amount, asset: discovered.asset, globalActionIndex: discovered.globalActionIndex }
        upsertOperation(operationKey(operation.recipient), enriched)
        return enriched
      })
      const nextOperations = [
        ...new Map(
          [...discoveredOperations, ...enrichedRecovered].map((operation) => [operation.id, operation])
        ).values()
      ]
      setOperations(nextOperations)
      const selected = selectedOperationRef.current
      setSelectedDeposit((current) => current
        ? result.deposits.find((row) => row.nonce === current.nonce)
        : undefined)
      setSelectedWithdrawal((current) => current
        ? allWithdrawals.find(
            (row) => row.globalActionIndex === current.globalActionIndex
          )
        : selected?.globalActionIndex === undefined
          ? undefined
          : allWithdrawals.find(
              (row) => row.globalActionIndex === selected.globalActionIndex
            ))
      setSelectedOperation((current) => {
        const next = current
          ? nextOperations.find(
            (operation) =>
              operation.id === current.id ||
              (operation.globalActionIndex !== undefined &&
                operation.globalActionIndex === current.globalActionIndex)
          )
          : undefined
        selectedOperationRef.current = next
        return next
      })
    } catch (error) {
      if (request === activityRequest.current) {
        setActivityError(error instanceof Error ? error.message : String(error))
      }
    } finally {
      activityRefreshes.current.delete(refreshKey)
      if (request === activityRequest.current) setActivityLoading(false)
    }
  }, [assets, bridgeAddress, client, config, ethereumAccount, zekoAccount, zekoClient])

  useEffect(() => {
    if (!config || !client) return
    void refreshActivity()
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void refreshActivity()
    }, config.pollIntervalMs)
    const onVisibility = () => document.visibilityState === "visible" && void refreshActivity()
    document.addEventListener("visibilitychange", onVisibility)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener("visibilitychange", onVisibility)
    }
  }, [client, config, refreshActivity])

  useEffect(() => {
    if (!config || !bridgeAddress) return
    setActivityLoading(false)
    setActivityError("")
    setDeposits([])
    setWithdrawals([])
    setOperations(currentActivityOperations(config, bridgeAddress, zekoAccount, ethereumAccount))
    setSelectedDeposit(undefined)
    setSelectedWithdrawal(undefined)
    setSelectedOperation(undefined)
  }, [bridgeAddress, config, ethereumAccount, zekoAccount])

  useEffect(() => {
    selectedOperationRef.current = selectedOperation
  }, [selectedOperation])

  useEffect(() => {
    if (!toast) return
    const timer = window.setTimeout(() => setToast(""), 3200)
    return () => window.clearTimeout(timer)
  }, [toast])

  const submitTransfer = async () => {
    if (!config || !amountResult) return
    setBusy(true)
    setActionError("")
    try {
      if (direction === "deposit") {
        const base = asset.kind === "native" ? await ensureClient() : await ensureFullClient()
        await ensureEthereumNetwork(getEthereumProvider(), config.expectedEthereumChainId)
        const result = asset.kind === "native"
          ? await depositNative({
              client: base,
              recipient,
              valueWei: bridgeAmountFromEth(amount).valueWei
            })
          : await (async () => {
              const depositAmount = bridgeAmountFromToken(amount, asset.zekoDecimals).ethereumAmount
              await ensureTokenAllowance({
                provider: getEthereumProvider(),
                account: base.account,
                token: asset.token,
                spender: getAddress(base.config.bridgeAddress),
                amount: depositAmount
              })
              return depositToken({ client: base, token: asset.token, recipient, amount: depositAmount })
            })()
        const operation: PendingOperation = {
          id: `deposit:${result.nonce}`,
          direction: "deposit",
          amount,
          recipient,
          transactionHash: result.hash,
          depositNonce: result.nonce,
          createdAt: new Date().toISOString(),
          asset: asset.kind === "erc20"
            ? { kind: "erc20", token: asset.token, assetId: asset.assetId, symbol: asset.symbol, decimals: asset.zekoDecimals }
            : undefined
        }
        rememberOperation(operation, base.config.bridgeAddress)
        setSelectedOperation(operation)
        setSelectedDeposit(result.deposit)
        setScreen("deposit-progress")
        setToast(`${asset.symbol} locked on ${ethereum}. Gateway tracking has started.`)
      } else {
        if (!zekoAccount) throw new Error(missingWalletMessage("auro"))
        const provider = getMinaProvider(minaWallet)
        await ensureAuroPoCNetwork(provider, config)
        const full = await ensureFullClient()
        const hash = asset.kind === "native"
          ? await requestNativeWithdrawal({
              client: full,
              sender: zekoAccount,
              recipient: getAddress(recipient),
              amount: amountResult.zekoAmount,
              config,
              provider
            })
          : await requestTokenWithdrawal({
              client: full,
              sender: zekoAccount,
              recipient: getAddress(recipient),
              token: asset.token,
              amount: amountResult.zekoAmount,
              config,
              provider
            })
        const operation: PendingOperation = {
          id: `withdrawal:${hash}`,
          direction: "withdrawal",
          amount,
          recipient: getAddress(recipient),
          transactionHash: hash,
          createdAt: new Date().toISOString(),
          asset: asset.kind === "erc20"
            ? { kind: "erc20", token: asset.token, assetId: asset.assetId, symbol: asset.symbol, decimals: asset.zekoDecimals }
            : undefined
        }
        rememberOperation(operation, full.config.bridgeAddress)
        setSelectedOperation(operation)
        setSelectedWithdrawal(undefined)
        setScreen("withdrawal-progress")
        setToast("Withdrawal request accepted by the Zeko sequencer.")
        void refreshActivity()
      }
    } catch (error) {
      setActionError(formatWalletError(error))
    } finally {
      setBusy(false)
    }
  }

  const finalizeSelectedDeposit = async () => {
    if (!config || !selectedDeposit) return
    setBusy(true)
    setActionError("")
    try {
      if (!zekoAccount) throw new Error(missingWalletMessage("auro"))
      const full = await ensureFullClient()
      const depositRecipient = selectedOperation?.direction === "deposit"
        ? selectedOperation.recipient
        : zekoAccount ?? recipient
      const depositAsset = selectedDeposit.assetId
        ? assets.find((candidate) => candidate.kind === "erc20" && candidate.token.toLowerCase() === selectedDeposit.token.toLowerCase())
        : NATIVE_ASSET
      if (!depositAsset) throw new Error(`Deposit token ${selectedDeposit.token} is not in the authenticated asset registry`)
      const hash = depositAsset.kind === "native"
        ? await finalizeDeposit({
            client: full,
            recipient: recipientForDeposit(selectedDeposit, depositRecipient),
            config,
            provider: getMinaProvider(minaWallet)
          })
        : await finalizeTokenDeposit({
            client: full,
            token: depositAsset.token,
            recipient: recipientForDeposit(selectedDeposit, depositRecipient),
            encodingVersion: selectedDeposit.encodingVersion ?? undefined,
            config,
            provider: getMinaProvider(minaWallet)
          })
      const operation = selectedOperation?.direction === "deposit" ? { ...selectedOperation, zekoTransactionHash: hash } : undefined
      if (operation) rememberOperation(operation)
      setCompletion({ direction: "deposit", amount: operation?.amount ?? formatUnits(BigInt(selectedDeposit.zekoAmount), depositAsset.zekoDecimals, depositAsset.zekoDecimals), symbol: depositAsset.symbol, hash, url: zekoTransactionUrl(config, hash) })
      setScreen("complete")
    } catch (error) {
      setActionError(formatWalletError(error))
    } finally {
      setBusy(false)
    }
  }

  const claimSelectedWithdrawal = async () => {
    if (!config || !selectedWithdrawal) return
    setBusy(true)
    setActionError("")
    try {
      const withdrawalAsset = activityAsset(assets,
        "token" in selectedWithdrawal ? selectedWithdrawal.token : undefined,
        "assetId" in selectedWithdrawal ? selectedWithdrawal.assetId : undefined)
      if (!withdrawalAsset) throw new Error("Withdrawal asset is not authenticated and active")
      const base = await ensureClient()
      await ensureEthereumNetwork(getEthereumProvider(), config.expectedEthereumChainId)
      const hash = "token" in selectedWithdrawal
        ? await base.claimTokenWithdrawal(selectedWithdrawal)
        : await base.claimNativeWithdrawal(selectedWithdrawal)
      const operation = selectedOperation?.direction === "withdrawal" ? { ...selectedOperation, ethereumClaimHash: hash } : undefined
      if (operation) rememberOperation(operation)
      const decimals = withdrawalAsset.zekoDecimals
      const symbol = withdrawalAsset.symbol
      setCompletion({ direction: "withdrawal", amount: operation?.amount ?? formatUnits(BigInt(selectedWithdrawal.amount), decimals, decimals), symbol, hash, url: ethereumTransactionUrl(config, hash) })
      setScreen("complete")
      void refreshActivity()
    } catch (error) {
      setActionError(formatWalletError(error))
    } finally {
      setBusy(false)
    }
  }

  const swapDirection = () => {
    const next = direction === "deposit" ? "withdrawal" : "deposit"
    setDirection(next)
    setAmount("")
    setRecipient(next === "deposit" ? zekoAccount ?? "" : ethereumAccount ?? "")
    setValidation("")
    setScreen("form")
  }

  const openActivity = () => {
    setTab("activity")
    void refreshActivity()
  }

  if (configError) {
    return <main className="fatal-state"><h1>Bridge configuration unavailable</h1><p>{configError}</p><button type="button" onClick={() => window.location.reload()}>Retry</button></main>
  }
  if (!config) return <main className="loading-state"><span className="loading-mark">Z</span><p>Loading bridge configuration…</p></main>

  const selectedDepositAsset = selectedDeposit?.assetId
    ? assets.find((candidate) => candidate.kind === "erc20" && candidate.token.toLowerCase() === selectedDeposit.token.toLowerCase())
    : NATIVE_ASSET
  const selectedOperationToken = selectedOperation?.asset?.kind === "erc20"
    ? selectedOperation.asset.token
    : undefined
  const selectedOperationAsset = selectedOperationToken
    ? assets.find((candidate) => candidate.kind === "erc20" && candidate.token.toLowerCase() === selectedOperationToken.toLowerCase())
    : NATIVE_ASSET
  let bridgeContent
  if (screen === "review") {
    bridgeContent = <ReviewView direction={direction} amount={amount} recipient={recipient} asset={asset} config={config} busy={busy} onBack={() => setScreen("form")} onConfirm={submitTransfer} />
  } else if (screen === "deposit-progress" && selectedDeposit) {
    bridgeContent = <DepositProgress deposit={selectedDeposit} symbol={selectedDepositAsset?.symbol} decimals={selectedDepositAsset?.zekoDecimals} ethereumTransactionUrl={ethereumTransactionUrl(config, selectedDeposit.ethereumTransactionHash)} onFinalize={finalizeSelectedDeposit} busy={busy} />
  } else if (screen === "withdrawal-progress" && selectedOperation) {
    bridgeContent = <WithdrawalProgress withdrawal={selectedWithdrawal} amount={selectedOperation.amount} symbol={selectedOperationAsset?.symbol ?? selectedOperation.asset?.symbol} transactionHash={selectedOperation.transactionHash} zekoTransactionUrl={selectedOperation.transactionHash.startsWith("Gateway-") ? undefined : zekoTransactionUrl(config, selectedOperation.transactionHash)} onClaim={claimSelectedWithdrawal} busy={busy} />
  } else if (screen === "complete" && completion) {
    bridgeContent = <CompleteView {...completion} onActivity={openActivity} onNewTransfer={() => { setAmount(""); setScreen("form"); setCompletion(undefined) }} />
  } else {
    bridgeContent = (
      <BridgeForm
        direction={direction}
        amount={amount}
        recipient={recipient}
        ethereumAccount={ethereumAccount}
        zekoAccount={zekoAccount}
        ethereumBalance={ethereumBalance}
        zekoBalance={zekoBalance}
        config={config}
        validation={validation}
        canReview={formValid}
        showDetails={showDetails}
        assets={assets}
        asset={asset}
        assetsLoading={assetsLoading}
        onAssetChange={(id) => {
          const selected = assets.find((candidate) => candidate.id.toLowerCase() === id.toLowerCase())
          if (!selected) return
          setAsset(selected)
          setAmount("")
          setValidation("")
        }}
        onAmountChange={(value) => { setAmount(value); setValidation("") }}
        onRecipientChange={(value) => { setRecipient(value); setValidation("") }}
        onSwap={swapDirection}
        onReview={() => void validateForm().then((valid) => valid && setScreen("review"))}
        onToggleDetails={() => setShowDetails((value) => !value)}
      />
    )
  }

  return (
    <div className="app-shell">
      <BackgroundWave className="contour-desktop" src="/assets/zeko-contours.webm" storageKey="zeko-wave-desktop-time" />
      <BackgroundWave className="contour-mobile" src="/assets/zeko-contours-mobile.webm" storageKey="zeko-wave-mobile-time" />
      <div className="background-wash"></div>
      <header className="site-header">
        <button type="button" className="brand-link" onClick={() => { setTab("bridge"); setScreen("form") }} aria-label="Zeko bridge home"><img src="/assets/zeko-logo.svg" alt="Zeko" /></button>
        <div className="network-health"><span className="health-dot"></span><span>{ethereum} · Experimental</span></div>
        <div className="header-actions">
          <WalletChip network="ethereum" ethereumNetworkName={ethereum} account={ethereumAccount} balance={ethereumBalance} symbol={asset.symbol} onClick={() => void connectEthereumWallet()} />
          <WalletChip network="zeko" minaWallet={minaWallet} account={zekoAccount} balance={zekoBalance} symbol={asset.symbol} onClick={() => { setActionError(""); setMinaWalletOpen(true) }} />
        </div>
      </header>
      <main className="main-content">
        <div className="hero"><p className="eyebrow">Ethereum settlement</p><h1>Ethereum ↔ Zeko Bridge</h1><p className="hero-copy">Move native ETH and registered ERC-20 assets between Ethereum custody and Zeko execution, with every transition proven through SP1 and anchored to settlement state.</p></div>
        <div className="environment-banner"><strong>Experimental PoC</strong><span>{ethereum}</span><span>·</span><span>No cancellation/refund</span><span>·</span><span>Zeko signs as temporary <code>testnet</code></span></div>
        <div className="bridge-card">
          <div className="card-header">
            <div className="tabs" role="tablist" aria-label="Bridge navigation"><button type="button" className={`tab${tab === "bridge" ? " active" : ""}`} role="tab" aria-selected={tab === "bridge"} onClick={() => setTab("bridge")}>Bridge</button><button type="button" className={`tab${tab === "wallet" ? " active" : ""}`} role="tab" aria-selected={tab === "wallet"} onClick={() => setTab("wallet")}>Wallet</button><button type="button" className={`tab${tab === "activity" ? " active" : ""}`} role="tab" aria-selected={tab === "activity"} onClick={openActivity}>Activity</button></div>
            <button type="button" className="icon-button" onClick={() => setSettingsOpen(true)} aria-label="Open bridge settings"><span className="settings-glyph">⚙︎</span></button>
          </div>
          <div className="card-body">
            {actionError && <Notice kind="error">{actionError}</Notice>}
            {assetWarning && <Notice kind="warning">{assetWarning}</Notice>}
            {tab === "activity" && activityError && <Notice kind="error">{activityError}</Notice>}
            {tab === "activity" ? <ActivityView deposits={deposits} withdrawals={withdrawals} operations={operations} assets={assets} loading={activityLoading} onDeposit={(deposit) => { setSelectedDeposit(deposit); setSelectedOperation(operations.find((row) => row.direction === "deposit" && row.depositNonce === deposit.nonce)); setScreen("deposit-progress"); setTab("bridge") }} onWithdrawal={(withdrawal, operation) => { setSelectedWithdrawal(withdrawal); setSelectedOperation(operation); setScreen("withdrawal-progress"); setTab("bridge") }} /> : tab === "wallet" ? <WalletView config={config} getProvider={() => getMinaProvider(minaWallet)} account={zekoAccount} onConnect={() => { setActionError(""); setMinaWalletOpen(true) }} onSubmitted={(hash, kind) => { setToast(`${kind} payment submitted: ${hash}`); void fetchZekoBalance(config.sequencerGraphqlUrl, zekoAccount ?? "").then(setZekoBalance).catch(() => undefined) }} /> : bridgeContent}
          </div>
          <div className="card-footnote"><span className="footnote-proof">SP1</span><span>verifies the Zeko state transition</span><span>·</span><span>Ethereum verifies settlement</span></div>
        </div>
        <footer className="page-footer"><span><span className="health-dot"></span>{client ? "Gateway connected" : "Connect wallet to verify gateway"}</span><span>{bridgeAddress ? `Bridge ${shortAddress(bridgeAddress)}` : "Bridge address from gateway"}</span><span>Proof of concept</span></footer>
      </main>
      {settingsOpen && <SettingsModal config={config} minaWallet={minaWallet} minaWalletBusy={minaWalletBusy} showDetails={showDetails} onMinaWalletChange={selectMinaWallet} onToggleDetails={() => setShowDetails((value) => !value)} onClose={() => setSettingsOpen(false)} />}
      {minaWalletOpen && <MinaWalletModal account={zekoAccount} minaWallet={minaWallet} busy={minaWalletBusy} error={actionError} onConnect={(wallet) => void connectMinaWallet(wallet)} onDisconnect={() => void disconnectMinaWallet()} onClose={() => setMinaWalletOpen(false)} />}
      {toast && <div className="toast" role="status"><span className="toast-dot"></span>{toast}</div>}
    </div>
  )
}

const recipientForDeposit = (deposit: DepositStatus, fallback: string): string => {
  const operationRecipient = fallback
  if (!operationRecipient) throw new Error(`Deposit ${deposit.nonce} has no locally recovered B62 recipient`)
  return operationRecipient
}

const archiveTimestamp = (value: string): string => {
  const milliseconds = Number(value)
  if (Number.isFinite(milliseconds) && milliseconds >= 0) {
    return new Date(milliseconds).toISOString()
  }
  const date = new Date(value)
  return Number.isNaN(date.valueOf()) ? new Date(0).toISOString() : date.toISOString()
}

const currentActivityOperations = (
  config: RuntimeConfig | undefined,
  bridgeAddress: string,
  zekoAccount: string | undefined,
  ethereumAccount: Address | undefined
): PendingOperation[] => {
  if (!config || !bridgeAddress) return []
  const deposits = zekoAccount
    ? readOperations(operationStorageKey(config.expectedEthereumChainId, bridgeAddress, zekoAccount))
        .filter((operation) => operation.direction === "deposit")
    : []
  const withdrawals = ethereumAccount
    ? readOperations(operationStorageKey(config.expectedEthereumChainId, bridgeAddress, ethereumAccount))
        .filter((operation) => operation.direction === "withdrawal")
    : []
  return [...new Map([...deposits, ...withdrawals].map((operation) => [operation.id, operation])).values()]
}
