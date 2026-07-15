import { expect, test, type Page } from "@playwright/test";

const fixtures = {
  summary: {
    schemaVersion: 1,
    asOf: "2026-07-15T15:00:00Z",
    sources: { archive: true, gateway: true, ethereum: true, sequencer: true },
    l2: {
      blockHeight: "18492",
      transactionCount: "18491",
      accountCount: "2310",
    },
    settlement: {
      latestSequence: "284",
      commitSchedule: {
        periodSeconds: 900,
        phase: "WAITING",
        lastAttemptStartedAt: "2026-07-15T14:52:30Z",
        nextAttemptAt: "2026-07-15T15:07:30Z",
      },
    },
    bridge: {
      depositCount: "147",
      withdrawalCount: "39",
      depositedAmount: "24820000000000000000",
    },
  },
  transaction: {
    hash: "5JtY7ZkLwcDaV8Njzv9zc1H7EmL4WQwNwJ8eqZ9GzGCxM2nu82qR",
    kind: "zkapp",
    status: "applied",
    failureReason: null,
    blockHeight: "18492",
    stateHash: "3NLstateHashExample18492",
    timestamp: "2026-07-15T14:59:48Z",
    feePayer: "B62qkJzXExampleFeePayer111111111111111111111111111x7Kj",
    source: null,
    receiver: null,
    amount: null,
    fee: "100000000",
    nonce: "18446744073709551615",
    memo: "Zeko transaction",
    accountUpdateCount: "1",
    accountUpdates: [
      {
        index: "0",
        publicKey: "B62qAccountUpdate11111111111111111111111111111111111",
        tokenId: "1",
        balanceChange: "1000000000",
        incrementNonce: false,
        callDepth: "0",
        authorizationKind: "Proof",
        useFullCommitment: true,
        mayUseToken: "No",
      },
    ],
  },
  block: {
    height: "18492",
    stateHash: "3NLstateHashExample18492",
    parentHash: "3NLstateHashExample18491",
    timestamp: "2026-07-15T14:59:48Z",
    chainStatus: "canonical",
    creator: "B62qCreator111111111111111111111111111111111111111",
    blockWinner: "B62qWinner1111111111111111111111111111111111111111",
    ledgerHash: "jwLedgerHash111111111111111111111111111111111111111111",
    globalSlot: "91884",
    transactionCount: "1",
    transaction: {
      hash: "5JtY7ZkLwcDaV8Njzv9zc1H7EmL4WQwNwJ8eqZ9GzGCxM2nu82qR",
      kind: "zkapp",
      status: "applied",
    },
  },
  settlement: {
    id: "event-284",
    source: "event",
    status: "confirmed",
    createdAt: "2026-07-15T14:58:00Z",
    batchSequence: "284",
    settlementCommandDigest:
      "5JSettlementCommandDigest111111111111111111111111111111",
    ethereumTransactionHash:
      "0x25e200000000000000000000000000000000000000000000000000000000b8c1",
    ledgerHash: "jwLedgerHash111111111111111111111111111111111111111111",
    outerActionState: "19483838493049493930494930394930394930394930394930394",
    outerActionStateLength: "147",
    innerActionState: "20483838493049493930494930394930394930394930394930394",
    innerActionStateLength: "39",
    slotLower: "91884",
    slotUpper: "91948",
    innerActionRoot:
      "0xroot000000000000000000000000000000000000000000000000000000000001",
    innerActionStartIndex: "38",
    innerActionCount: "1",
    claimableSlot: "91968",
    confirmations: "18",
    ethereumGasUsed: "450000",
    cycleCount: "52146595101",
  },
  deposit: {
    nonce: "147",
    token: "0x0000000000000000000000000000000000000000",
    sender: "0x8Fb20000000000000000000000000000000019A0",
    zekoRecipient: "B62qDepositRecipient111111111111111111111111111111x7Kj",
    ethereumAmount: "84000000000000000",
    zekoAmount: "84000000",
    timeout: "1000",
    ethereumTransactionHash:
      "0xdeposit00000000000000000000000000000000000000000000000000000147",
    ethereumBlockNumber: "8800147",
    ethereumFinalized: true,
    bridgeJobId: "98ab8f02-6d31-489a-b251-36cae4f7968c",
    bridgeJobStatus: "confirmed",
    outerActionSequence: "147",
    outerActionStateAfter: "123456789",
    synchronizedSettlementSequence: "284",
    status: "synchronized",
    nextAction: "finalizeOnZeko",
    accuracyNote:
      "Synchronization is authoritative; the archive does not persist a canonical deposit-nonce to L2-finalization mapping.",
  },
  withdrawal: {
    settlementSequence: "284",
    offset: 3,
    globalActionIndex: "38",
    recipient: "0x71aC000000000000000000000000000000000b2E",
    amount: "12000000",
    actionFieldsHash:
      "0xaction00000000000000000000000000000000000000000000000000000001",
    siblings: Array.from({ length: 16 }, (_, i) => `0xsibling${i}`),
    innerActionRoot:
      "0xroot000000000000000000000000000000000000000000000000000000000001",
    commitSlotUpper: 91948,
    claimableSlot: "91968",
    currentVirtualSlot: "91965",
    recipientCursor: "38",
    status: "waitingForDelay",
    nextAction: "waitForWithdrawalDelay",
  },
};

