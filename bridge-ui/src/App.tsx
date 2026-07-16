import type { DepositStatus, EthereumBridgeClient, WithdrawalProof } from "@zeko-labs/eth-bridge-sdk"
import type { Address } from "viem"
import { getAddress, isAddress } from "viem"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { ActivityView } from "./components/ActivityView"
import { BackgroundWave, DepositProgress, type Direction, Notice, WalletChip, WithdrawalProgress } from "./components/BridgeUi"
import { BridgeForm } from "./components/BridgeForm"
import { CompleteView } from "./components/CompleteView"
import { ReviewView } from "./components/ReviewView"
import { SettingsModal } from "./components/SettingsModal"
import { bridgeAmountFromEth, formatUnits } from "./lib/amount"
import {
  createEthereumBridgeClient,
  depositNative,
  ethereumTransactionUrl,
  fetchEthereumBalance,
  fetchZekoBalance,
  finalizeDeposit,
  isValidZekoAddress,
  listWalletActivity,
  loadBridgeModules,
  requestNativeWithdrawal,
  zekoTransactionUrl
} from "./lib/bridge"
import { ethereumNetworkName, loadRuntimeConfig, type RuntimeConfig } from "./lib/config"
import {
  operationStorageKey,
  readOperations,
  rememberAuroConnection,
  type PendingOperation,
  upsertOperation,
  wasAuroConnected
} from "./lib/storage"
import {
  connectAuro,
  connectEthereum,
  ensureAuroPoCNetwork,
  ensureEthereumNetwork,
  formatWalletError,
  getAuroProvider,
  getEthereumProvider,
  isAuroPoCNetwork,
  shortAddress
} from "./lib/wallets"

type Screen = "form" | "review" | "deposit-progress" | "withdrawal-progress" | "complete"
type Completion = { direction: Direction; amount: string; hash: string; url: string }

const missingWalletMessage = (wallet: "ethereum" | "auro", ethereum = "Sepolia") =>
  wallet === "ethereum"
    ? `Connect an Ethereum wallet on ${ethereum} before continuing.`
    : "Connect Auro to Zeko Testnet before continuing."

