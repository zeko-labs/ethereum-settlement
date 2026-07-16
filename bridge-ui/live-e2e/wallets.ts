import type { Page } from "@playwright/test"
import { Mina, PrivateKey } from "o1js"

declare const process: { env: Record<string, string | undefined> }

export type LiveWallets = {
  ethereumAccounts: `0x${string}`[]
  zekoAccounts: string[]
}

export async function installLiveWallets(
  page: Page,
  ethereumAccounts: `0x${string}`[],
  zekoPrivateKeys: string[]
): Promise<LiveWallets> {
  const keys = zekoPrivateKeys.map((value) => PrivateKey.fromBase58(value))
  const zekoAccounts = keys.map((key) => key.toPublicKey().toBase58())
  let ethereumIndex = 0
  let zekoIndex = 0

  await page.exposeFunction("__e2eGetEthereumWalletIndex", () => ethereumIndex)
  await page.exposeFunction("__e2eGetZekoWalletIndex", () => zekoIndex)
  await page.exposeFunction("__e2eSetEthereumWalletIndex", (index: number) => {
    ethereumIndex = index
  })
  await page.exposeFunction("__e2eSetZekoWalletIndex", (index: number) => {
    zekoIndex = index
  })

  await page.exposeFunction("__e2eSignZekoTransaction", async (input: {
    transaction: unknown
  }) => {
    const key = keys[zekoIndex]
    if (!key) throw new Error(`No E2E Zeko key at index ${zekoIndex}`)
    const json = typeof input.transaction === "string"
      ? JSON.parse(input.transaction) as Parameters<typeof Mina.Transaction.fromJSON>[0]
      : input.transaction as Parameters<typeof Mina.Transaction.fromJSON>[0]
    const transaction = Mina.Transaction.fromJSON(json)
    for (const update of transaction.transaction.accountUpdates) {
      if (
        update.body.authorizationKind.isSigned.toBoolean() &&
        update.publicKey.equals(key.toPublicKey()).toBoolean()
      ) {
        update.lazyAuthorization = { kind: "lazy-signature" }
      }
    }
    const signed = transaction.sign([key])
    return JSON.stringify({ zkappCommand: JSON.parse(signed.toJSON()) })
  })

  await page.addInitScript(({ ethereumAccounts, zekoAccounts, rpcUrl }) => {
    type Listener = (value: unknown) => void
    let rpcId = 0
    type HarnessWindow = Window & typeof globalThis & {
      __e2eGetEthereumWalletIndex: () => Promise<number>
      __e2eGetZekoWalletIndex: () => Promise<number>
      __e2eSetEthereumWalletIndex: (index: number) => Promise<void>
      __e2eSetZekoWalletIndex: (index: number) => Promise<void>
      __e2eSignZekoTransaction: (input: { transaction: unknown }) => Promise<string>
    }
    const harness = window as HarnessWindow
    const ethereumListeners = new Map<string, Set<Listener>>()
    const auroListeners = new Map<string, Set<Listener>>()
    const emit = (listeners: Map<string, Set<Listener>>, event: string, value: unknown) => {
      for (const listener of listeners.get(event) ?? []) listener(value)
    }
    const rpc = async (method: string, params: unknown[] = []) => {
      const response = await fetch(rpcUrl, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: ++rpcId, method, params })
      })
      const body = await response.json() as { result?: unknown; error?: { message?: string } }
      if (body.error) throw new Error(body.error.message ?? `Ethereum RPC ${method} failed`)
      return body.result
    }

    Object.defineProperty(window, "ethereum", {
      configurable: true,
      value: {
        request: async ({ method, params = [] }: { method: string; params?: unknown[] }) => {
          if (method === "eth_accounts" || method === "eth_requestAccounts") {
            return [ethereumAccounts[await harness.__e2eGetEthereumWalletIndex()]]
          }
          if (method === "wallet_switchEthereumChain") return null
          return rpc(method, params)
        },
        on: (event: string, listener: Listener) => {
          const listeners = ethereumListeners.get(event) ?? new Set()
          listeners.add(listener)
          ethereumListeners.set(event, listeners)
        },
        removeListener: (event: string, listener: Listener) => {
          ethereumListeners.get(event)?.delete(listener)
        }
      }
    })
    Object.defineProperty(window, "mina", {
      configurable: true,
      value: {
        requestAccounts: async () => [zekoAccounts[await harness.__e2eGetZekoWalletIndex()]],
        requestNetwork: async () => ({ networkID: "testnet" }),
        addChain: async () => ({ networkID: "testnet" }),
        switchChain: async () => ({ networkID: "testnet" }),
        sendTransaction: async ({ transaction }: { transaction: unknown }) => ({
          signedData: await harness.__e2eSignZekoTransaction({ transaction })
        }),
        on: (event: string, listener: Listener) => {
          const listeners = auroListeners.get(event) ?? new Set()
          listeners.add(listener)
          auroListeners.set(event, listeners)
        },
        removeAllListeners: () => auroListeners.clear()
      }
    })
    Object.defineProperty(window, "__e2eWallets", {
      configurable: true,
      value: {
        selectEthereum: async (index: number) => {
          await harness.__e2eSetEthereumWalletIndex(index)
          emit(ethereumListeners, "accountsChanged", [ethereumAccounts[index]])
        },
        selectZeko: async (index: number) => {
          await harness.__e2eSetZekoWalletIndex(index)
          emit(auroListeners, "accountsChanged", [zekoAccounts[index]])
        }
      }
    })
  }, {
    ethereumAccounts,
    zekoAccounts,
    rpcUrl: required("BRIDGE_E2E_RPC_URL")
  })

  return { ethereumAccounts, zekoAccounts }
}

export async function selectWallets(page: Page, ethereumIndex: number, zekoIndex: number) {
  await page.evaluate(async ([ethereum, zeko]) => {
    const wallets = (window as unknown as {
      __e2eWallets: {
        selectEthereum: (index: number) => Promise<void>
        selectZeko: (index: number) => Promise<void>
      }
    }).__e2eWallets
    await wallets.selectEthereum(ethereum)
    await wallets.selectZeko(zeko)
  }, [ethereumIndex, zekoIndex])
}

function required(name: string): string {
  const value = process.env[name]
  if (!value) throw new Error(`${name} is required for the live bridge E2E suite`)
  return value
}
