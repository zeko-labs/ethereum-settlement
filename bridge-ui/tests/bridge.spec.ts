import { expect, test as base, type Page } from "@playwright/test"
import { encodeAbiParameters, parseAbiParameters } from "viem"

const test = base.extend<{ browserErrors: void }>({
  browserErrors: [async ({ page }, use) => {
    const errors: string[] = []
    page.on("pageerror", (error) => errors.push(error.message))
    page.on("console", (message) => {
      if (message.type() === "error") errors.push(message.text())
    })
    await use()
    expect(errors, "browser console and page errors").toEqual([])
  }, { auto: true }]
})

const ETH_ACCOUNT = "0x0000000000000000000000000000000000000001"
const BRIDGE_ADDRESS = "0x00000000000000000000000000000000000000b0"
const ZEKO_ACCOUNT = "B62qpuhMDp748xtE77iBXRRaipJYgs6yumAeTzaM7zS9dn8avLPaeFF"
const TX_HASH = `0x${"ab".repeat(32)}`
const BLOCK_HASH = `0x${"cd".repeat(32)}`
const DEPOSIT_TOPICS = [
  "0x587a076b072ed273d96c527559d92a57f840dfa2429ad93ddafba644bedeed09",
  `0x${"0".repeat(63)}1`,
  `0x${"11".repeat(32)}`,
  `0x${"22".repeat(32)}`
]
const DEPOSIT_DATA = encodeAbiParameters(
  parseAbiParameters("bytes32,address,address,uint256,uint256,uint256,uint64"),
  [
    `0x${"00".repeat(32)}`,
    "0x0000000000000000000000000000000000000000",
    ETH_ACCOUNT,
    1n,
    100000000000000000n,
    100000000n,
    4294967295n
  ]
)

const deposit = {
  nonce: 1,
  token: "0x0000000000000000000000000000000000000000",
  sender: ETH_ACCOUNT,
  zekoRecipient: "0x01",
  ethereumAmount: "100000000000000000",
  zekoAmount: "100000000",
  timeout: 4294967295,
  ethereumTransactionHash: TX_HASH,
  ethereumFinalized: true,
  bridgeJobId: "deposit-job-1",
  bridgeJobStatus: "confirmed",
  outerActionSequence: 1,
  outerActionStateAfter: `0x${"11".repeat(32)}`,
  synchronizedSettlementSequence: null,
  status: "bridgeProven",
  nextAction: "waitForSettlementSynchronization"
}

const installApiMocks = async (page: Page) => {
  await page.route("**/v1/bridge/config", (route) => route.fulfill({
    json: {
      schemaVersion: 1,
      chainId: 11155111,
      bridgeAddress: BRIDGE_ADDRESS,
      settlementAddress: "0x00000000000000000000000000000000000000c0",
      ethereumDecimals: 18,
      zekoNativeDecimals: 9,
      ethereumConfirmations: 1,
      withdrawalDelaySlots: 10,
      currentVirtualSlot: 100
    }
  }))
  await page.route("**/v1/bridge/deposits/1", (route) => route.fulfill({ json: deposit }))
  await page.route(/.*\/v1\/bridge\/deposits(?:\?.*)?$/, (route) => route.fulfill({ json: [deposit] }))
  await page.route(/.*\/v1\/bridge\/withdrawals(?:\?.*)?$/, (route) => route.fulfill({ json: [] }))
  await page.route("**/graphql", async (route) => {
    const request = route.request()
    if (request.method() !== "POST") return route.continue()
    const body = request.postDataJSON() as { query?: string }
    if (body.query?.includes("AccountBalance")) {
      return route.fulfill({ json: { data: { account: { balance: { total: "2.5" } } } } })
    }
    return route.fulfill({ json: { data: {} } })
  })
}

const installWallets = async (page: Page, options: { rejectDeposit?: boolean; rejectSwitch?: boolean } = {}) => {
  await page.addInitScript(({ account, zeko, bridge, txHash, blockHash, topics, data, rejectDeposit, rejectSwitch }) => {
    let chainId = rejectSwitch ? "0x1" : "0xaa36a7"
    const ethereumListeners = new Map<string, Set<(value: unknown) => void>>()
    window.ethereum = {
      request: async ({ method }) => {
        if (method === "eth_chainId") return chainId
        if (method === "eth_accounts" || method === "eth_requestAccounts") return [account]
        if (method === "eth_getBalance") return "0x4563918244f40000"
        if (method === "wallet_switchEthereumChain") {
          if (rejectSwitch) throw new Error("User rejected the request")
          chainId = "0xaa36a7"
          return null
        }
        if (method === "eth_sendTransaction") {
          if (rejectDeposit) throw new Error("User rejected the request")
          return txHash
        }
        if (method === "eth_getTransactionReceipt") {
          return {
            blockHash,
            blockNumber: "0x1",
            contractAddress: null,
            cumulativeGasUsed: "0x5208",
            effectiveGasPrice: "0x1",
            from: account,
            gasUsed: "0x5208",
            logs: [{
              address: bridge,
              blockHash,
              blockNumber: "0x1",
              data,
              logIndex: "0x0",
              removed: false,
              topics,
              transactionHash: txHash,
              transactionIndex: "0x0"
            }],
            logsBloom: `0x${"0".repeat(512)}`,
            status: "0x1",
            to: bridge,
            transactionHash: txHash,
            transactionIndex: "0x0",
            type: "0x2"
          }
        }
        if (method === "eth_blockNumber") return "0x1"
        throw new Error(`Unexpected Ethereum method: ${method}`)
      },
      on: (event, listener) => {
        const listeners = ethereumListeners.get(event) ?? new Set()
        listeners.add(listener)
        ethereumListeners.set(event, listeners)
      },
      removeListener: (event, listener) => ethereumListeners.get(event)?.delete(listener)
    }

    const auroListeners = new Map<string, Set<(value: never) => void>>()
    window.mina = {
      requestAccounts: async () => [zeko],
      requestNetwork: async () => ({ networkID: "testnet" }),
      addChain: async () => ({ networkID: "testnet" }),
      switchChain: async () => ({ networkID: "testnet" }),
      sendTransaction: async () => { throw new Error("Auro signing is not used in this isolated deposit test") },
      on: (event, listener) => {
        const listeners = auroListeners.get(event) ?? new Set()
        listeners.add(listener as (value: never) => void)
        auroListeners.set(event, listeners)
      },
      removeAllListeners: () => auroListeners.clear()
    }
  }, {
    account: ETH_ACCOUNT,
    zeko: ZEKO_ACCOUNT,
    bridge: BRIDGE_ADDRESS,
    txHash: TX_HASH,
    blockHash: BLOCK_HASH,
    topics: DEPOSIT_TOPICS,
    data: DEPOSIT_DATA,
    ...options
  })
}

