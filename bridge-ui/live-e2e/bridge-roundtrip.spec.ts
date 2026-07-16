import { expect, test, type APIRequestContext, type Page, type TestInfo } from "@playwright/test"
import { installLiveWallets, selectWallets } from "./wallets"

declare const process: { env: Record<string, string | undefined> }

type Deposit = { nonce: number; status: string; zekoRecipient: string }
type WithdrawalRequest = {
  globalActionIndex: number
  transactionHash: string
  recipient: string
  status: string
}
type Withdrawal = {
  settlementSequence: number
  offset: number
  globalActionIndex: number
  recipient: string
  claimableSlot: number
  currentVirtualSlot: number
  status: string
}

type ExplorerSearch = {
  groups?: {
    deposits?: Array<{ nonce?: string }>
  }
}

const gateway = required("BRIDGE_E2E_GATEWAY_URL")
const rpcUrl = required("BRIDGE_E2E_RPC_URL")
const explorerUrl = required("BRIDGE_E2E_EXPLORER_UI_URL")
const proofApiKey = required("BRIDGE_E2E_PROOF_API_KEY")
const zekoPrivateKeys = required("BRIDGE_E2E_ZEKO_PRIVATE_KEYS").split(",")
const virtualMinaSlotSeconds = positiveInteger(
  "BRIDGE_E2E_VIRTUAL_MINA_SLOT_SECONDS",
  process.env.BRIDGE_E2E_VIRTUAL_MINA_SLOT_SECONDS ?? "12"
)
const timeline: Array<{ at: string; event: string; value?: unknown }> = []