async function mockApi(page: Page) {
  await page.route("http://127.0.0.1:8080/v1/explorer/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    let body: unknown;
    if (path.endsWith("/summary")) body = fixtures.summary;
    else if (path.endsWith("/search"))
      body = {
        query: url.searchParams.get("q"),
        groups: {
          blocks: [
            {
              height: fixtures.block.height,
              stateHash: fixtures.block.stateHash,
            },
          ],
          transactions: [
            {
              hash: fixtures.transaction.hash,
              kind: fixtures.transaction.kind,
            },
          ],
          accounts: [],
          settlements: [],
          deposits: [],
          withdrawals: [],
        },
      };
    else if (path.endsWith("/blocks"))
      body = { items: [fixtures.block], nextCursor: null };
    else if (path.includes("/blocks/")) body = fixtures.block;
    else if (path.endsWith("/transactions"))
      body = { items: [fixtures.transaction], nextCursor: null };
    else if (path.includes("/transactions/")) body = fixtures.transaction;
    else if (path.includes("/accounts/"))
      body = {
        publicKey: fixtures.transaction.feePayer,
        tokenId: "1",
        balance: "12345678901234567890",
        nonce: fixtures.transaction.nonce,
        delegate: null,
        lastUpdatedBlock: "18492",
        lastUpdatedStateHash: fixtures.block.stateHash,
        transactions: [fixtures.transaction],
      };
    else if (path.endsWith("/settlements"))
      body = { items: [fixtures.settlement], nextCursor: null };
    else if (path.includes("/settlements/")) body = fixtures.settlement;
    else if (path.endsWith("/deposits"))
      body = { items: [fixtures.deposit], nextCursor: null };
    else if (path.includes("/deposits/")) body = fixtures.deposit;
    else if (path.endsWith("/withdrawals"))
      body = { items: [fixtures.withdrawal], nextCursor: null };
    else if (path.includes("/withdrawals/")) body = fixtures.withdrawal;
    else body = { error: `unhandled ${path}` };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  });
}

test.beforeEach(async ({ page }) => {
  await mockApi(page);
});

test("overview joins execution, settlement, and both bridge directions", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(
    page.getByRole("heading", {
      name: "Network activity, from execution to settlement.",
    }),
  ).toBeVisible();
  await expect(page.getByText("18,492")).toBeVisible();
  await expect(page.getByText("Next commit")).toBeVisible();
  await expect(page.getByText("Every 15m")).toBeVisible();
  await expect(page.getByText("Deposit #147")).toBeVisible();
  await expect(page.getByText("Withdrawal 284:3")).toBeVisible();
  await expect(page.getByText("Settlement #284")).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBe(0);
  expect(errors).toEqual([]);
});

test("search and full detail routes remain linkable after reload", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByRole("heading", {
      name: "Network activity, from execution to settlement.",
    }),
  ).toBeVisible();
  await page.keyboard.press("/");
  await page.getByPlaceholder(/Block height/).fill("18492");
  const result = page
    .getByRole("dialog", { name: "Global search" })
    .getByRole("button", { name: /Block/ });
  await expect(result).toBeVisible();
  await result.click();
  await expect(page).toHaveURL(/\/blocks\/18492$/);
  await expect(page.getByText("State hash")).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Block 18,492" }),
  ).toBeVisible();
});

test("bridge list opens deposit, withdrawal, and explorer links", async ({
  page,
}) => {
  await page.goto("/bridge");
  await page.getByRole("button", { name: /Deposit #147/ }).click();
  await expect(
    page.getByRole("heading", { name: "Deposit #147" }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: /0xdeposit/ })).toHaveAttribute(
    "href",
    /sepolia\.etherscan\.io\/tx/,
  );
  await page.getByRole("button", { name: /Bridge activity/ }).click();
  await page.getByRole("button", { name: /Withdrawal 284:3/ }).click();
  await expect(
    page.getByRole("heading", { name: "Withdrawal 284:3" }),
  ).toBeVisible();
  await expect(page.getByText("Claimable slot")).toBeVisible();
});

test("responsive navigation and data surfaces fit mobile", async ({
  page,
  isMobile,
}) => {
  test.skip(!isMobile, "mobile project only");
  await page.goto("/");
  await expect(
    page.getByRole("navigation", { name: "Mobile navigation" }),
  ).toBeVisible();
  await expect(page.getByText("Deposit #147")).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    ),
  ).toBe(0);
});