const openConnectedApp = async (page: Page, options: { rejectDeposit?: boolean; rejectSwitch?: boolean } = {}) => {
  await installApiMocks(page)
  await installWallets(page, options)
  await page.goto("/")
  await expect(page.getByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })).toBeVisible()
  if (!options.rejectSwitch) {
    await expect(page.getByRole("button", { name: "Ethereum wallet 0x0000…0001" })).toBeVisible({ timeout: 20_000 })
    await page.getByRole("button", { name: /Connect Auro/ }).click()
    await expect(page.getByRole("button", { name: "Auro wallet B62qpu…aeFF" })).toBeVisible({ timeout: 20_000 })
  }
}

test("deposits native ETH through the injected wallet and resumes gateway progress", async ({ page }) => {
  await openConnectedApp(page)
  await page.getByLabel("Amount of native ETH to bridge").fill("0.1")
  await expect(page.getByRole("button", { name: /Review deposit/i })).toBeEnabled({ timeout: 20_000 })
  await page.getByRole("button", { name: /Review deposit/i }).click()
  await expect(page.getByText("Experimental PoC: there is no cancellation or refund path.")).toBeVisible()
  await page.getByRole("button", { name: "Confirm in Ethereum wallet" }).click()
  await expect(page.getByTestId("deposit-progress")).toContainText("Deposit #1", { timeout: 20_000 })
  await expect(page.getByTestId("deposit-progress")).toContainText("Bridge proof accepted")
})

test("builds the withdrawal review route with Auro's testnet signing salt", async ({ page }) => {
  await openConnectedApp(page)
  await page.getByRole("button", { name: "Reverse bridge direction" }).click()
  await page.getByLabel("Amount of native ETH to bridge").fill("0.05")
  await expect(page.getByRole("button", { name: /Review withdrawal/i })).toBeEnabled({ timeout: 20_000 })
  await page.getByRole("button", { name: /Review withdrawal/i }).click()
  await expect(page.getByText("Auro · testnet salt")).toBeVisible()
  await expect(page.getByRole("button", { name: "Confirm in Auro" })).toBeVisible()
})

test("recovers indexed deposit activity after reload and wallet reconnection", async ({ page }) => {
  await openConnectedApp(page)
  await page.reload()
  await expect(page.getByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })).toBeVisible()
  await page.getByRole("button", { name: /Connect Auro/ }).click()
  await page.getByRole("tab", { name: "Activity" }).click()
  await expect(page.getByText("0.1 ETH · Deposit #1")).toBeVisible({ timeout: 20_000 })
  await page.getByRole("button", { name: "Resume" }).click()
  await expect(page.getByTestId("deposit-progress")).toBeVisible()
})

test("shows actionable wallet errors for rejected signatures", async ({ page }) => {
  await openConnectedApp(page, { rejectDeposit: true })
  await page.getByLabel("Amount of native ETH to bridge").fill("0.1")
  await page.getByRole("button", { name: /Review deposit/i }).click()
  await page.getByRole("button", { name: "Confirm in Ethereum wallet" }).click()
  await expect(page.getByText("The wallet request was rejected.")).toBeVisible({ timeout: 20_000 })
})

test("surfaces a rejected Sepolia network switch", async ({ page }) => {
  await installApiMocks(page)
  await installWallets(page, { rejectSwitch: true })
  await page.goto("/")
  await page.getByRole("button", { name: /Connect wallet/ }).click()
  await expect(page.getByText("The wallet request was rejected.")).toBeVisible()
})

test("keeps the primary bridge surface inside desktop and mobile viewports", async ({ page }) => {
  await openConnectedApp(page)
  const card = page.locator(".bridge-card")
  const box = await card.boundingBox()
  const viewport = page.viewportSize()
  expect(box).not.toBeNull()
  expect(viewport).not.toBeNull()
  expect(box!.x).toBeGreaterThanOrEqual(0)
  expect(box!.x + box!.width).toBeLessThanOrEqual(viewport!.width + 1)
})