test("two destination wallets complete isolated deposit and withdrawal roundtrips", async ({ page, request }, testInfo) => {
  test.setTimeout(45 * 60 * 1_000)
  const browserErrors: string[] = []
  page.on("pageerror", (error) => browserErrors.push(error.message))
  page.on("console", (message) => {
    if (message.type() === "error") browserErrors.push(message.text())
  })

  const ethereumAccounts = await rpc<string[]>("eth_accounts") as `0x${string}`[]
  expect(ethereumAccounts.length).toBeGreaterThanOrEqual(2)
  expect(zekoPrivateKeys).toHaveLength(2)
  const wallets = await installLiveWallets(page, ethereumAccounts.slice(0, 2), zekoPrivateKeys)
  expect(new Set(ethereumAccounts.slice(0, 2).map((account) => account.toLowerCase())).size).toBe(2)
  expect(new Set(wallets.zekoAccounts).size).toBe(2)

  try {
    await page.goto("/")
    await expect(page.getByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })).toBeVisible()
    await page.getByRole("button", { name: /Connect Auro/ }).click()

    const depositA = await submitDeposit(page, 0, wallets.zekoAccounts[0], "2")
    const depositB = await submitDeposit(page, 1, wallets.zekoAccounts[1], "3")
    record("deposits-submitted", { depositA, depositB })

    await assertDepositOwnership(page, depositA, depositB, wallets.zekoAccounts)
    await mine(2)
    await waitDeposit(request, depositA, (deposit) => deposit.status === "locked")
    await waitDeposit(request, depositB, (deposit) => deposit.status === "locked")
    const prove = await request.post(`${gateway}/v1/bridge/deposits/prove`, {
      headers: { "x-api-key": proofApiKey }
    })
    const proveBody = await prove.text()
    expect(prove.ok(), proveBody).toBeTruthy()
    record("deposit-proof-queued", JSON.parse(proveBody))

    await waitDeposit(request, depositA, depositHasBridgeProof)
    await waitDeposit(request, depositB, depositHasBridgeProof)
    await waitDeposit(request, depositA, (deposit) => deposit.status === "synchronized", 25 * 60_000)
    await waitDeposit(request, depositB, (deposit) => deposit.status === "synchronized", 25 * 60_000)
    record("deposits-synchronized")

    await finalizeDeposit(page, 0, 0, depositA)
    await finalizeDeposit(page, 1, 1, depositB)
    await assertActivityRow(page, 0, 0, `activity-deposit-${depositA}`, "finalized")
    await assertActivityRow(page, 1, 1, `activity-deposit-${depositB}`, "finalized")

    await assertExplorerDeposit(page, depositA, wallets.zekoAccounts[0])
    await assertExplorerDeposit(page, depositB, wallets.zekoAccounts[1])

    const withdrawalToB = await submitWithdrawal(page, 0, 1, ethereumAccounts[1], "1")
    const withdrawalToA = await submitWithdrawal(page, 1, 0, ethereumAccounts[0], "1")
    const pendingToB = await waitPendingWithdrawal(request, withdrawalToB, ethereumAccounts[1])
    const pendingToA = await waitPendingWithdrawal(request, withdrawalToA, ethereumAccounts[0])
    record("withdrawals-pending", { pendingToA, pendingToB })

    await assertWithdrawalOwnership(page, pendingToA, pendingToB)
    await assertExplorerWithdrawalTransaction(page, withdrawalToA)
    await assertExplorerWithdrawalTransaction(page, withdrawalToB)

    const settledToA = await waitWithdrawal(request, ethereumAccounts[0], pendingToA.globalActionIndex)
    const settledToB = await waitWithdrawal(request, ethereumAccounts[1], pendingToB.globalActionIndex)
    record("withdrawals-settled", { settledToA, settledToB })
    await assertSettledRow(page, 0, 0, settledToA)
    await assertSettledRow(page, 1, 1, settledToB)

    await advancePastWithdrawalDelay(settledToA, settledToB)
    await waitWithdrawalStatus(request, settledToA, "claimable")
    await waitWithdrawalStatus(request, settledToB, "claimable")

    await claimWithdrawal(page, 0, 0, settledToA)
    await claimWithdrawal(page, 1, 1, settledToB)
    await waitWithdrawalStatus(request, settledToA, "processed")
    await waitWithdrawalStatus(request, settledToB, "processed")
    await page.reload()
    await assertSettledRow(page, 0, 0, { ...settledToA, status: "processed" })
    await assertSettledRow(page, 1, 1, { ...settledToB, status: "processed" })
    await assertExplorerWithdrawal(page, settledToA, "processed")
    await assertExplorerWithdrawal(page, settledToB, "processed")

    expect(browserErrors, "browser console and page errors").toEqual([])
  } finally {
    await attachTimeline(testInfo)
  }
})