export default function App() {
  const [config, setConfig] = useState<RuntimeConfig>()
  const [configError, setConfigError] = useState("")
  const [sdkReady, setSdkReady] = useState(false)
  const [ethereumAccount, setEthereumAccount] = useState<Address>()
  const [zekoAccount, setZekoAccount] = useState<string>()
  const [ethereumBalance, setEthereumBalance] = useState<string>()
  const [zekoBalance, setZekoBalance] = useState<string>()
  const [client, setClient] = useState<EthereumBridgeClient>()
  const [zekoClient, setZekoClient] = useState<EthereumBridgeClient>()
  const [bridgeAddress, setBridgeAddress] = useState("")
  const [direction, setDirection] = useState<Direction>("deposit")
  const [tab, setTab] = useState<"bridge" | "activity">("bridge")
  const [screen, setScreen] = useState<Screen>("form")
  const [amount, setAmount] = useState("")
  const [recipient, setRecipient] = useState("")
  const [validation, setValidation] = useState("")
  const [recipientValid, setRecipientValid] = useState(false)
  const [showDetails, setShowDetails] = useState(true)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [activityLoading, setActivityLoading] = useState(false)
  const [deposits, setDeposits] = useState<DepositStatus[]>([])
  const [withdrawals, setWithdrawals] = useState<WithdrawalProof[]>([])
  const [operations, setOperations] = useState<PendingOperation[]>([])
  const [selectedDeposit, setSelectedDeposit] = useState<DepositStatus>()
  const [selectedWithdrawal, setSelectedWithdrawal] = useState<WithdrawalProof>()
  const [selectedOperation, setSelectedOperation] = useState<PendingOperation>()
  const [completion, setCompletion] = useState<Completion>()
  const [toast, setToast] = useState("")
  const [actionError, setActionError] = useState("")
  const ethereum = config ? ethereumNetworkName(config.expectedEthereumChainId) : "Sepolia"
  const activityRunning = useRef(false)

  useEffect(() => {
    let active = true
    void loadRuntimeConfig()
      .then((loaded) => active && setConfig(loaded))
      .catch((error: unknown) => active && setConfigError(error instanceof Error ? error.message : String(error)))
    return () => {
      active = false
    }
  }, [])

  useEffect(() => {
    if (!config) return
    let active = true
    void loadBridgeModules()
      .then(() => active && setSdkReady(true))
      .catch((error: unknown) => active && setActionError(error instanceof Error ? error.message : String(error)))
    return () => {
      active = false
    }
  }, [config])

  const setEthereumConnection = useCallback(
    async (account: Address) => {
      if (!config) throw new Error("Runtime configuration is not loaded")
      const provider = getEthereumProvider()
      await ensureEthereumNetwork(provider, config.expectedEthereumChainId)
      const connectedClient = await createEthereumBridgeClient({ config, provider, account })
      setEthereumAccount(account)
      setClient(connectedClient)
      setZekoClient(undefined)
      setBridgeAddress(connectedClient.config.bridgeAddress)
      setEthereumBalance(await fetchEthereumBalance(provider, account).catch(() => "0"))
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

  const connectAuroWallet = useCallback(async () => {
    if (!config) return
    setBusy(true)
    setActionError("")
    try {
      const account = await connectAuro(config)
      rememberAuroConnection(true)
      setZekoAccount(account)
      setZekoClient(undefined)
      setZekoBalance(await fetchZekoBalance(config.sequencerGraphqlUrl, account).catch(() => "0"))
      setRecipient((current) => direction === "deposit" && !current ? account : current)
      setToast("Auro connected to Zeko Testnet with the temporary testnet signing domain.")
    } catch (error) {
      setActionError(formatWalletError(error))
    } finally {
      setBusy(false)
    }
  }, [config, direction])

  useEffect(() => {
    if (!config || !window.mina || !wasAuroConnected()) return
    let active = true
    void connectAuro(config)
      .then(async (account) => {
        if (!active) return
        setZekoAccount(account)
        setZekoBalance(await fetchZekoBalance(config.sequencerGraphqlUrl, account).catch(() => "0"))
        setRecipient((current) => current || account)
      })
      .catch(() => {
        if (active) rememberAuroConnection(false)
      })
    return () => {
      active = false
    }
  }, [config])

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
    const auro = window.mina
    const onEthereumAccounts = (value: unknown) => {
      const account = Array.isArray(value) && typeof value[0] === "string" ? value[0] : undefined
      setClient(undefined)
      setZekoClient(undefined)
      setActivityLoading(false)
      if (!account || !isAddress(account)) {
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
      setZekoClient(undefined)
      setZekoAccount(accounts[0])
      if (accounts[0]) {
        rememberAuroConnection(true)
        void fetchZekoBalance(config.sequencerGraphqlUrl, accounts[0]).then(setZekoBalance).catch(() => setZekoBalance("0"))
      } else {
        rememberAuroConnection(false)
        setZekoBalance(undefined)
      }
    }
    const onAuroChain = (network: { networkID: string }) => {
      setZekoClient(undefined)
      if (!isAuroPoCNetwork(network.networkID)) {
        setActionError("Auro must use Zeko Testnet for this PoC's temporary testnet signing domain.")
      }
    }
    ethereum?.on?.("accountsChanged", onEthereumAccounts)
    ethereum?.on?.("chainChanged", onEthereumChain)
    auro?.on?.("accountsChanged", onAuroAccounts)
    auro?.on?.("chainChanged", onAuroChain)
    return () => {
      ethereum?.removeListener?.("accountsChanged", onEthereumAccounts)
      ethereum?.removeListener?.("chainChanged", onEthereumChain)
      auro?.removeAllListeners?.()
    }
  }, [config, ethereum, setEthereumConnection])

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
      return bridgeAmountFromEth(amount)
    } catch {
      return undefined
    }
  }, [amount, config, direction])

  const formValid = Boolean(
    amountResult &&
      recipientValid &&
      (direction === "deposit" ? ethereumAccount : zekoAccount) &&
      sdkReady
  )

  const validateForm = async (): Promise<boolean> => {
    if (!config) return false
    try {
      bridgeAmountFromEth(amount)
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
    if (!zekoAccount) throw new Error(missingWalletMessage("auro"))
    const base = await ensureClient()
    const full = await createEthereumBridgeClient({
      config,
      provider: getEthereumProvider(),
      account: base.account,
      withZeko: true
    })
    setZekoClient(full)
    return full
  }

  const operationKey = (operationRecipient: string) => {
    if (!config || !bridgeAddress) throw new Error("Bridge configuration is not loaded")
    return operationStorageKey(config.expectedEthereumChainId, bridgeAddress, operationRecipient)
  }

  const rememberOperation = (operation: PendingOperation) => {
    const key = operationKey(operation.recipient)
    upsertOperation(key, operation)
    setOperations((current) => [operation, ...current.filter((row) => row.id !== operation.id)])
  }

  const refreshActivity = useCallback(async () => {
    if (!client || activityRunning.current || (!zekoAccount && !ethereumAccount)) return
    activityRunning.current = true
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
      setDeposits(result.deposits)
      setWithdrawals(result.withdrawals)
      const discoveredOperations: PendingOperation[] = result.withdrawalRequests.map((request) => ({
        id: `withdrawal:${request.transactionHash}`,
        direction: "withdrawal",
        amount: formatUnits(BigInt(request.amount), 9, 9),
        recipient: request.recipient,
        transactionHash: request.transactionHash,
        createdAt: archiveTimestamp(request.timestamp)
      }))
      setOperations((current) => [
        ...new Map(
          [...discoveredOperations, ...current].map((operation) => [operation.id, operation])
        ).values()
      ])
      setSelectedDeposit((current) => current ? result.deposits.find((row) => row.nonce === current.nonce) ?? current : current)
      setSelectedWithdrawal((current) => current ? result.withdrawals.find((row) => row.settlementSequence === current.settlementSequence && row.offset === current.offset) ?? current : current)
      if (!selectedWithdrawal && selectedOperation?.direction === "withdrawal") {
        const matching = [...result.withdrawals].reverse().find(
          (row) =>
            row.recipient.toLowerCase() === selectedOperation.recipient.toLowerCase() &&
            formatUnits(BigInt(row.amount), 9, 9) === selectedOperation.amount
        )
        if (matching) setSelectedWithdrawal(matching)
      }
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error))
    } finally {
      activityRunning.current = false
      setActivityLoading(false)
    }
  }, [client, config, ethereumAccount, selectedOperation, selectedWithdrawal, zekoAccount, zekoClient])

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
    const recovered = [
      ...(zekoAccount ? readOperations(operationStorageKey(config.expectedEthereumChainId, bridgeAddress, zekoAccount)) : []),
      ...(ethereumAccount ? readOperations(operationStorageKey(config.expectedEthereumChainId, bridgeAddress, ethereumAccount)) : [])
    ]
    setOperations([...new Map(recovered.map((operation) => [operation.id, operation])).values()])
  }, [bridgeAddress, config, ethereumAccount, zekoAccount])

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
        const base = await ensureClient()
        await ensureEthereumNetwork(getEthereumProvider(), config.expectedEthereumChainId)
        const result = await depositNative({ client: base, recipient, valueWei: amountResult.valueWei })
        const operation: PendingOperation = {
          id: `deposit:${result.nonce}`,
          direction: "deposit",
          amount,
          recipient,
          transactionHash: result.hash,
          depositNonce: result.nonce,
          createdAt: new Date().toISOString()
        }
        rememberOperation(operation)
        setSelectedOperation(operation)
        setSelectedDeposit(result.deposit)
        setScreen("deposit-progress")
        setToast(`ETH locked on ${ethereum}. Gateway tracking has started.`)
      } else {
        if (!zekoAccount) throw new Error(missingWalletMessage("auro"))
        await ensureAuroPoCNetwork(getAuroProvider(), config)
        const full = await ensureFullClient()
        const hash = await requestNativeWithdrawal({
          client: full,
          sender: zekoAccount,
          recipient: getAddress(recipient),
          amount: amountResult.zekoAmount,
          config
        })
        const operation: PendingOperation = {
          id: `withdrawal:${hash}`,
          direction: "withdrawal",
          amount,
          recipient: getAddress(recipient),
          transactionHash: hash,
          createdAt: new Date().toISOString()
        }
        rememberOperation(operation)
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
      const hash = await finalizeDeposit({ client: full, recipient: recipientForDeposit(selectedDeposit, depositRecipient), config })
      const operation = selectedOperation?.direction === "deposit" ? { ...selectedOperation, zekoTransactionHash: hash } : undefined
      if (operation) rememberOperation(operation)
      setCompletion({ direction: "deposit", amount: operation?.amount ?? formatUnits(BigInt(selectedDeposit.zekoAmount), 9, 9), hash, url: zekoTransactionUrl(config, hash) })
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
      const base = await ensureClient()
      await ensureEthereumNetwork(getEthereumProvider(), config.expectedEthereumChainId)
      const hash = await base.claimNativeWithdrawal(selectedWithdrawal)
      const operation = selectedOperation?.direction === "withdrawal" ? { ...selectedOperation, ethereumClaimHash: hash } : undefined
      if (operation) rememberOperation(operation)
      setCompletion({ direction: "withdrawal", amount: operation?.amount ?? formatUnits(BigInt(selectedWithdrawal.amount), 9, 9), hash, url: ethereumTransactionUrl(config, hash) })
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

  let bridgeContent
  if (screen === "review") {
    bridgeContent = <ReviewView direction={direction} amount={amount} recipient={recipient} config={config} busy={busy} onBack={() => setScreen("form")} onConfirm={submitTransfer} />
  } else if (screen === "deposit-progress" && selectedDeposit) {
    bridgeContent = <DepositProgress deposit={selectedDeposit} ethereumTransactionUrl={ethereumTransactionUrl(config, selectedDeposit.ethereumTransactionHash)} onFinalize={finalizeSelectedDeposit} busy={busy} />
  } else if (screen === "withdrawal-progress" && selectedOperation) {
    bridgeContent = <WithdrawalProgress withdrawal={selectedWithdrawal} amount={selectedOperation.amount} transactionHash={selectedOperation.transactionHash} zekoTransactionUrl={selectedOperation.transactionHash.startsWith("Gateway-") ? undefined : zekoTransactionUrl(config, selectedOperation.transactionHash)} onClaim={claimSelectedWithdrawal} busy={busy} />
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
          <WalletChip network="ethereum" ethereumNetworkName={ethereum} account={ethereumAccount} balance={ethereumBalance} onClick={() => void connectEthereumWallet()} />
          <WalletChip network="zeko" account={zekoAccount} balance={zekoBalance} onClick={() => void connectAuroWallet()} />
        </div>
      </header>
      <main className="main-content">
        <div className="hero"><p className="eyebrow">Ethereum settlement</p><h1>Ethereum ↔ Zeko Bridge</h1><p className="hero-copy">Move native ETH between Ethereum custody and Zeko execution, with every transition proven through SP1 and anchored to settlement state.</p></div>
        <div className="environment-banner"><strong>Experimental PoC</strong><span>{ethereum}</span><span>·</span><span>No cancellation/refund</span><span>·</span><span>Zeko signs as temporary <code>testnet</code></span></div>
        <div className="bridge-card">
          <div className="card-header">
            <div className="tabs" role="tablist" aria-label="Bridge navigation"><button type="button" className={`tab${tab === "bridge" ? " active" : ""}`} role="tab" aria-selected={tab === "bridge"} onClick={() => setTab("bridge")}>Bridge</button><button type="button" className={`tab${tab === "activity" ? " active" : ""}`} role="tab" aria-selected={tab === "activity"} onClick={openActivity}>Activity</button></div>
            <button type="button" className="icon-button" onClick={() => setSettingsOpen(true)} aria-label="Open bridge settings"><span className="settings-glyph">⚙︎</span></button>
          </div>
          <div className="card-body">
            {actionError && <Notice kind="error">{actionError}</Notice>}
            {tab === "activity" ? <ActivityView deposits={deposits} withdrawals={withdrawals} operations={operations} loading={activityLoading} onDeposit={(deposit) => { setSelectedDeposit(deposit); setSelectedOperation(operations.find((row) => row.direction === "deposit" && row.depositNonce === deposit.nonce)); setScreen("deposit-progress"); setTab("bridge") }} onWithdrawal={(withdrawal, operation) => { setSelectedWithdrawal(withdrawal); setSelectedOperation(operation); setScreen("withdrawal-progress"); setTab("bridge") }} /> : bridgeContent}
          </div>
          <div className="card-footnote"><span className="footnote-proof">SP1</span><span>verifies the Zeko state transition</span><span>·</span><span>Ethereum verifies settlement</span></div>
        </div>
        <footer className="page-footer"><span><span className="health-dot"></span>{client ? "Gateway connected" : "Connect wallet to verify gateway"}</span><span>{bridgeAddress ? `Bridge ${shortAddress(bridgeAddress)}` : "Bridge address from gateway"}</span><span>Proof of concept</span></footer>
      </main>
      {settingsOpen && <SettingsModal config={config} showDetails={showDetails} onToggleDetails={() => setShowDetails((value) => !value)} onClose={() => setSettingsOpen(false)} />}
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