async function submitDeposit(page: Page, ethereumIndex: number, recipient: string, amount: string) {
  await selectWallets(page, ethereumIndex, ethereumIndex)
  await openNewTransfer(page, "deposit")
  await page.getByLabel("Amount of native ETH to bridge").fill(amount)
  await page.getByLabel("Zeko recipient").fill(recipient)
  await page.getByRole("button", { name: /Review deposit/i }).click()
  await page.getByRole("button", { name: "Confirm in Ethereum wallet" }).click()
  const heading = page.getByTestId("deposit-progress").getByRole("heading")
  await expect(heading).toContainText("Deposit #")
  const match = (await heading.textContent())?.match(/Deposit #(\d+)/)
  if (!match) throw new Error("Deposit progress did not expose its nonce")
  const nonce = Number(match[1])
  await page.getByRole("tab", { name: "Activity" }).click()
  return nonce
}

async function finalizeDeposit(page: Page, ethereumIndex: number, zekoIndex: number, nonce: number) {
  await selectWallets(page, ethereumIndex, zekoIndex)
  await openActivity(page)
  const row = page.getByTestId(`activity-deposit-${nonce}`)
  await expect(row).toContainText("synchronized")
  await row.getByRole("button", { name: "Resume" }).click()
  await expect(page.getByRole("button", { name: "Finalize on Zeko" })).toBeEnabled()
  await page.getByRole("button", { name: "Finalize on Zeko" }).click()
  await expect(page.getByRole("heading", { name: "Deposit finalized" })).toBeVisible({ timeout: 10 * 60_000 })
  await page.getByRole("button", { name: "View activity" }).click()
  await expect(page.getByTestId(`activity-deposit-${nonce}`)).toContainText("finalized", { timeout: 5 * 60_000 })
}

async function submitWithdrawal(
  page: Page,
  zekoIndex: number,
  ethereumIndex: number,
  recipient: string,
  amount: string
) {
  await selectWallets(page, ethereumIndex, zekoIndex)
  await openNewTransfer(page, "withdrawal")
  await page.getByLabel("Amount of native ETH to bridge").fill(amount)
  await page.getByLabel("Ethereum recipient").fill(recipient)
  await page.getByRole("button", { name: /Review withdrawal/i }).click()
  await page.getByRole("button", { name: "Confirm in Auro" }).click()
  await expect(page.getByTestId("withdrawal-progress")).toContainText("Withdrawal request submitted", {
    timeout: 10 * 60_000
  })
  const link = page.getByTestId("withdrawal-progress")
    .locator(".summary-row")
    .filter({ hasText: "Zeko transaction" })
    .getByRole("link")
  const href = await link.getAttribute("href")
  const hash = href?.split("/transactions/")[1]
  if (!hash) throw new Error("Withdrawal progress did not expose its Zeko transaction hash")
  await page.getByRole("tab", { name: "Activity" }).click()
  return decodeURIComponent(hash)
}

async function assertDepositOwnership(page: Page, nonceA: number, nonceB: number, zekoAccounts: string[]) {
  await selectWallets(page, 0, 0)
  await openActivity(page)
  await expect(page.getByTestId(`activity-deposit-${nonceA}`)).toBeVisible()
  await expect(page.getByTestId(`activity-deposit-${nonceB}`)).toHaveCount(0)
  await selectWallets(page, 1, 0)
  await expect(page.getByTestId(`activity-deposit-${nonceA}`)).toBeVisible()
  await expect(page.getByTestId(`activity-deposit-${nonceB}`)).toHaveCount(0)
  await selectWallets(page, 1, 1)
  await expect(page.getByTestId(`activity-deposit-${nonceB}`)).toBeVisible()
  await expect(page.getByTestId(`activity-deposit-${nonceA}`)).toHaveCount(0)
  record("deposit-ownership-verified", zekoAccounts)
}

async function assertWithdrawalOwnership(page: Page, toA: WithdrawalRequest, toB: WithdrawalRequest) {
  await selectWallets(page, 0, 0)
  await openActivity(page)
  await expect(page.getByTestId(`activity-withdrawal-${toA.globalActionIndex}`)).toBeVisible()
  await expect(page.getByTestId(`activity-withdrawal-${toB.globalActionIndex}`)).toHaveCount(0)
  await selectWallets(page, 1, 0)
  await expect(page.getByTestId(`activity-withdrawal-${toB.globalActionIndex}`)).toBeVisible()
  await expect(page.getByTestId(`activity-withdrawal-${toA.globalActionIndex}`)).toHaveCount(0)
  await selectWallets(page, 1, 1)
  await expect(page.getByTestId(`activity-withdrawal-${toB.globalActionIndex}`)).toBeVisible()
  await selectWallets(page, 1, 0)
  await expect(page.getByTestId(`activity-withdrawal-${toB.globalActionIndex}`)).toBeVisible()
}

async function assertActivityRow(page: Page, ethereumIndex: number, zekoIndex: number, id: string, status: string) {
  await selectWallets(page, ethereumIndex, zekoIndex)
  await openActivity(page)
  await expect(page.getByTestId(id)).toContainText(status)
}

async function assertSettledRow(page: Page, ethereumIndex: number, zekoIndex: number, withdrawal: Withdrawal) {
  await assertActivityRow(
    page,
    ethereumIndex,
    zekoIndex,
    `activity-withdrawal-${withdrawal.globalActionIndex}`,
    withdrawal.status
  )
  await expect(page.getByTestId(`activity-withdrawal-${withdrawal.globalActionIndex}`)).toHaveCount(1)
}

async function claimWithdrawal(page: Page, ethereumIndex: number, zekoIndex: number, withdrawal: Withdrawal) {
  await selectWallets(page, ethereumIndex, zekoIndex)
  await openActivity(page)
  await page.getByTestId(`activity-withdrawal-${withdrawal.globalActionIndex}`).getByRole("button", { name: "Resume" }).click()
  await expect(page.getByRole("button", { name: "Claim ETH on Ethereum" })).toBeEnabled()
  await page.getByRole("button", { name: "Claim ETH on Ethereum" }).click()
  await expect(page.getByRole("heading", { name: "Withdrawal claimed" })).toBeVisible()
}

async function assertExplorerDeposit(page: Page, nonce: number, recipient: string) {
  await page.goto(`${explorerUrl}/bridge/deposits/${nonce}`)
  await expect(page.getByRole("heading", { name: `Deposit #${nonce}` })).toBeVisible()
  await expect(page.getByText(recipient, { exact: true })).toBeVisible()
  const response = await fetch(`${gateway}/v1/explorer/search?q=${encodeURIComponent(recipient)}`)
  const search = await response.json() as ExplorerSearch
  expect(search.groups?.deposits?.some((deposit) => deposit.nonce === String(nonce))).toBeTruthy()
}

async function assertExplorerWithdrawalTransaction(page: Page, transactionHash: string) {
  await page.goto(`${explorerUrl}/transactions/${encodeURIComponent(transactionHash)}`)
  await expect(page.getByText("Native withdrawal request")).toBeVisible({ timeout: 5 * 60_000 })
}

async function assertExplorerWithdrawal(page: Page, withdrawal: Withdrawal, status: string) {
  await page.goto(`${explorerUrl}/bridge/withdrawals/${withdrawal.settlementSequence}/${withdrawal.offset}`)
  await expect(page.getByRole("heading", {
    name: `Withdrawal ${withdrawal.settlementSequence}:${withdrawal.offset}`
  })).toBeVisible()
  await expect(page.locator(".detail-hero")).toContainText(status)
}

async function openActivity(page: Page) {
  await page.goto(process.env.BRIDGE_E2E_BRIDGE_UI_URL ?? "http://127.0.0.1:4174")
  await expect(page.getByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })).toBeVisible()
  await page.getByRole("tab", { name: "Activity" }).click()
}

async function openNewTransfer(page: Page, direction: "deposit" | "withdrawal") {
  await page.goto(process.env.BRIDGE_E2E_BRIDGE_UI_URL ?? "http://127.0.0.1:4174")
  await expect(page.getByRole("heading", { name: "Ethereum ↔ Zeko Bridge" })).toBeVisible()
  const expected = direction === "deposit" ? "Deposit to Zeko" : "Withdraw to Ethereum"
  if (await page.getByRole("heading", { name: expected }).count() === 0) {
    await page.getByRole("button", { name: "Reverse bridge direction" }).click()
  }
  await expect(page.getByRole("heading", { name: expected })).toBeVisible()
}

async function waitDeposit(
  request: APIRequestContext,
  nonce: number,
  predicate: (deposit: Deposit) => boolean,
  timeout = 5 * 60_000
) {
  return poll(`deposit ${nonce}`, timeout, async () => {
    const response = await request.get(`${gateway}/v1/bridge/deposits/${nonce}`)
    if (!response.ok()) return undefined
    const deposit = await response.json() as Deposit
    record(`deposit-${nonce}-${deposit.status}`)
    return predicate(deposit) ? deposit : undefined
  })
}

async function waitPendingWithdrawal(request: APIRequestContext, hash: string, recipient: string) {
  return poll(`pending withdrawal ${hash}`, 10 * 60_000, async () => {
    const response = await request.get(
      `${gateway}/v1/bridge/withdrawal-requests?recipient=${encodeURIComponent(recipient)}`
    )
    if (!response.ok()) return undefined
    const rows = await response.json() as WithdrawalRequest[]
    return rows.find((row) => row.transactionHash === hash)
  })
}

async function waitWithdrawal(request: APIRequestContext, recipient: string, globalActionIndex: number) {
  return poll(`settled withdrawal ${globalActionIndex}`, 25 * 60_000, async () => {
    const response = await request.get(
      `${gateway}/v1/bridge/withdrawals?recipient=${encodeURIComponent(recipient)}`
    )
    if (!response.ok()) return undefined
    const rows = await response.json() as Withdrawal[]
    return rows.find((row) => row.globalActionIndex === globalActionIndex)
  })
}

async function waitWithdrawalStatus(request: APIRequestContext, withdrawal: Withdrawal, status: string) {
  return poll(`withdrawal ${withdrawal.globalActionIndex} ${status}`, 5 * 60_000, async () => {
    const response = await request.get(
      `${gateway}/v1/bridge/withdrawals/${withdrawal.settlementSequence}/${withdrawal.offset}`
    )
    if (!response.ok()) return undefined
    const current = await response.json() as Withdrawal
    record(`withdrawal-${withdrawal.globalActionIndex}-${current.status}`)
    return current.status === status ? current : undefined
  })
}

async function poll<T>(label: string, timeout: number, read: () => Promise<T | undefined>): Promise<T> {
  const deadline = Date.now() + timeout
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      const value = await read()
      if (value !== undefined) return value
    } catch (error) {
      lastError = error
    }
    await new Promise((resolve) => setTimeout(resolve, 2_000))
  }
  throw new Error(`${label} timed out${lastError ? `: ${String(lastError)}` : ""}`)
}

async function mine(blocks: number) {
  await rpc("anvil_mine", [`0x${blocks.toString(16)}`])
}

async function advanceTime(seconds: number) {
  const latest = await rpc<{ timestamp: string }>("eth_getBlockByNumber", ["latest", false])
  const nextTimestamp = Number(BigInt(latest.timestamp)) + seconds
  await rpc("evm_setNextBlockTimestamp", [nextTimestamp])
  await rpc("evm_mine")
}

async function advancePastWithdrawalDelay(...withdrawals: Withdrawal[]) {
  const remainingSlots = Math.max(
    0,
    ...withdrawals.map((withdrawal) => withdrawal.claimableSlot - withdrawal.currentVirtualSlot)
  )
  await advanceTime((remainingSlots + 2) * virtualMinaSlotSeconds)
}

async function rpc<T = unknown>(method: string, params: unknown[] = []): Promise<T> {
  const response = await fetch(rpcUrl, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: Date.now(), method, params })
  })
  const body = await response.json() as { result?: T; error?: { message?: string } }
  if (body.error) throw new Error(body.error.message ?? `${method} failed`)
  return body.result as T
}

function record(event: string, value?: unknown) {
  timeline.push({ at: new Date().toISOString(), event, value })
}

function depositHasBridgeProof(deposit: Deposit): boolean {
  return deposit.status === "bridgeProven" || deposit.status === "synchronized"
}

async function attachTimeline(testInfo: TestInfo) {
  await testInfo.attach("bridge-state-timeline", {
    body: JSON.stringify(timeline, null, 2),
    contentType: "application/json"
  })
}

function required(name: string): string {
  const value = process.env[name]
  if (!value) throw new Error(`${name} is required for the live bridge E2E suite`)
  return value
}

function positiveInteger(name: string, value: string): number {
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed) || parsed <= 0) throw new Error(`${name} must be a positive integer`)
  return parsed
}
